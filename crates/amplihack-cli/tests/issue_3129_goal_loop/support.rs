//! Harness for the #3129 goal-loop behaviour tests.
//!
//! Real: the `amplihack` binary, the pinned recipe runner, the recipes, tools
//! and goal text of a byte-for-byte copy of this checkout's bundle, `git`,
//! `jq`, and a bare origin with `receive.denyNonFastForwards=true` and
//! `receive.denyDeletes=true` (so a rebase-and-force-push or a branch delete
//! is refused by the remote itself).
//!
//! Stand-ins: `gh` (a script on PATH, [`FAKE_GH`]) and the agent (a script
//! the real launcher starts through `AMPLIHACK_CLAUDE_BINARY_PATH`,
//! [`STUB_AGENT`]). No test opens or reads a file under `amplifier-bundle/`,
//! `crates/`, `docs/` or `skills/`; the only thing read from them is the
//! bundle directory copied byte for byte for the binary. What tests read is
//! what the run produced: the call log, comments, the remote's refs, the
//! state folder.
//!
//! Stand-in gh. State comes from fixture files in `$GH_STATE`, and from the
//! bare remote wherever a test depends on it: the head is the remote's PR
//! branch, `mergeable` is `git merge-tree --write-tree`, `BEHIND` is the base
//! having commits the PR lacks (only when `strict` exists). Every call is
//! appended to `calls.log`. `pr merge` refuses a `--match-head-commit` that is not
//! the remote's head, and a method the repository does not allow (by default
//! merge and squash are allowed and rebase is not; `allow_methods` changes
//! that). `pr checks --watch` blocks reading
//! `$GH_STATE/release.fifo`; a test releases it (`fifo_release`), and if the
//! run exits early the harness's watcher opens the FIFO itself, so nothing
//! sleeps and nothing times out.
//!
//! Stub agent. It reads nothing from the prompt. When a turn folder under
//! `$STUB_STATE_DIR/turns/` has a `reading.json` and no `decision.json`, it
//! is the turn's agent: it runs the test's policy (`$STUB_POLICY`, a bash
//! snippet) in the repo worktree with `$TURN` set, and prints the snippet's
//! stdout. For any other agent step it prints `{}`, or the reply a test set
//! with [`Harness::default_reply`]. A policy chooses by a
//! fixed rule over `reading.json` and may do fixed work (resolve a conflict,
//! run the setup script, do the task's later step), exactly what the real
//! agent is told it may do.
//!
//! Reading fields the policies use (the contract of `reading.json`, schema 1,
//! in `docs/reference/autodrive-goal-loop.md`): `.goal.pr.head`,
//! `.goal.pr.state`, `.goal.checks.counts.pending`, `.goal.base.conflict_files`,
//! `.goal.merge_authority.effective`, `.guidance[]` with `.name` and `.new`,
//! and `.previous.outcome`. The agent's final object is
//! `{"action": word, "args": {...}, "why": "..."}`; `handoff` args are
//! `owner`, `decision`, `text`; `ready` args are `text`.
//!
//! Every path is under the test's own temporary folder (TMPDIR).

#![cfg(target_os = "linux")]
#![allow(dead_code)]

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::exec_stub::write_executable;
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

pub const PR: &str = "7";
pub const PR_URL: &str = "https://github.com/o/r/pull/7";
pub const PR_BRANCH: &str = "feat/widget";

pub const FAKE_GH: &str = r##"#!/usr/bin/env bash
set -u
G="$GH_STATE"; O="$GH_ORIGIN"; BR="$GH_PR_BRANCH"; BASE=main
printf '%s\n' "$*" >> "$G/calls.log"
gitO() { git --git-dir="$O" "$@"; }
head() { gitO rev-parse "refs/heads/$BR"; }
state() { if [ -e "$G/merged" ]; then echo MERGED; elif [ -e "$G/closed" ]; then echo CLOSED; else echo OPEN; fi; }
mergeable() { if gitO merge-tree --write-tree "refs/heads/$BASE" "refs/heads/$BR" >/dev/null 2>&1; then echo MERGEABLE; else echo CONFLICTING; fi; }
behind() { [ "$(gitO rev-list --count "refs/heads/$BR..refs/heads/$BASE")" != 0 ]; }
pending() { grep -q $'\tpending' "$G/checks" 2>/dev/null; }
failing() { grep -q $'\tfail' "$G/checks" 2>/dev/null; }
mss() {
  if [ "$(mergeable)" = CONFLICTING ]; then echo DIRTY
  elif pending || failing; then echo BLOCKED
  elif [ -e "$G/strict" ] && behind; then echo BEHIND
  else echo CLEAN; fi
}
settings() { if [ -e "$G/repo.json" ]; then cat "$G/repo.json"; else echo '{"allow_merge_commit":true,"allow_squash_merge":true,"allow_rebase_merge":false,"default_branch":"main"}'; fi; }
filter() { local f=""; while [ $# -gt 0 ]; do case "$1" in --jq|-q) f="$2"; shift 2 ;; *) shift ;; esac; done
  if [ -n "$f" ]; then jq -r "$f"; else cat; fi; }
case "${1:-} ${2:-}" in
  "pr view")
    jq -cn --arg h "$(head)" --arg s "$(state)" --arg m "$(mergeable)" --arg ms "$(mss)" --arg b "$BR" --arg x "$([ -e "$G/cross" ] && echo true || echo false)" \
      --arg bn "$(cat "$G/basename" 2>/dev/null || echo main)" \
      '{number:7,state:$s,headRefOid:$h,headRefName:$b,baseRefName:$bn,mergeable:$m,mergeStateStatus:$ms,
        isDraft:false,isCrossRepository:($x=="true"),title:"Fixture",body:"Fixture body",url:"https://github.com/o/r/pull/7",
        reviews:[],reviewDecision:"",headRepository:{name:"r"},headRepositoryOwner:{login:"o"},
        author:{login:"alice"},files:[{path:"widget.txt"}]}' | filter "$@" ;;
  "pr checks")
    watch=0; for a in "$@"; do [ "$a" = --watch ] && watch=1; done
    [ -e "$G/watch79" ] && exit 79
    if [ "$watch" = 1 ]; then read -r _ < "$G/release.fifo"; fi
    cat "$G/checks" 2>/dev/null
    if pending; then exit 8; elif failing; then exit 1; fi; exit 0 ;;
  "pr merge")
    want=""; method=""; prev=""
    for a in "$@"; do
      case "$prev" in --match-head-commit) want="$a" ;; esac
      case "$a" in --merge|--squash|--rebase) method="$a" ;; --admin) echo "no --admin" >&2; exit 1 ;; esac
      prev="$a"
    done
    [ "$want" = "$(head)" ] || { echo "head branch was modified (match-head-commit)" >&2; exit 1; }
    case "$method" in
      --merge) k=allow_merge_commit ;; --squash) k=allow_squash_merge ;; --rebase) k=allow_rebase_merge ;; *) k=none ;;
    esac
    [ "$(settings | jq -r --arg k "$k" '.[$k] // false')" = true ] || { echo "merge method not allowed" >&2; exit 1; }
    pending && { echo "required status checks are expected" >&2; exit 1; }
    : > "$G/merged"; echo merged ;;
  "pr comment")
    n=$(( $(cat "$G/comment.count" 2>/dev/null || echo 0) + 1 )); echo "$n" > "$G/comment.count"
    prev=""; mkdir -p "$G/comments"
    for a in "$@"; do
      case "$prev" in --body-file|-F) cp -- "$a" "$G/comments/$n.md" ;; --body|-b) printf '%s' "$a" > "$G/comments/$n.md" ;; esac
      prev="$a"
    done ;;
  "pr ready"|"pr edit") exit 0 ;;
  "repo view") echo '{"nameWithOwner":"o/r","defaultBranchRef":{"name":"main"}}' | filter "$@" ;;
  "auth status") exit 0 ;;
  api*)
    case "$*" in
      *rules/branches*) if [ -e "$G/strict" ]; then
          echo '[{"type":"required_status_checks","parameters":{"strict_required_status_checks_policy":true}}]'
        else echo '[]'; fi ;;
      *protection*) echo "HTTP 404: Branch not protected" >&2; exit 1 ;;
      *repos/*) settings | filter "$@" ;;
      *) echo "stand-in gh: unexpected api call: $*" >&2; exit 1 ;;
    esac ;;
  *) echo "stand-in gh: unexpected call: $*" >&2; exit 1 ;;
esac
"##;

pub const STUB_AGENT: &str = r##"#!/usr/bin/env bash
if [ "${1:-}" = "--version" ]; then echo "2.1.329 (Claude Code)"; exit 0; fi
cat > /dev/null
turn=""
# STUB_STATE_GLOB (unquoted on purpose) finds turns when the runner picks the
# state folder itself; otherwise the test's own folder.
for d in $(ls -1d ${STUB_STATE_GLOB:-"$STUB_STATE_DIR"}/turns/*/ 2>/dev/null | sort); do
  [ -f "$d/reading.json" ] && [ ! -f "$d/decision.json" ] && turn="${d%/}"
done
if [ -z "$turn" ]; then
  if [ -f "$STUB_LOG/default-reply.json" ]; then cat "$STUB_LOG/default-reply.json"; else echo '{}'; fi
  exit 0
fi
printf '%s\n' "$turn" >> "$STUB_LOG/agent-turns.log"
export TURN="$turn"
cd "$STUB_REPO" || exit 1
export GIT_AUTHOR_NAME=Stub GIT_AUTHOR_EMAIL=stub@example.invalid GIT_COMMITTER_NAME=Stub GIT_COMMITTER_EMAIL=stub@example.invalid
# shellcheck disable=SC1090
. "$STUB_POLICY"
"##;

/// Everything one test needs, under one temporary folder.
pub struct Harness {
    pub _tmp: tempfile::TempDir,
    pub base: PathBuf,
    pub home: PathBuf,
    pub bundle_root: PathBuf,
    pub bin: PathBuf,
    pub gh: PathBuf,
    pub origin: PathBuf,
    pub repo: PathBuf,
    pub state: PathBuf,
    pub state_root: PathBuf,
    pub work: PathBuf,
    pub policy: PathBuf,
    pub fifo: PathBuf,
    pub initial_head: String,
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = amplihack_git::command_in(dir)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
        ])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        let kind = entry.file_type().unwrap();
        if kind.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(entry.path()).unwrap(), &dest).unwrap();
        } else if kind.is_dir() {
            copy_tree(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), &dest).unwrap();
        }
    }
}

fn private(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

impl Harness {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let home = base.join("home");
        let bundle_root = base.join("trusted");
        let bin = base.join("bin");
        let gh = base.join("ghstate");
        let origin = base.join("origin.git");
        let repo = base.join("repo");
        let state = base.join("state");
        let state_root = base.join("state-root");
        let work = base.join("work");
        for d in [&home, &bin, &gh, &repo] {
            fs::create_dir_all(d).unwrap();
        }
        for d in [&state, &state_root, &work] {
            private(d);
        }
        // Byte-for-byte copy of the bundle: the only thing read from it.
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../amplifier-bundle");
        copy_tree(&src, &bundle_root.join("amplifier-bundle"));

        git(
            &base,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                origin.to_str().unwrap(),
            ],
        );
        git(&origin, &["config", "receive.denyNonFastForwards", "true"]);
        git(&origin, &["config", "receive.denyDeletes", "true"]);
        git(&repo, &["init", "-q", "-b", "main"]);
        git(
            &repo,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        fs::write(
            repo.join("README.md"),
            "Setup: run scripts/setup to create .venv\n",
        )
        .unwrap();
        fs::write(repo.join("widget.txt"), "line one\nline two\nline three\n").unwrap();
        fs::create_dir_all(repo.join("scripts")).unwrap();
        write_executable(
            &repo.join("scripts/setup"),
            "#!/bin/sh\nmkdir -p .venv\n",
            0o755,
        );
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "base"]);
        git(&repo, &["push", "-q", "origin", "main"]);
        git(&repo, &["checkout", "-q", "-b", PR_BRANCH]);
        fs::write(
            repo.join("widget.txt"),
            "line one\nline two changed by the PR\nline three\n",
        )
        .unwrap();
        git(&repo, &["commit", "-qam", "the PR's change"]);
        git(&repo, &["push", "-q", "origin", PR_BRANCH]);
        let initial_head = git(&repo, &["rev-parse", "HEAD"]);

        write_executable(&bin.join("gh"), FAKE_GH, 0o755);
        write_executable(&bin.join("claude"), STUB_AGENT, 0o755);
        std::os::unix::fs::symlink(amplihack_bin(), bin.join("amplihack")).unwrap();
        fs::write(
            gh.join("checks"),
            "build\tpass\t1m\thttps://example.invalid\n",
        )
        .unwrap();
        let fifo = base.join("stub.fifo");
        crate::support::fifo_release::mkfifo(&gh.join("release.fifo"));
        crate::support::fifo_release::mkfifo(&fifo);
        let policy = base.join("policy.sh");
        Harness {
            _tmp: tmp,
            base,
            home,
            bundle_root,
            bin,
            gh,
            origin,
            repo,
            state,
            state_root,
            work,
            policy,
            fifo,
            initial_head,
        }
    }

    /// What the stub agent answers for every agent step that is not a turn's
    /// (the build stage's preflight and PR resolution, for example).
    pub fn default_reply(&self, reply: &Value) {
        fs::write(self.base.join("default-reply.json"), format!("{reply}\n")).unwrap();
    }

    /// The main branch's commit on the remote.
    pub fn base_head(&self) -> String {
        self.origin_head("main")
    }

    /// The agent's policy: bash that prints the final JSON object.
    pub fn policy(&self, body: &str) {
        fs::write(&self.policy, body).unwrap();
    }

    pub fn set_checks(&self, tsv: &str) {
        fs::write(self.gh.join("checks"), tsv).unwrap();
    }

    /// The repository's allowed merge methods, as `gh api repos/o/r` reports
    /// them and as the stand-in `pr merge` enforces them.
    pub fn allow_methods(&self, merge: bool, squash: bool, rebase: bool) {
        let v = serde_json::json!({"allow_merge_commit": merge, "allow_squash_merge": squash,
            "allow_rebase_merge": rebase, "default_branch": "main"});
        fs::write(self.gh.join("repo.json"), v.to_string()).unwrap();
    }

    /// The merge action's record, `merge-result.json` in the state folder.
    pub fn merge_result(&self) -> Value {
        let raw = fs::read_to_string(self.state.join("merge-result.json"))
            .unwrap_or_else(|e| panic!("no merge-result.json: {e}"));
        serde_json::from_str(raw.trim()).unwrap()
    }

    /// The run's report (`autodrive_outcome`) from `--format json` stdout.
    pub fn outcome(out: &Output) -> Value {
        let v: Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("stdout is not the run's JSON ({e}): {}", describe(out)));
        v["autodrive_outcome"].clone()
    }

    /// Makes the stand-in PR cross-repository.
    pub fn cross_repository(&self) {
        fs::write(self.gh.join("cross"), "").unwrap();
    }

    /// Makes the stand-in report this base branch name.
    pub fn base_name(&self, name: &str) {
        fs::write(self.gh.join("basename"), name).unwrap();
    }

    /// Makes `gh pr checks` exit 79, as `gh` does when the login is gone.
    pub fn checks_exit_79(&self) {
        fs::write(self.gh.join("watch79"), "").unwrap();
    }

    pub fn result(&self, turn: &str) -> serde_json::Value {
        let raw =
            fs::read_to_string(self.state.join("turns").join(turn).join("result.json")).unwrap();
        serde_json::from_str(&raw).unwrap()
    }

    pub fn strict(&self) {
        fs::write(self.gh.join("strict"), "").unwrap();
    }

    /// Advances the base on the remote by one commit (so the PR is behind).
    /// With `conflicting`, the commit edits the line the PR changed.
    pub fn advance_base(&self, conflicting: bool) {
        let body = if conflicting {
            "line one\nline two changed by the base\nline three\n"
        } else {
            "line one\nline two\nline three\n"
        };
        let file = if conflicting {
            "widget.txt"
        } else {
            "unrelated.txt"
        };
        let tmp = self.base.join("base-work");
        git(
            &self.base,
            &[
                "clone",
                "-q",
                self.origin.to_str().unwrap(),
                tmp.to_str().unwrap(),
            ],
        );
        git(&tmp, &["checkout", "-q", "main"]);
        fs::write(tmp.join(file), body).unwrap();
        git(&tmp, &["add", "."]);
        git(&tmp, &["commit", "-qm", "the base moves on"]);
        git(&tmp, &["push", "-q", "origin", "main"]);
        fs::remove_dir_all(&tmp).unwrap();
    }

    pub fn origin_head(&self, branch: &str) -> String {
        git(
            &self.origin,
            &["rev-parse", &format!("refs/heads/{branch}")],
        )
    }

    /// The run's own `amplihack` command, ready to spawn.
    pub fn command(&self, args: &[&str]) -> Command {
        let path = format!("{}:/usr/bin:/bin", self.bin.display());
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(args)
            .current_dir(&self.repo)
            .env("HOME", &self.home)
            .env("TMPDIR", &self.base)
            .env("PATH", path)
            .env("AMPLIHACK_HOME", &self.bundle_root)
            .env("AMPLIHACK_STATE_DIR", &self.state_root)
            .env("AMPLIHACK_WORK_ROOT", &self.work)
            .env(
                "RECIPE_RUNNER_RS_PATH",
                crate::issue_2539_step_recovery_fixture::real_runner(),
            )
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", self.bin.join("claude"))
            .env("GH_STATE", &self.gh)
            .env("GH_ORIGIN", &self.origin)
            .env("GH_PR_BRANCH", PR_BRANCH)
            .env("STUB_STATE_DIR", &self.state)
            .env("STUB_LOG", &self.base)
            .env("STUB_REPO", &self.repo)
            .env("STUB_POLICY", &self.policy)
            .env("STUB_FIFO", &self.fifo)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .env("AMPLIHACK_CLEANUP_ON_EXIT", "0")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_DISABLE_BLARIFY", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        c
    }

    /// `amplihack recipe run <recipe> -c pr_number=7 ...` in the repo.
    pub fn drive_command(&self, recipe: &str, extra: &[&str]) -> Command {
        let state = format!("autodrive_state_dir={}", self.state.display());
        let mut args: Vec<String> = vec!["recipe".into(), "run".into(), recipe.into()];
        for kv in [
            format!("pr_number={PR}"),
            "task_description=Get this pull request merged.".to_string(),
            state,
        ] {
            args.push("-c".into());
            args.push(kv);
        }
        for kv in extra {
            args.push("-c".into());
            args.push((*kv).to_string());
        }
        for a in [
            "--format",
            "json",
            "-w",
            self.repo.to_str().unwrap(),
            "--no-detach",
        ] {
            args.push(a.into());
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.command(&refs)
    }

    pub fn drive(&self, recipe: &str, extra: &[&str]) -> Output {
        self.drive_command(recipe, extra)
            .output()
            .expect("amplihack recipe run")
    }

    /// The real `amplihack recipe guide`, for this PR, with `text` on stdin.
    pub fn guide(&self, text: &str) -> Output {
        use std::io::Write;
        let mut c = self.command(&["recipe", "guide", PR_URL, "-"]);
        c.stdin(Stdio::piped());
        let mut child = c.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{}", describe(&out));
        out
    }

    // --- what the run produced -------------------------------------------

    pub fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.gh.join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    pub fn merge_calls(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|c| c.starts_with("pr merge"))
            .collect()
    }

    pub fn comments(&self) -> Vec<String> {
        let dir = self.gh.join("comments");
        let mut names: Vec<_> = fs::read_dir(&dir)
            .map(|d| d.map(|e| e.unwrap().path()).collect())
            .unwrap_or_default();
        names.sort_by_key(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.parse::<u32>().ok())
        });
        names
            .iter()
            .map(|p| fs::read_to_string(p).unwrap())
            .collect()
    }

    pub fn loop_end(&self) -> Value {
        let raw = fs::read_to_string(self.state.join("loop-end.json"))
            .unwrap_or_else(|e| panic!("no loop-end.json in {}: {e}", self.state.display()));
        serde_json::from_str(raw.trim()).unwrap()
    }

    pub fn turns(&self) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(self.state.join("turns"))
            .map(|d| {
                d.map(|e| e.unwrap().file_name().into_string().unwrap())
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    pub fn reading(&self, turn: &str) -> Value {
        let raw =
            fs::read_to_string(self.state.join("turns").join(turn).join("reading.json")).unwrap();
        serde_json::from_str(&raw).unwrap()
    }

    pub fn record(&self, turn: &str) -> String {
        fs::read_to_string(self.state.join("turns").join(turn).join("record.md")).unwrap()
    }

    /// Commits on the remote's PR branch with more than one parent.
    pub fn merge_commits_on(&self, branch: &str) -> Vec<String> {
        git(
            &self.origin,
            &["rev-list", "--merges", &format!("refs/heads/{branch}")],
        )
        .lines()
        .map(str::to_owned)
        .collect()
    }

    pub fn is_ancestor(&self, a: &str, b: &str) -> bool {
        amplihack_git::command_in(&self.origin)
            .args(["merge-base", "--is-ancestor", a, b])
            .status()
            .unwrap()
            .success()
    }
}

pub fn describe(out: &Output) -> String {
    format!(
        "status: {}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The policy "merge when nothing is pending", used by several tests.
pub const MERGE_WHEN_GREEN: &str = r#"
head=$(jq -r '.goal.pr.head' "$TURN/reading.json")
pending=$(jq -r '.goal.checks.counts.pending // 0' "$TURN/reading.json")
if [ "$pending" != "0" ]; then
  echo '{"action":"wait","args":{"required":true},"why":"A required check is still running at this head, so I wait for it."}'
else
  printf '{"action":"merge","args":{"head":"%s"},"why":"Every required check passed at this head and the owner'"'"'s conditions hold."}\n' "$head"
fi
"#;
