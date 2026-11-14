//! Issue #2471 (first slice of #2472): a failed push step gets an agentic
//! recovery instead of ending the run on an environment fault.
//!
//! Each case builds a small recipe from the real step definitions in
//! `workflow-publish.yaml` and `workflow-pr-review.yaml`, taken by id, and
//! runs it with the real
//! `recipe-runner-rs`. The recovery call step resolves the real
//! `push-failure-recovery` sub-recipe from the bundle at `AMPLIHACK_HOME`
//! (this checkout).
//!
//! Fixture, all under one TempDir:
//! - private `HOME`, `TMPDIR`, `XDG_*`; `GIT_CONFIG_GLOBAL=/dev/null`,
//!   `GIT_CONFIG_NOSYSTEM=1`; the environment is cleared by `isolated(..)`, so
//!   no sign-in or `AMPLIHACK_*` variable reaches the run;
//! - a local bare remote whose `pre-receive` hook rejects any push that
//!   `RECIPE_VAR_issue_number` reaches (or every push, when `reject-always`
//!   exists);
//! - a linked worktree on `feat/x`, one commit ahead of `origin/feat/x`;
//! - a fake `AMPLIHACK_LAUNCHER_BINARY` that stands in for the recovery
//!   agent. It refuses to run outside the TempDir, makes the clean-environment
//!   push (`env -u` for each `RECIPE_VAR_*`/`WORKFLOW_*` name) and prints the
//!   agent JSON. Its behaviour is chosen by the `agent-mode` file. Agent
//!   steps get only the inherited environment, not the recipe's context, so
//!   a case reads the snapshot from the runner's JSON context instead;
//! - step-20b is not covered here: #2570's general recovery owns it.
//! - a fake `amplihack` on `PATH` for `hygiene artifact-guard` and the
//!   hook-repair `recipe run` call (it answers without making a fix).
//!
//! Limit: the fake agent tests the wiring, not the model's judgement.
//! The cases are skipped when `recipe-runner-rs` is not installed, as the
//! #1552 scenario is; CI installs the runner.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value as Json;
use serde_yaml::{Mapping, Value as Yaml};

/// The value the inherited `RECIPE_VAR_issue_number` carries. It must never
/// appear in a recorded result: only variable names may be recorded.
const INHERITED_ISSUE: &str = "4242424242";
const INHERITED_STARTED_AT: &str = "2099-01-01T00:00:00Z";
const BRANCH: &str = "feat/x";

fn repo_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn recipe_yaml(name: &str) -> Yaml {
    let path = repo_root()
        .join("amplifier-bundle/recipes")
        .join(format!("{name}.yaml"));
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn step_by_id(recipe: &Yaml, id: &str) -> Yaml {
    recipe["steps"]
        .as_sequence()
        .expect("recipe has steps")
        .iter()
        .find(|s| s["id"].as_str() == Some(id))
        .cloned()
        .unwrap_or_else(|| panic!("step {id} not found in recipe"))
}

fn step_str<'a>(step: &'a Yaml, key: &str) -> &'a str {
    step[key].as_str().unwrap_or_default()
}

/// `recipe-runner-rs` from `RECIPE_RUNNER_RS_PATH` or `PATH`; `None` skips.
fn runner() -> Option<PathBuf> {
    if let Some(v) = std::env::var_os("RECIPE_RUNNER_RS_PATH") {
        let p = PathBuf::from(v);
        if p.is_file() {
            return Some(p);
        }
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|d| d.join("recipe-runner-rs"))
        .find(|c| c.is_file())
}

macro_rules! runner_or_skip {
    () => {
        match runner() {
            Some(r) => r,
            None => {
                eprintln!(
                    "skipping: recipe-runner-rs not installed (set RECIPE_RUNNER_RS_PATH \
                     or put it on PATH; CI installs it)"
                );
                return;
            }
        }
    };
}

fn make_private_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .unwrap_or_else(|e| panic!("chmod {}: {e}", path.display()));
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

// --- fixture -----------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Upstream {
    /// `origin/feat/x`, one commit behind HEAD.
    Ahead,
    /// `origin/feat/other`: step-15 refuses to push (#1762).
    OtherBranch,
    /// `origin/feat/x` has a commit HEAD lacks (#978), in a file HEAD does
    /// not touch, so a merge is clean.
    Diverged,
    /// `origin/feat/x` has a commit HEAD lacks that adds `change.txt` with
    /// other content: merging it gives an add/add conflict.
    Conflict,
    /// `origin/feat/x` already equals HEAD.
    AtHead,
    /// `origin/feat/x` already has the run's commit and one more on top, so
    /// HEAD is behind but not ahead: a merge is a fast-forward.
    Behind,
    /// `refs/heads/feat/x` does not exist on the remote: a first push. Only
    /// the remote's ref is deleted; the worktree keeps its tracking ref.
    Absent,
}

/// A token-shaped placeholder. It must never appear unredacted in a record.
const FIXTURE_TOKEN: &str = "ghp_TESTTOKEN0000";

struct Fixture {
    temp: tempfile::TempDir,
    runner: PathBuf,
    /// The run's branch: `BRANCH` unless a case needs another name.
    branch: String,
}

struct Run {
    code: i32,
    stderr: String,
    json: Json,
}

impl Run {
    fn result(&self, id: &str) -> Option<&Json> {
        self.json["step_results"]
            .as_array()?
            .iter()
            .find(|r| r["step_id"] == id)
    }
    fn status(&self, id: &str) -> String {
        self.result(id)
            .and_then(|r| r["status"].as_str())
            .unwrap_or("absent")
            .to_string()
    }
    /// A context value; a JSON string holding an object is parsed.
    fn ctx(&self, key: &str) -> Json {
        let v = self.json["context"][key].clone();
        match v.as_str() {
            Some(s) => serde_json::from_str(s).unwrap_or(v),
            None => v,
        }
    }
    fn all_text(&self) -> String {
        let mut t = self.stderr.clone();
        for r in self.json["step_results"].as_array().into_iter().flatten() {
            for k in ["error", "output"] {
                t.push('\n');
                t.push_str(r[k].as_str().unwrap_or_default());
            }
        }
        t
    }
    fn debug(&self) -> String {
        format!(
            "exit={} stderr=\n{}\njson=\n{}",
            self.code,
            self.stderr,
            serde_json::to_string_pretty(&self.json).unwrap_or_default()
        )
    }
}

impl Fixture {
    fn new(upstream: Upstream) -> Self {
        Self::with_branch(upstream, BRANCH)
    }

    fn with_branch(upstream: Upstream, branch: &str) -> Self {
        let runner = runner().expect("caller checked the runner");
        let temp = tempfile::tempdir().expect("tempdir");
        let f = Fixture {
            temp,
            runner,
            branch: branch.to_string(),
        };
        for d in [
            "home",
            "tmp",
            "xdg/config",
            "xdg/cache",
            "xdg/data",
            "xdg/state",
            "work",
            "recipes",
            "bin",
            "prompts",
        ] {
            fs::create_dir_all(f.p(d)).unwrap();
        }
        f.build_repos(upstream);
        f.write_fake_amplihack();
        f.write_fake_launcher();
        f.set_agent_mode("env");
        f
    }

    fn p(&self, rel: &str) -> PathBuf {
        self.temp.path().join(rel)
    }
    fn root(&self) -> String {
        self.temp.path().display().to_string()
    }
    fn wt(&self) -> PathBuf {
        self.p("wt")
    }
    fn remote(&self) -> PathBuf {
        self.p("remote.git")
    }

    fn git_cmd(&self, dir: &Path, args: &[&str]) -> Command {
        let mut cmd = amplihack_git::command();
        cmd.env_clear()
            .current_dir(dir)
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.p("home"))
            .env("TMPDIR", self.p("tmp"))
            .env("XDG_CONFIG_HOME", self.p("xdg/config"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args([
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "user.name=Fixture",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args);
        crate::isolated_amplihack::no_git_prompt(&mut cmd);
        cmd
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let out = self.git_cmd(dir, args).output().expect("run fixture git");
        assert!(
            out.status.success(),
            "fixture git {args:?} in {} failed: {}",
            dir.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Whether a git command succeeds; for checks such as `merge-base
    /// --is-ancestor` whose failure is an answer, not a fixture error.
    fn git_ok(&self, dir: &Path, args: &[&str]) -> bool {
        self.git_cmd(dir, args)
            .output()
            .expect("run fixture git")
            .status
            .success()
    }

    /// Another clone commits `file` with `content` on the run's branch and
    /// pushes it, so the remote has a commit HEAD lacks.
    fn push_other(&self, file: &str, content: &str) {
        let other = self.p("other");
        self.git(
            &self.p("tmp"),
            &[
                "clone",
                "-q",
                &self.remote().display().to_string(),
                &other.display().to_string(),
            ],
        );
        self.git(&other, &["switch", "-q", &self.branch]);
        fs::write(other.join(file), content).unwrap();
        self.git(&other, &["add", file]);
        self.git(&other, &["commit", "-q", "-m", "someone else"]);
        self.git(&other, &["push", "-q", "origin", &self.branch]);
    }

    fn build_repos(&self, upstream: Upstream) {
        let remote = self.remote();
        let src = self.p("src");
        let tmp = self.p("tmp");
        self.git(
            &tmp,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                &remote.display().to_string(),
            ],
        );
        fs::create_dir_all(&src).unwrap();
        self.git(&src, &["init", "-q", "-b", "main"]);
        self.git(
            &src,
            &["remote", "add", "origin", &remote.display().to_string()],
        );
        fs::write(src.join("README.md"), "base\n").unwrap();
        self.git(&src, &["add", "README.md"]);
        self.git(&src, &["commit", "-q", "-m", "base"]);
        self.git(&src, &["push", "-q", "origin", "main"]);
        let br = self.branch.as_str();
        let remote_ref = format!("refs/heads/{br}");
        let tracking = format!("origin/{br}");
        self.git(
            &src,
            &["push", "-q", "origin", &format!("main:{remote_ref}")],
        );
        self.git(
            &src,
            &["push", "-q", "origin", "main:refs/heads/feat/other"],
        );
        self.git(&src, &["fetch", "-q", "origin"]);
        let wt = self.wt().display().to_string();
        self.git(&src, &["worktree", "add", "-q", "-b", br, &wt, &tracking]);
        let up = if upstream == Upstream::OtherBranch {
            "origin/feat/other"
        } else {
            tracking.as_str()
        };
        self.git(&self.wt(), &["branch", "-q", "--set-upstream-to", up]);
        fs::write(self.wt().join("change.txt"), "the run's change\n").unwrap();
        self.git(&self.wt(), &["add", "change.txt"]);
        self.git(&self.wt(), &["commit", "-q", "-m", "the run's change"]);
        match upstream {
            // Someone else pushed to feat/x: HEAD is behind as well as ahead.
            Upstream::Diverged => self.push_other("other.txt", "someone else\n"),
            Upstream::Conflict => self.push_other("change.txt", "someone else's change\n"),
            Upstream::AtHead => {
                self.git(
                    &self.wt(),
                    &["push", "-q", "origin", &format!("HEAD:{remote_ref}")],
                );
                self.git(&self.wt(), &["fetch", "-q", "origin"]);
            }
            Upstream::Behind => {
                self.git(
                    &self.wt(),
                    &["push", "-q", "origin", &format!("HEAD:{remote_ref}")],
                );
                self.push_other("other.txt", "someone else\n");
            }
            Upstream::Absent => {
                self.git(&remote, &["update-ref", "-d", &remote_ref]);
            }
            Upstream::Ahead | Upstream::OtherBranch => {}
        }
        // Installed last, so the fixture's own pushes above are accepted.
        let hook = remote.join("hooks/pre-receive");
        fs::write(
            &hook,
            format!(
                r#"#!/bin/sh
if [ -e {root}/reject-always ]; then
  echo "fixture: the change broke a test" >&2
  exit 1
fi
if [ -n "${{RECIPE_VAR_issue_number+x}}" ]; then
  echo "fixture: an inherited RECIPE_VAR_issue_number reached the hook" >&2
  exit 1
fi
exit 0
"#,
                root = shell_quote(&self.root())
            ),
        )
        .unwrap();
        make_private_executable(&hook);
    }

    fn write_fake_amplihack(&self) {
        let path = self.p("bin/amplihack");
        fs::write(
            &path,
            format!(
                r#"#!/usr/bin/env bash
T={root}
case "${{1:-}}" in
  hygiene)
    if [ -e "$T/fail-artifact-guard" ]; then echo "fixture: artifact guard tool missing" >&2; exit 127; fi
    exit 0 ;;
  recipe)
    printf '%s\n' "$*" >> "$T/amplihack-recipe-calls.log"
    echo "fixture: hook repair answered; no fix made" >&2
    exit 1 ;;
  *) echo "fixture amplihack: unexpected call: $*" >&2; exit 99 ;;
esac
"#,
                root = shell_quote(&self.root())
            ),
        )
        .unwrap();
        make_private_executable(&path);
    }

    /// The fake recovery agent. Modes (file `agent-mode`):
    /// - `env`: clean-environment push, `ENVIRONMENT`;
    /// - `change`: clean-environment push (rejected), `CHANGE`;
    /// - `rogue`: pushes `HEAD:refs/heads/feat/x` and claims `ENVIRONMENT`
    ///   even though the upstream is another branch;
    /// - `claim-env`: makes no push but claims `ENVIRONMENT`;
    /// - `commit-push`: disobeys the prompt, commits a new file and pushes it,
    ///   then claims `ENVIRONMENT`;
    /// - `forged-merge`: disobeys the prompt, builds a two-parent merge of HEAD
    ///   and `origin/<branch>` by hand with `git commit-tree` around a new
    ///   file, resets to it so `git status` is clean, pushes and claims
    ///   `ENVIRONMENT`;
    /// - `reset-to-remote`: disobeys the prompt, fetches and runs `git reset
    ///   --hard origin/<branch>`, dropping the run's commits, then the clean
    ///   push (which has nothing to send) and `ENVIRONMENT`;
    /// - `none`: no push, `NONE`;
    /// - `none-noworktree`: no worktree to work in, `NONE`;
    /// - `force`: disobeys the prompt, force-pushes HEAD over the remote
    ///   branch and claims `ENVIRONMENT`, with a token-shaped explanation;
    /// - `env-untracked`: leaves an untracked file in the worktree, then the
    ///   clean push, `ENVIRONMENT`;
    /// - `repoint`: points `origin` at a decoy bare repository of its own,
    ///   pushes there and claims `ENVIRONMENT`;
    /// - `merge`: as the prompt asks when the remote has commits HEAD lacks,
    ///   fetches and merges `origin/<branch>` with hooks on and no
    ///   `--no-verify`, then the clean push and `ENVIRONMENT`; on a conflict
    ///   it runs `git merge --abort` and returns `CHANGE` without a push.
    fn write_fake_launcher(&self) {
        let path = self.p("bin/fake-launcher");
        fs::write(
            &path,
            format!(
                r#"#!/usr/bin/env bash
set -uo pipefail
T={root}
WT={wt}
BR={br}
case "$0" in "$T"/*) ;; *) echo "fake agent: refusing to run outside the fixture" >&2; exit 97 ;; esac
case "$(pwd -P)/" in "$T"/*|"$(cd "$T" && pwd -P)"/*) ;; *) echo "fake agent: cwd $(pwd -P) is outside the fixture" >&2; exit 97 ;; esac
prompt=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -p|--system-prompt) prompt="$prompt
${{2:-}}"; shift 2 ;;
    --append-system-prompt-file) [ -f "${{2:-}}" ] && prompt="$prompt
$(cat "$2")"; shift 2 ;;
    *) shift ;;
  esac
done
n=$(ls "$T/prompts" | wc -l)
printf '%s' "$prompt" > "$T/prompts/$n.txt"
step=""
for s in step-15-commit-push step-18c-push-feedback-changes; do
  case "$prompt" in *"$s"*) step="$s"; break ;; esac
done
case "$prompt" in *CHANGE*) ;; *) step="" ;; esac
case "$prompt" in *ENVIRONMENT*) ;; *) step="" ;; esac
if [ -z "$step" ]; then echo "fake agent: unexpected prompt" >&2; exit 98; fi
printf '%s\n' "$step" >> "$T/agent-calls.log"
explanation="fixture agent"
mode=$(cat "$T/agent-mode")
names=$(compgen -e | LC_ALL=C grep -E '^(RECIPE_VAR_|WORKFLOW_)' | LC_ALL=C sort | paste -sd' ' -)
unset_args=(); for v in $names; do unset_args+=(-u "$v"); done
branch="$BR"
if [ "$mode" = none-noworktree ]; then
  branch=""
else
  cd "$WT" || {{ echo "fake agent: no worktree" >&2; exit 96; }}
fi
action="env $(printf -- '-u %s ' $names)git push origin HEAD:refs/heads/$BR"
push_clean() {{ env "${{unset_args[@]}}" git push origin "HEAD:refs/heads/$BR" > "$T/agent-push.log" 2>&1; }}
actions='[]'; push_result=not-attempted; evidence_last=""
case "$mode" in
  env)
    if push_clean; then push_result=pushed; evidence_last="the clean push succeeded"; else push_result=failed; evidence_last="the clean push failed"; fi
    cause=ENVIRONMENT; actions=$(jq -nc --arg a "$action" '[$a]') ;;
  change)
    if push_clean; then push_result=pushed; else push_result=failed; fi
    cause=CHANGE; evidence_last="the clean push was rejected by a test the change broke"
    actions=$(jq -nc --arg a "$action" '[$a]') ;;
  rogue)
    if push_clean; then push_result=pushed; else push_result=failed; fi
    cause=ENVIRONMENT; evidence_last="pushed anyway"; actions=$(jq -nc --arg a "$action" '[$a]') ;;
  commit-push)
    printf 'untested\n' > agent-edit.txt
    git add agent-edit.txt && git -c user.email=a@example.invalid -c user.name=A -c commit.gpgsign=false commit -q --no-verify -m untested
    if push_clean; then push_result=pushed; else push_result=failed; fi
    cause=ENVIRONMENT; evidence_last="committed and pushed"; actions=$(jq -nc --arg a "$action" '[$a]') ;;
  forged-merge)
    env "${{unset_args[@]}}" GIT_TERMINAL_PROMPT=0 GCM_INTERACTIVE=never git fetch -q origin > "$T/agent-merge.log" 2>&1
    printf 'forged\n' > agent-edit.txt
    git add agent-edit.txt
    tree=$(git write-tree)
    forged=$(git -c user.email=a@example.invalid -c user.name=A -c commit.gpgsign=false commit-tree "$tree" -p HEAD -p "origin/$BR" -m forged)
    git reset -q --hard "$forged"
    if push_clean; then push_result=pushed; else push_result=failed; fi
    cause=ENVIRONMENT; evidence_last="merged and pushed"; actions=$(jq -nc --arg a "$action" '[$a]') ;;
  reset-to-remote)
    env "${{unset_args[@]}}" GIT_TERMINAL_PROMPT=0 GCM_INTERACTIVE=never git fetch -q origin > "$T/agent-merge.log" 2>&1
    git reset -q --hard "origin/$BR"
    if push_clean; then push_result=pushed; else push_result=failed; fi
    cause=ENVIRONMENT; evidence_last="reset to origin/$BR and pushed"; actions=$(jq -nc --arg a "$action" '[$a]') ;;
  force)
    if env "${{unset_args[@]}}" git push --force origin "HEAD:refs/heads/$BR" > "$T/agent-push.log" 2>&1; then push_result=pushed; else push_result=failed; fi
    cause=ENVIRONMENT; evidence_last="pushed over the remote branch"; actions=$(jq -nc --arg a "$action" '[$a]')
    explanation="pushed via https://u:{token}@example.invalid/x with token {token}" ;;
  env-untracked)
    printf 'agent note\n' > agent-note.txt
    if push_clean; then push_result=pushed; else push_result=failed; fi
    cause=ENVIRONMENT; evidence_last="left a note"; actions=$(jq -nc --arg a "$action" '[$a]') ;;
  repoint)
    git clone -q --bare "$T/remote.git" "$T/decoy.git" > /dev/null 2>&1
    git remote set-url origin "$T/decoy.git"
    if push_clean; then push_result=pushed; else push_result=failed; fi
    cause=ENVIRONMENT; evidence_last="pushed to a remote of its choosing"; actions=$(jq -nc --arg a "$action" '[$a]') ;;
  merge|merge-ours)
    strategy=(); [ "$mode" = merge ] || strategy=(-s ours)
    if env "${{unset_args[@]}}" GIT_TERMINAL_PROMPT=0 GCM_INTERACTIVE=never git fetch -q origin > "$T/agent-merge.log" 2>&1 \
      && env "${{unset_args[@]}}" GIT_TERMINAL_PROMPT=0 GCM_INTERACTIVE=never GIT_EDITOR=true \
           git -c user.email=a@example.invalid -c user.name=A -c commit.gpgsign=false merge --no-edit "${{strategy[@]}}" "origin/$BR" >> "$T/agent-merge.log" 2>&1; then
      if push_clean; then push_result=pushed; else push_result=failed; fi
      cause=ENVIRONMENT; evidence_last="merged origin/$BR at $(git rev-parse HEAD^2) ${{strategy[*]}} and pushed"
      actions=$(jq -nc --arg a "git merge --no-edit ${{strategy[*]}} origin/$BR" --arg b "$action" '[$a, $b]')
    else
      git merge --abort > /dev/null 2>&1
      cause=CHANGE; evidence_last="the merge of origin/$BR conflicted and could not be resolved"
    fi ;;
  claim-env) cause=ENVIRONMENT; evidence_last="claimed without a push" ;;
  none|none-noworktree) cause=NONE; evidence_last="nothing needed pushing" ;;
  *) echo "fake agent: unknown mode $mode" >&2; exit 95 ;;
esac
jq -nc --arg failed "$step: push rejected" --arg cause "$cause" --arg branch "$branch" \
  --arg names "inherited: $names" --arg last "$evidence_last" \
  --argjson actions "$actions" --arg pr "$push_result" --arg explanation "$explanation" \
  '{{failed:$failed, cause:$cause, branch:$branch, evidence:[$names, $last], actions:$actions, push_result:$pr, explanation:$explanation}}'
"#,
                root = shell_quote(&self.root()),
                wt = shell_quote(&self.wt().display().to_string()),
                br = shell_quote(&self.branch),
                token = FIXTURE_TOKEN,
            ),
        )
        .unwrap();
        make_private_executable(&path);
    }

    fn set_agent_mode(&self, mode: &str) {
        fs::write(self.p("agent-mode"), mode).unwrap();
    }
    fn touch(&self, name: &str) {
        fs::write(self.p(name), "").unwrap();
    }
    /// A `pre-commit` hook in the shared hooks directory that rejects every
    /// commit in the worktree.
    fn reject_commits(&self) {
        let hook = self.p("src/.git/hooks/pre-commit");
        fs::write(
            &hook,
            "#!/bin/sh\necho 'fixture: commit rejected' >&2\nexit 1\n",
        )
        .unwrap();
        make_private_executable(&hook);
    }

    fn head(&self) -> String {
        self.git(&self.wt(), &["rev-parse", "HEAD"])
    }
    fn remote_ref(&self, branch: &str) -> String {
        self.git(
            &self.remote(),
            &["rev-parse", &format!("refs/heads/{branch}")],
        )
    }
    fn agent_calls(&self) -> Vec<String> {
        fs::read_to_string(self.p("agent-calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn worktree_setup(&self, path: &Path) -> Yaml {
        let mut m = Mapping::new();
        m.insert("worktree_path".into(), path.display().to_string().into());
        m.insert("branch_name".into(), self.branch.as_str().into());
        m.insert("base_ref".into(), "origin/main".into());
        m.insert("created".into(), true.into());
        Yaml::Mapping(m)
    }

    /// A recipe named `name` with `phase`'s own context map, the fixture's
    /// context on top, and the given steps.
    fn write_recipe(&self, name: &str, phase: &str, steps: Vec<Yaml>, worktree: &Path) -> PathBuf {
        let source = recipe_yaml(phase);
        let mut ctx = source["context"].as_mapping().cloned().unwrap_or_default();
        let mut terminal = Mapping::new();
        terminal.insert("terminal_success".into(), "false".into());
        terminal.insert("should_publish".into(), "true".into());
        terminal.insert("should_finalize".into(), "true".into());
        ctx.insert("terminal_state".into(), Yaml::Mapping(terminal));
        ctx.insert("goal_already_met".into(), "false".into());
        ctx.insert("worktree_setup".into(), self.worktree_setup(worktree));
        ctx.insert("base_branch".into(), "main".into());
        ctx.insert(
            "repo_path".into(),
            self.p("src").display().to_string().into(),
        );
        let mut recipe = Mapping::new();
        recipe.insert("name".into(), name.into());
        recipe.insert("version".into(), "1.0.0".into());
        if let Some(r) = source.get("recursion") {
            recipe.insert("recursion".into(), r.clone());
        }
        recipe.insert("context".into(), Yaml::Mapping(ctx));
        recipe.insert("steps".into(), Yaml::Sequence(steps));
        let path = self.p("recipes").join(format!("{name}.yaml"));
        fs::write(
            &path,
            serde_yaml::to_string(&Yaml::Mapping(recipe)).unwrap(),
        )
        .unwrap();
        path
    }

    /// step-15, its recovery call and a stand-in step-16 with the real
    /// condition, so a test can see whether PR creation would run.
    fn publish_recipe(&self, name: &str) -> PathBuf {
        let publish = recipe_yaml("workflow-publish");
        let step15 = step_by_id(&publish, "step-15-commit-push");
        let recovery = step_by_id(&publish, "step-15-push-recovery");
        let real16 = step_by_id(&publish, "step-16-create-draft-pr");
        let stub16: Yaml = serde_yaml::from_str(&format!(
            "id: step-16-create-draft-pr\ntype: bash\ncondition: {}\ncommand: |\n  printf 'STEP16 reason=%s\\n' \"$(printf '%s' \"${{RECIPE_VAR_commit_result:-}}\" | jq -r '.reason // empty')\"\noutput: step16_marker\n",
            serde_json::to_string(step_str(&real16, "condition")).unwrap()
        ))
        .unwrap();
        self.write_recipe(
            name,
            "workflow-publish",
            vec![step15, recovery, stub16],
            &self.wt(),
        )
    }

    /// step-15's recovery after a stand-in for step-15 that leaves `n`
    /// untracked files in the worktree and fails, so the snapshot sees them.
    /// The real step-15 runs `git add -A`, which would commit them.
    fn untracked_before_recovery_recipe(&self, n: usize) -> PathBuf {
        let publish = recipe_yaml("workflow-publish");
        let recovery = step_by_id(&publish, "step-15-push-recovery");
        let script = format!(
            r#"cd {wt}
for i in $(seq 1 {n}); do printf 'x\n' > "pre-$(printf '%04d' "$i").txt"; done
echo 'fixture: push rejected' >&2
exit 1
"#,
            wt = shell_quote(&self.wt().display().to_string()),
        );
        let mut m = Mapping::new();
        m.insert("id".into(), "step-15-commit-push".into());
        m.insert("type".into(), "bash".into());
        m.insert("continue_on_error".into(), true.into());
        m.insert("command".into(), script.into());
        let stand_in = Yaml::Mapping(m);
        self.write_recipe(
            "fixture-untracked",
            "workflow-publish",
            vec![stand_in, recovery],
            &self.wt(),
        )
    }

    fn pr_review_recipe(&self, worktree: &Path) -> PathBuf {
        let review = recipe_yaml("workflow-pr-review");
        let steps = vec![
            step_by_id(&review, "step-18c-push-feedback-changes"),
            step_by_id(&review, "step-18c-push-recovery"),
        ];
        self.write_recipe("fixture-pr-review", "workflow-pr-review", steps, worktree)
    }

    /// The fixture remote's ref as `git ls-remote` reports it.
    fn ls_remote(&self, branch: &str) -> String {
        let out = self.git(
            &self.p("tmp"),
            &[
                "ls-remote",
                &self.remote().display().to_string(),
                &format!("refs/heads/{branch}"),
            ],
        );
        out.split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// Run `recipe` with the inherited variables a parent recipe step would
    /// leave in the environment, plus `extra_env`.
    fn run(&self, recipe: &Path, sets: &[&str], extra_env: &[(&str, &str)]) -> Run {
        let mut cmd: Command = crate::isolated_amplihack::isolated(&self.runner);
        let path = format!(
            "{}:{}",
            self.p("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        cmd.env("HOME", self.p("home"))
            .env("TMPDIR", self.p("tmp"))
            .env("XDG_CONFIG_HOME", self.p("xdg/config"))
            .env("XDG_CACHE_HOME", self.p("xdg/cache"))
            .env("XDG_DATA_HOME", self.p("xdg/data"))
            .env("XDG_STATE_HOME", self.p("xdg/state"))
            .env("CLAUDE_CONFIG_DIR", self.p("home/.claude"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("PATH", path)
            .env("AMPLIHACK_HOME", repo_root())
            .env("AMPLIHACK_LAUNCHER_BINARY", self.p("bin/fake-launcher"))
            .env("RECIPE_RUNNER_NO_UPDATE_CHECK", "1")
            // What a parent run's step leaves behind (#2471).
            .env("RECIPE_VAR_issue_number", INHERITED_ISSUE)
            .env("WORKFLOW_STARTED_AT", INHERITED_STARTED_AT)
            .current_dir(self.p("work"))
            .arg(recipe)
            .args(["--output-format", "json", "-C"])
            .arg(self.p("work"))
            .arg("-R")
            .arg(self.p("recipes"));
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        for s in sets {
            cmd.args(["-c", s]);
        }
        let out = cmd.output().expect("spawn recipe-runner-rs");
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        let json = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "runner stdout is not JSON ({e}); stdout:\n{}\nstderr:\n{stderr}",
                String::from_utf8_lossy(&out.stdout)
            )
        });
        Run {
            code: out.status.code().unwrap_or(-1),
            stderr,
            json,
        }
    }
}

/// Repository state that no case may change: the checkout's status and the
/// fixture remote's `main`.
struct Untouched {
    workspace_status: String,
    remote_main: String,
}

fn workspace_status() -> String {
    let out = amplihack_git::command()
        .current_dir(repo_root())
        .args(["status", "--porcelain"])
        .output()
        .expect("git status of the checkout");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

impl Untouched {
    fn capture(f: &Fixture) -> Self {
        Untouched {
            workspace_status: workspace_status(),
            remote_main: f.remote_ref("main"),
        }
    }
    fn assert_held(&self, f: &Fixture, run: &Run) {
        assert_eq!(
            workspace_status(),
            self.workspace_status,
            "the run changed the checkout's git status"
        );
        assert_eq!(
            f.remote_ref("main"),
            self.remote_main,
            "the run moved the remote main"
        );
        for value in [INHERITED_ISSUE, INHERITED_STARTED_AT] {
            let recorded = [
                "commit_result",
                "push_recovery_snapshot",
                "push_recovery",
                "push_recovery_result",
            ]
            .iter()
            .map(|k| run.ctx(k).to_string())
            .collect::<Vec<_>>()
            .join("\n");
            assert!(
                !recorded.contains(value),
                "a recorded result holds the value of an inherited variable ({value}): {recorded}"
            );
        }
    }
}

fn assert_failed_with(run: &Run, reason: &str) {
    assert_ne!(run.code, 0, "the run must fail; {}", run.debug());
    let text = run.all_text();
    assert!(
        text.contains(&format!("push-failure-recovery: {reason}")),
        "the failure must name gate reason `{reason}`; {}",
        run.debug()
    );
}

fn assert_recovered(run: &Run, key: &str, reason: &str, head: &str) {
    let rec = run.ctx(key);
    assert_eq!(rec["pushed"], "true", "{key}.pushed; {}", run.debug());
    assert_eq!(rec["reason"], reason, "{key}.reason; {}", run.debug());
    assert_eq!(rec["sha"], head, "{key}.sha must be the gate's own HEAD");
    assert_eq!(rec["branch"], BRANCH, "{key}.branch");
    assert_eq!(
        rec["recovery_attempted"], "true",
        "{key}.recovery_attempted"
    );
}

/// The last recovery call's snapshot, taken before its agent ran, from the
/// runner's JSON context.
fn snapshot(run: &Run) -> Json {
    let snap = run.ctx("push_recovery_snapshot");
    assert!(
        snap.is_object(),
        "no push_recovery_snapshot in the context; {}",
        run.debug()
    );
    snap
}

/// A top-level field of a gate record, as a string.
fn rec_top(run: &Run, key: &str, field: &str) -> String {
    let rec = run.ctx(key);
    rec[field]
        .as_str()
        .unwrap_or_else(|| panic!("{key}.{field} is missing; {}", run.debug()))
        .to_string()
}

/// The stderr lines on which the gate printed its recovery record.
fn recovery_record_lines(run: &Run) -> String {
    let lines: Vec<&str> = run
        .stderr
        .lines()
        .filter(|l| l.contains("push-failure-recovery: recovery record: "))
        .collect();
    assert!(
        !lines.is_empty(),
        "no `push-failure-recovery: recovery record:` line on stderr; {}",
        run.debug()
    );
    lines.join("\n")
}

/// The commands after `To repair without rewriting history: ` on the gate's
/// `remote-history-rewritten` line, and that whole line.
fn repair_line(run: &Run) -> (String, String) {
    const MARK: &str = "To repair without rewriting history: ";
    let text = run.all_text();
    let line = text
        .lines()
        .find(|l| l.contains("remote-history-rewritten") && l.contains(MARK))
        .unwrap_or_else(|| panic!("no repair commands on the failure line; {}", run.debug()))
        .to_string();
    let cmd = line.split(MARK).nth(1).unwrap().trim_end().to_string();
    (line, cmd)
}

/// N2: the gate never prints a force push, of any spelling, as a remedy.
fn assert_no_force_remedy(line: &str) {
    for bad in ["--force", "force-with-lease", " -f "] {
        assert!(
            !line.contains(bad),
            "the repair line must not offer `{bad}`: {line}"
        );
    }
    assert!(
        !line.split_whitespace().any(|w| w.starts_with('+')),
        "the repair line must not offer a `+` refspec: {line}"
    );
}

/// The two halves of the repair commands, split on the non-shell separator
/// ` then: ` and each read back by bash as words, in `scratch`. Reading a
/// half evaluates only `set -- <half>`, so nothing in it runs.
fn repair_words(cmd: &str, scratch: &Path) -> Vec<Vec<String>> {
    let halves: Vec<&str> = cmd.split(" then: ").collect();
    assert_eq!(halves.len(), 2, "two commands joined by ` then: `: {cmd}");
    halves
        .iter()
        .map(|half| {
            let out = Command::new("bash")
                .current_dir(scratch)
                .args(["-c", r#"eval "set -- $1"; printf '%s\0' "$@""#, "_", half])
                .output()
                .expect("bash");
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout)
                .split('\0')
                .filter(|w| !w.is_empty())
                .map(str::to_string)
                .collect()
        })
        .collect()
}

/// N1: HEAD is a merge commit with two parents that contains both the
/// remote's commit `rb` and the run's own commit `run_head`.
fn assert_merged(f: &Fixture, rb: &str, run_head: &str) {
    let head = f.head();
    let parents = f.git(&f.wt(), &["rev-list", "--parents", "-n1", "HEAD"]);
    assert_eq!(
        parents.split_whitespace().count(),
        3,
        "HEAD must be a merge with two parents: {parents}"
    );
    for (name, sha) in [("the remote's commit", rb), ("the run's commit", run_head)] {
        assert!(
            f.git_ok(&f.wt(), &["merge-base", "--is-ancestor", sha, &head]),
            "{name} {sha} must be an ancestor of HEAD {head}"
        );
    }
    assert_eq!(
        fs::read_to_string(f.wt().join("other.txt")).unwrap_or_default(),
        "someone else\n",
        "the remote's file must be in the merged tree"
    );
}

// --- step-15 -------------------------------------------------------------------

/// The positive case the issue asks for: the push fails only because an
/// inherited RECIPE_VAR_issue_number reaches the remote hook. Recovery pushes
/// from a clean environment, the gate confirms the remote ref, and the record
/// says what failed, the cause and what was done.
#[test]
fn step15_environment_fault_is_recovered_and_recorded() {
    let _ = runner_or_skip!();
    let f = Fixture::new(Upstream::Ahead);
    let before = Untouched::capture(&f);
    let base_ref = f.remote_ref(BRANCH);
    let run = f.run(&f.publish_recipe("fixture-publish"), &[], &[]);

    assert_eq!(
        run.code,
        0,
        "recovery must let the run go on; {}",
        run.debug()
    );
    assert_eq!(
        run.status("step-15-commit-push"),
        "failed",
        "the script itself must fail first"
    );
    assert_ne!(base_ref, f.head());
    assert_eq!(
        f.remote_ref(BRANCH),
        f.head(),
        "the remote branch must equal HEAD"
    );
    assert_eq!(
        f.agent_calls(),
        vec!["step-15-commit-push"],
        "one recovery agent call"
    );

    let head = f.head();
    assert_recovered(&run, "commit_result", "recovered-environment", &head);
    let rec = &run.ctx("commit_result")["recovery"];
    assert_eq!(rec["cause"], "ENVIRONMENT");
    assert_eq!(rec["push_result"], "pushed");
    assert!(
        rec["evidence"]
            .to_string()
            .contains("RECIPE_VAR_issue_number"),
        "the record must name the inherited variable: {rec}"
    );
    let action = rec["actions"][0].as_str().unwrap_or_default();
    assert!(
        action.starts_with("env ")
            && action.contains("-u RECIPE_VAR_issue_number")
            && action.ends_with("git push origin HEAD:refs/heads/feat/x"),
        "the record must list the clean push as the action: {action}"
    );
    assert_eq!(
        run.status("step-16-create-draft-pr"),
        "completed",
        "step-16 must run on the gate's commit_result; {}",
        run.debug()
    );

    // B2: the remote branch as it was before the agent ran, from a fetch.
    assert_eq!(rec_top(&run, "commit_result", "remote_before"), base_ref);
    assert_eq!(
        rec_top(&run, "commit_result", "remote_before_source"),
        "fetched"
    );
    let snap = snapshot(&run);
    assert_eq!(snap["remote_before"], base_ref.as_str(), "snapshot: {snap}");
    assert_eq!(snap["remote_before_source"], "fetched", "snapshot: {snap}");
    before.assert_held(&f, &run);
}

/// The hook rejects every push and the agent judges the change to blame: the
/// run ends, says the cause is in the change, and the remote does not move.
#[test]
fn step15_change_fault_ends_the_run_and_says_so() {
    let _ = runner_or_skip!();
    let f = Fixture::new(Upstream::Ahead);
    f.touch("reject-always");
    f.set_agent_mode("change");
    let before = Untouched::capture(&f);
    let base_ref = f.remote_ref(BRANCH);
    let run = f.run(&f.publish_recipe("fixture-publish"), &[], &[]);

    assert_failed_with(&run, "cause-in-change");
    assert!(
        run.all_text().contains("cause is in the change"),
        "{}",
        run.debug()
    );
    // Minor 2: the gate leaves its recovery record on stderr when it fails.
    let record = recovery_record_lines(&run);
    assert!(
        record.contains("\"explanation\"") && record.contains("\"evidence\""),
        "stderr must hold the recovery record with explanation and evidence; {}",
        run.debug()
    );
    assert_eq!(
        f.remote_ref(BRANCH),
        base_ref,
        "the remote branch must not move"
    );
    assert_ne!(run.status("step-16-create-draft-pr"), "completed");
    before.assert_held(&f, &run);
}

/// The upstream is another branch, so step-15 refuses (#1762). An agent that
/// pushes to refs/heads/feat/x anyway and claims ENVIRONMENT is caught by the
/// gate's own upstream check, and PR creation does not run.
#[test]
fn step15_upstream_on_another_branch_is_refused_even_if_the_agent_pushes() {
    let _ = runner_or_skip!();
    let f = Fixture::new(Upstream::OtherBranch);
    f.set_agent_mode("rogue");
    let before = Untouched::capture(&f);
    let run = f.run(&f.publish_recipe("fixture-publish"), &[], &[]);

    assert_failed_with(&run, "upstream-mismatch");
    assert_ne!(run.status("step-16-create-draft-pr"), "completed");
    before.assert_held(&f, &run);
}

/// N1: the remote branch has a commit HEAD lacks (#978). The agent fetches,
/// merges origin/feat/x with hooks on and pushes with a plain push. The gate
/// accepts a HEAD whose only new commit is that merge, and PR creation runs.
#[test]
fn step15_diverged_upstream_is_merged_and_pushed() {
    let _ = runner_or_skip!();
    let f = Fixture::new(Upstream::Diverged);
    f.set_agent_mode("merge");
    let before = Untouched::capture(&f);
    let remote_before = f.remote_ref(BRANCH);
    let run_head = f.head();
    let run = f.run(&f.publish_recipe("fixture-publish"), &[], &[]);

    assert_eq!(
        run.code,
        0,
        "a diverged branch is merged, not stopped; {}",
        run.debug()
    );
    assert_eq!(
        f.agent_calls(),
        vec!["step-15-commit-push"],
        "one recovery agent call"
    );
    let head = f.head();
    assert_ne!(head, run_head, "the agent's merge moved HEAD");
    assert_merged(&f, &remote_before, &run_head);
    assert_recovered(&run, "commit_result", "recovered-environment", &head);
    assert_eq!(f.ls_remote(BRANCH), head, "the plain push reached origin");
    assert_eq!(
        snapshot(&run)["head"],
        run_head.as_str(),
        "the snapshot was taken before the merge"
    );
    assert_eq!(
        run.status("step-16-create-draft-pr"),
        "completed",
        "step-16 must run on the gate's commit_result; {}",
        run.debug()
    );
    before.assert_held(&f, &run);
}

/// N1: the agent judges the run's tree is meant to win and merges with
/// `-s ours`. The remote commit stays in history, HEAD keeps the run's tree,
/// and the gate accepts the merge and the plain push.
#[test]
fn step15_diverged_upstream_merged_with_ours_strategy_keeps_local_tree() {
    let _ = runner_or_skip!();
    let f = Fixture::new(Upstream::Diverged);
    f.set_agent_mode("merge-ours");
    let before = Untouched::capture(&f);
    let remote_before = f.remote_ref(BRANCH);
    let run_head = f.head();
    let run = f.run(&f.publish_recipe("fixture-publish"), &[], &[]);

    assert_eq!(run.code, 0, "an -s ours merge is pushed; {}", run.debug());
    let head = f.head();
    assert_eq!(
        f.git(&f.wt(), &["rev-list", "--parents", "-n1", "HEAD"]),
        format!("{head} {run_head} {remote_before}"),
        "HEAD is one merge of the run's commit and the remote's commit"
    );
    assert_eq!(
        f.git(&f.wt(), &["rev-parse", &format!("{head}^{{tree}}")]),
        f.git(&f.wt(), &["rev-parse", &format!("{run_head}^{{tree}}")]),
        "-s ours keeps the run's tree"
    );
    assert_recovered(&run, "commit_result", "recovered-environment", &head);
    assert_eq!(f.ls_remote(BRANCH), head, "the plain push reached origin");
    before.assert_held(&f, &run);
}

/// N1: the remote's commit conflicts with the run's. The agent cannot resolve
/// it, aborts the merge and answers CHANGE: the run ends, nothing is pushed
/// and no merge is left half done. The gate reason is not pinned: which rule
/// fires first is #2606's (m3).
#[test]
fn step15_unresolvable_merge_conflict_aborts_and_ends_the_run() {
    let _ = runner_or_skip!();
    let f = Fixture::new(Upstream::Conflict);
    f.set_agent_mode("merge");
    let before = Untouched::capture(&f);
    let remote_before = f.ls_remote(BRANCH);
    let run_head = f.head();
    let run = f.run(&f.publish_recipe("fixture-publish"), &[], &[]);

    assert_ne!(run.code, 0, "the run must fail; {}", run.debug());
    assert_eq!(
        f.agent_calls(),
        vec!["step-15-commit-push"],
        "{}",
        run.debug()
    );
    assert!(
        recovery_record_lines(&run).contains("\"cause\":\"CHANGE\""),
        "the record must hold the agent's CHANGE; {}",
        run.debug()
    );
    assert_eq!(f.ls_remote(BRANCH), remote_before, "nothing is pushed");
    assert_eq!(f.head(), run_head, "the aborted merge leaves HEAD alone");
    let merge_head = f.git(&f.wt(), &["rev-parse", "--git-path", "MERGE_HEAD"]);
    assert!(
        !f.wt().join(&merge_head).exists(),
        "no MERGE_HEAD may be left behind: {merge_head}"
    );
    assert_ne!(run.status("step-16-create-draft-pr"), "completed");
    before.assert_held(&f, &run);
}

/// step-15 fails before its push (a missing tool) while the remote already
/// has HEAD. NONE with a verified ref gives pushed "true" and step-16 runs.
#[test]
fn step15_none_with_branch_at_head_continues_to_pr_creation() {
    let _ = runner_or_skip!();
    let f = Fixture::new(Upstream::AtHead);
    f.touch("fail-artifact-guard");
    f.set_agent_mode("none");
    let before = Untouched::capture(&f);
    let run = f.run(&f.publish_recipe("fixture-publish"), &[], &[]);

    assert_eq!(run.code, 0, "{}", run.debug());
    assert_eq!(run.status("step-15-commit-push"), "failed");
    let head = f.head();
    assert_recovered(&run, "commit_result", "already-at-head", &head);
    assert_eq!(
        run.status("step-16-create-draft-pr"),
        "completed",
        "{}",
        run.debug()
    );
    before.assert_held(&f, &run);
}

/// A parent run's RECIPE_VAR_commit_result of {"pushed":"true"} in the
/// process environment must not make recovery skip.
#[test]
fn step15_stale_commit_result_in_the_environment_does_not_skip_recovery() {
    let _ = runner_or_skip!();
    let f = Fixture::new(Upstream::Ahead);
    let before = Untouched::capture(&f);
    let stale = r#"{"pushed":"true","sha":"","branch":"feat/x","reason":"pushed"}"#;
    let run = f.run(
        &f.publish_recipe("fixture-publish"),
        &[],
        &[("RECIPE_VAR_commit_result", stale)],
    );

    assert_eq!(run.code, 0, "{}", run.debug());
    assert_eq!(
        f.agent_calls(),
        vec!["step-15-commit-push"],
        "recovery must run"
    );
    assert_eq!(f.remote_ref(BRANCH), f.head());
    let head = f.head();
    assert_recovered(&run, "commit_result", "recovered-environment", &head);
    before.assert_held(&f, &run);
}

/// A caller context with commit_result {"pushed":"true"} must not make
/// recovery skip: the real default-workflow call step for workflow-publish
/// resets the key. The call step is taken by id, pointed at the fixture's
/// publish recipe.
#[test]
fn step15_stale_commit_result_in_the_caller_context_does_not_skip_recovery() {
    let _ = runner_or_skip!();
    let f = Fixture::new(Upstream::Ahead);
    let before = Untouched::capture(&f);
    f.publish_recipe("fixture-publish-child");
    let mut call = step_by_id(&recipe_yaml("default-workflow"), "workflow-publish");
    call.as_mapping_mut()
        .unwrap()
        .insert("recipe".into(), "fixture-publish-child".into());
    let mut ctx = Mapping::new();
    ctx.insert("commit_result".into(), "".into());
    let mut parent = Mapping::new();
    parent.insert("name".into(), "fixture-parent".into());
    parent.insert("version".into(), "1.0.0".into());
    parent.insert("context".into(), Yaml::Mapping(ctx));
    parent.insert("steps".into(), Yaml::Sequence(vec![call]));
    let parent_path = f.p("recipes/fixture-parent.yaml");
    fs::write(
        &parent_path,
        serde_yaml::to_string(&Yaml::Mapping(parent)).unwrap(),
    )
    .unwrap();

    let run = f.run(
        &parent_path,
        &[r#"commit_result={"pushed":"true","sha":"","branch":"feat/x","reason":"pushed"}"#],
        &[],
    );

    assert_eq!(
        f.agent_calls(),
        vec!["step-15-commit-push"],
        "recovery must run; {}",
        run.debug()
    );
    assert_eq!(run.code, 0, "{}", run.debug());
    assert_eq!(
        f.remote_ref(BRANCH),
        f.head(),
        "the clean push must reach the remote"
    );
    before.assert_held(&f, &run);
}

/// step-15's commit fails, so the change stays staged and HEAD is the older
/// commit, which origin already has. An agent that answers NONE must not turn
/// that into a push: the gate sees tracked changes the snapshot recorded
/// before the agent ran, and PR creation does not run.
#[test]
fn step15_failed_commit_is_not_reported_as_pushed() {
    let _ = runner_or_skip!();
    let f = Fixture::new(Upstream::AtHead);
    f.reject_commits();
    fs::write(f.wt().join("README.md"), "the uncommitted change\n").unwrap();
    f.set_agent_mode("none");
    let before = Untouched::capture(&f);
    let run = f.run(&f.publish_recipe("fixture-publish"), &[], &[]);

    assert_eq!(
        run.status("step-15-commit-push"),
        "failed",
        "{}",
        run.debug()
    );
    assert_eq!(f.agent_calls(), vec!["step-15-commit-push"]);
    assert_failed_with(&run, "uncommitted-changes");
    assert_ne!(run.status("step-16-create-draft-pr"), "completed");
    before.assert_held(&f, &run);
}

/// An agent that commits and pushes a commit the run never tested is caught
/// by the snapshot's HEAD, even though origin equals the new HEAD.
#[test]
fn step15_agent_commit_is_refused_as_worktree_changed() {
    let _ = runner_or_skip!();
    let f = Fixture::new(Upstream::Ahead);
    f.set_agent_mode("commit-push");
    let before = Untouched::capture(&f);
    let run = f.run(&f.publish_recipe("fixture-publish"), &[], &[]);

    assert_eq!(
        f.agent_calls(),
        vec!["step-15-commit-push"],
        "{}",
        run.debug()
    );
    assert_failed_with(&run, "worktree-changed");
    assert_ne!(run.status("step-16-create-draft-pr"), "completed");
    before.assert_held(&f, &run);
}

/// An agent that builds a two-parent merge of HEAD and `origin/<branch>` by
/// hand, when the remote branch is already in HEAD and there is nothing to
/// merge, is refused. Such a merge passes an ancestry-only check and adds no
/// plain commit, so only rule 9's shape check catches it; that is why this
/// test pins `worktree-changed`.
#[test]
fn step15_hand_built_merge_with_nothing_to_merge_is_refused() {
    let _ = runner_or_skip!();
    let f = Fixture::new(Upstream::Ahead);
    f.set_agent_mode("forged-merge");
    let before = Untouched::capture(&f);
    let run = f.run(&f.publish_recipe("fixture-publish"), &[], &[]);

    assert_eq!(
        f.agent_calls(),
        vec!["step-15-commit-push"],
        "{}",
        run.debug()
    );
    // The fixture really built the merge: HEAD has two parents, and the
    // second was already an ancestor of the first.
    let wt = f.wt();
    let p1 = f.git(&wt, &["rev-parse", "HEAD^1"]);
    let p2 = f.git(&wt, &["rev-parse", "HEAD^2"]);
    assert!(
        f.git_ok(&wt, &["merge-base", "--is-ancestor", &p2, &p1]),
        "{}",
        run.debug()
    );
    assert_failed_with(&run, "worktree-changed");
    assert_ne!(run.status("step-16-create-draft-pr"), "completed");
    before.assert_held(&f, &run);
}

/// An agent that resets the branch to `origin/<branch>`, dropping the run's
/// tested commit, is refused. HEAD equals the remote, `git status` is clean
/// and the push has nothing to send, so only rule 9's fast-forward check
/// (the snapshot HEAD must be an ancestor of the remote) catches it; that is
/// why this test pins `worktree-changed`.
#[test]
fn step15_reset_to_remote_dropping_the_run_commit_is_refused() {
    let _ = runner_or_skip!();
    let f = Fixture::new(Upstream::Diverged);
    f.set_agent_mode("reset-to-remote");
    let before = Untouched::capture(&f);
    let remote_before = f.ls_remote(BRANCH);
    let run_head = f.head();
    let run = f.run(&f.publish_recipe("fixture-publish"), &[], &[]);

    assert_eq!(
        f.agent_calls(),
        vec!["step-15-commit-push"],
        "{}",
        run.debug()
    );
    // The fixture really dropped the run's commit: HEAD is the remote's
    // commit, which does not contain it.
    let wt = f.wt();
    assert_eq!(f.head(), remote_before, "{}", run.debug());
    assert!(
        !f.git_ok(&wt, &["merge-base", "--is-ancestor", &run_head, "HEAD"]),
        "{}",
        run.debug()
    );
    assert_failed_with(&run, "worktree-changed");
    assert_eq!(
        f.ls_remote(BRANCH),
        remote_before,
        "the remote is unchanged"
    );
    assert_ne!(run.status("step-16-create-draft-pr"), "completed");
    before.assert_held(&f, &run);
}

/// The remote already holds the run's commit with one more on top. step-15
/// would call that already pushed, so it is made to fail before its push (a
/// missing tool) and recovery runs. The agent's merge is a fast-forward, so
/// HEAD becomes the remote's commit and the push has nothing to send. Rule 9 accepts it because the result equals
/// `remote_before` and the snapshot HEAD is its ancestor.
#[test]
fn step15_fast_forward_to_remote_is_accepted() {
    let _ = runner_or_skip!();
    let f = Fixture::new(Upstream::Behind);
    f.touch("fail-artifact-guard");
    f.set_agent_mode("merge");
    let before = Untouched::capture(&f);
    let remote_before = f.ls_remote(BRANCH);
    let run_head = f.head();
    assert_ne!(remote_before, run_head, "the fixture leaves HEAD behind");
    let run = f.run(&f.publish_recipe("fixture-publish"), &[], &[]);

    assert_eq!(run.code, 0, "a fast-forward is accepted; {}", run.debug());
    assert_eq!(
        f.agent_calls(),
        vec!["step-15-commit-push"],
        "one recovery agent call"
    );
    let head = f.head();
    assert_eq!(head, remote_before, "HEAD fast-forwarded to the remote");
    assert!(
        f.git_ok(&f.wt(), &["merge-base", "--is-ancestor", &run_head, "HEAD"]),
        "the run's commit is still in HEAD"
    );
    assert_eq!(
        snapshot(&run)["remote_before"],
        remote_before.as_str(),
        "the snapshot recorded the remote before the agent ran"
    );
    assert_recovered(&run, "commit_result", "recovered-environment", &head);
    assert_eq!(f.ls_remote(BRANCH), head);
    assert_eq!(
        run.status("step-16-create-draft-pr"),
        "completed",
        "{}",
        run.debug()
    );
    before.assert_held(&f, &run);
}

/// step-15 refuses to commit on a protected local branch. An agent that
/// pushes HEAD to the task branch anyway is refused by the gate's own
/// local-branch check.
#[test]
fn step15_protected_local_branch_is_refused_even_if_the_agent_pushes() {
    let _ = runner_or_skip!();
    let f = Fixture::new(Upstream::Ahead);
    f.git(&f.wt(), &["switch", "-q", "-c", "master"]);
    f.set_agent_mode("rogue");
    let before = Untouched::capture(&f);
    let run = f.run(&f.publish_recipe("fixture-publish"), &[], &[]);

    assert_eq!(
        run.status("step-15-commit-push"),
        "failed",
        "{}",
        run.debug()
    );
    assert_failed_with(&run, "unsafe-ref");
    assert_ne!(run.status("step-16-create-draft-pr"), "completed");
    before.assert_held(&f, &run);
}

/// B2: the remote branch has someone else's commit, and the agent
/// force-pushes HEAD over it. The remote now equals HEAD, so every other check
/// passes; the gate's ancestry check against the snapshot's remote_before
/// fails the run, names both commits and prints a repair that does not
/// rewrite history (N2): merge the lost commit back, then a plain push. The
/// agent's token-shaped explanation reaches the record only in redacted form.
#[test]
fn step15_force_push_over_another_commit_fails_closed() {
    let _ = runner_or_skip!();
    let f = Fixture::new(Upstream::Diverged);
    f.set_agent_mode("force");
    let before = Untouched::capture(&f);
    let overwritten = f.remote_ref(BRANCH);
    let run = f.run(&f.publish_recipe("fixture-publish"), &[], &[]);
    let head = f.head();

    assert_ne!(overwritten, head);
    assert_eq!(
        f.agent_calls(),
        vec!["step-15-commit-push"],
        "{}",
        run.debug()
    );
    assert_failed_with(&run, "remote-history-rewritten");
    assert_eq!(
        run.status("step-15-push-recovery"),
        "failed",
        "{}",
        run.debug()
    );
    let text = run.all_text();
    assert!(
        text.contains(&overwritten) && text.contains(&head),
        "the failure must name the overwritten commit and HEAD; {}",
        run.debug()
    );
    let (line, cmd) = repair_line(&run);
    let halves: Vec<&str> = cmd.split(" then: ").collect();
    assert_eq!(
        halves,
        vec![
            format!("git merge {overwritten}").as_str(),
            format!("git push origin HEAD:refs/heads/{BRANCH}").as_str(),
        ],
        "the repair is a merge of the lost commit and a plain push: {line}"
    );
    assert_no_force_remedy(&line);
    assert_eq!(
        f.ls_remote(BRANCH),
        head,
        "the gate detects the force-push; it does not undo it"
    );
    assert_ne!(run.status("step-16-create-draft-pr"), "completed");

    // S11: the record shows the explanation redacted, never the token.
    let record = recovery_record_lines(&run);
    assert!(
        !record.contains(FIXTURE_TOKEN) && record.contains("***"),
        "the stderr record must hold the explanation only in redacted form: {record}"
    );
    before.assert_held(&f, &run);
}

/// S-B: the snapshot's untracked list stops at 200 paths. Paths past the cap
/// count as new, so the count is an upper bound, and the record says the list
/// was cut.
#[test]
fn step15_untracked_count_past_snapshot_cap_is_flagged() {
    let _ = runner_or_skip!();
    let f = Fixture::new(Upstream::Ahead);
    f.set_agent_mode("env-untracked");
    let run = f.run(&f.untracked_before_recovery_recipe(250), &[], &[]);

    assert_eq!(run.code, 0, "{}", run.debug());
    let head = f.head();
    assert_recovered(&run, "commit_result", "recovered-environment", &head);
    let snap = snapshot(&run);
    assert_eq!(
        snap["untracked"].as_array().map(Vec::len),
        Some(200),
        "snapshot: {snap}"
    );
    assert_eq!(snap["untracked_truncated"], "true", "snapshot: {snap}");
    let rec = run.ctx("commit_result");
    assert_eq!(rec["untracked_truncated"], "true", "{}", run.debug());
    // 50 paths past the cap plus the agent's note.
    assert_eq!(rec["new_untracked_count"], 51, "{}", run.debug());
}

/// S-A: git allows shell metacharacters in branch names, so the repair
/// commands the gate prints quote each word. Each half, read back by bash on
/// its own, gives the intended arguments and runs nothing; neither half is a
/// force push.
#[test]
fn step15_repair_commands_quote_branch_with_shell_metacharacters() {
    let _ = runner_or_skip!();
    let branch = "feat/x;$(touch>pwned)`touch>pwned`|a&b>pwned";
    let f = Fixture::with_branch(Upstream::Diverged, branch);
    f.set_agent_mode("force");
    let overwritten = f.remote_ref(branch);
    let run = f.run(&f.publish_recipe("fixture-publish"), &[], &[]);
    let head = f.head();

    assert_failed_with(&run, "remote-history-rewritten");
    let (line, cmd) = repair_line(&run);
    let scratch = f.p("tmp/parse");
    fs::create_dir_all(&scratch).unwrap();
    let words = repair_words(&cmd, &scratch);
    assert_eq!(
        words,
        vec![
            vec!["git".to_string(), "merge".to_string(), overwritten.clone()],
            vec![
                "git".to_string(),
                "push".to_string(),
                "origin".to_string(),
                format!("HEAD:refs/heads/{branch}"),
            ],
        ],
        "repair commands: {cmd}"
    );
    assert!(
        fs::read_dir(&scratch).unwrap().next().is_none(),
        "reading the repair commands must run nothing"
    );
    assert_no_force_remedy(&line);
    assert_eq!(
        f.ls_remote(branch),
        head,
        "the gate does not undo the force-push"
    );
}

/// D3: a first push. The remote has no feat/x, so the snapshot records
/// remote_before "absent" (ls-remote exit code 2), the ancestry check has
/// nothing to compare, and the clean push is recovered.
#[test]
fn step15_first_push_with_absent_remote_is_recovered() {
    let _ = runner_or_skip!();
    let f = Fixture::new(Upstream::Absent);
    let before = Untouched::capture(&f);
    assert!(
        f.ls_remote(BRANCH).is_empty(),
        "fixture: feat/x must be absent"
    );
    let run = f.run(&f.publish_recipe("fixture-publish"), &[], &[]);

    assert_eq!(run.code, 0, "{}", run.debug());
    assert_eq!(run.status("step-15-commit-push"), "failed");
    let head = f.head();
    assert_recovered(&run, "commit_result", "recovered-environment", &head);
    assert_eq!(
        f.remote_ref(BRANCH),
        head,
        "origin must be at HEAD afterwards"
    );
    let snap = snapshot(&run);
    assert_eq!(snap["remote_before"], "absent", "snapshot: {snap}");
    assert_eq!(snap["remote_before_source"], "absent", "snapshot: {snap}");
    assert_eq!(rec_top(&run, "commit_result", "remote_before"), "absent");
    before.assert_held(&f, &run);
}

/// Minor 1: an untracked file the agent leaves behind is recorded, not a
/// failure. The snapshot's status ignores untracked files; the record counts
/// the new path in new_untracked_count and never names it.
#[test]
fn step15_untracked_file_left_by_agent_is_recovered_and_recorded() {
    let _ = runner_or_skip!();
    let f = Fixture::new(Upstream::Ahead);
    f.set_agent_mode("env-untracked");
    let before = Untouched::capture(&f);
    let run = f.run(&f.publish_recipe("fixture-publish"), &[], &[]);

    assert_eq!(run.code, 0, "{}", run.debug());
    let head = f.head();
    assert_recovered(&run, "commit_result", "recovered-environment", &head);
    assert_eq!(f.remote_ref(BRANCH), head);
    let rec = run.ctx("commit_result");
    assert_eq!(rec["new_untracked_count"], 1, "{}", run.debug());
    assert_eq!(rec["untracked_truncated"], "false", "{}", run.debug());
    assert!(
        rec.get("new_untracked").is_none() && !rec.to_string().contains("agent-note.txt"),
        "the record must not name the agent's files: {rec}"
    );
    let snap = snapshot(&run);
    assert_eq!(snap["untracked"], serde_json::json!([]), "snapshot: {snap}");
    assert!(
        snap["untracked_truncated"] == "false" || snap["untracked_truncated"] == false,
        "snapshot.untracked_truncated: {snap}"
    );
    assert_eq!(
        run.status("step-16-create-draft-pr"),
        "completed",
        "{}",
        run.debug()
    );
    before.assert_held(&f, &run);
}

/// S8: an agent that points origin at a remote of its own and pushes there
/// would pass every remote check against that remote. The gate compares the
/// origin URL hashes with the snapshot and refuses; the real remote does not
/// move.
#[test]
fn step15_agent_repointing_origin_is_refused() {
    let _ = runner_or_skip!();
    let f = Fixture::new(Upstream::Ahead);
    f.set_agent_mode("repoint");
    let before = Untouched::capture(&f);
    let base_ref = f.remote_ref(BRANCH);
    let run = f.run(&f.publish_recipe("fixture-publish"), &[], &[]);

    assert_eq!(
        f.agent_calls(),
        vec!["step-15-commit-push"],
        "{}",
        run.debug()
    );
    assert_failed_with(&run, "push-unverified");
    assert_eq!(
        f.remote_ref(BRANCH),
        base_ref,
        "the real remote must not move"
    );
    assert_ne!(run.status("step-16-create-draft-pr"), "completed");
    before.assert_held(&f, &run);
}

// --- step-18c ------------------------------------------------------------------

/// step-18c has no intended skip: with its worktree gone, the run ends even
/// when the agent answers NONE.
#[test]
fn step18c_missing_worktree_ends_the_run() {
    let _ = runner_or_skip!();
    let f = Fixture::new(Upstream::Ahead);
    f.set_agent_mode("none-noworktree");
    let before = Untouched::capture(&f);
    let base_ref = f.remote_ref(BRANCH);
    let run = f.run(&f.pr_review_recipe(&f.p("gone")), &[], &[]);

    assert_failed_with(&run, "not-linked-worktree");
    assert_eq!(f.remote_ref(BRANCH), base_ref);
    before.assert_held(&f, &run);
}

/// step-18c refuses a detached HEAD. An agent that pushes it to the task
/// branch anyway is refused by the gate.
#[test]
fn step18c_detached_head_is_refused_even_if_the_agent_pushes() {
    let _ = runner_or_skip!();
    let f = Fixture::new(Upstream::Ahead);
    f.git(&f.wt(), &["switch", "-q", "--detach"]);
    f.set_agent_mode("rogue");
    let before = Untouched::capture(&f);
    let run = f.run(&f.pr_review_recipe(&f.wt()), &[], &[]);

    assert_eq!(
        run.status("step-18c-push-feedback-changes"),
        "failed",
        "{}",
        run.debug()
    );
    assert_failed_with(&run, "unsafe-ref");
    before.assert_held(&f, &run);
}

/// A bundle without worktree_setup_path.sh is named as such: the gate fails
/// with `push-unverified`, not with a missing worktree.
#[test]
fn step18c_missing_worktree_helper_is_named_not_reported_as_no_worktree() {
    let _ = runner_or_skip!();
    let f = Fixture::new(Upstream::Ahead);
    f.set_agent_mode("none-noworktree");
    let tools = f.p("bundle/amplifier-bundle/tools");
    fs::create_dir_all(&tools).unwrap();
    for entry in fs::read_dir(repo_root().join("amplifier-bundle/tools")).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if entry.file_type().unwrap().is_file() && name != "worktree_setup_path.sh" {
            fs::copy(entry.path(), tools.join(&name)).unwrap();
        }
    }
    // The runner finds the sub-recipe through AMPLIHACK_HOME as well.
    let recipes = f.p("bundle/amplifier-bundle/recipes");
    fs::create_dir_all(&recipes).unwrap();
    fs::copy(
        repo_root().join("amplifier-bundle/recipes/push-failure-recovery.yaml"),
        recipes.join("push-failure-recovery.yaml"),
    )
    .unwrap();
    let before = Untouched::capture(&f);
    let base_ref = f.remote_ref(BRANCH);
    let bundle = f.p("bundle").display().to_string();
    let run = f.run(
        &f.pr_review_recipe(&f.wt()),
        &[],
        &[("AMPLIHACK_HOME", bundle.as_str())],
    );

    assert_failed_with(&run, "push-unverified");
    assert!(
        run.all_text()
            .contains("worktree_setup_path.sh not found in the bundle"),
        "the failure must name the missing helper; {}",
        run.debug()
    );
    assert!(
        !run.all_text()
            .contains("push-failure-recovery: not-linked-worktree"),
        "a missing helper is not a missing worktree; {}",
        run.debug()
    );
    assert_eq!(f.remote_ref(BRANCH), base_ref);
    before.assert_held(&f, &run);
}

/// N1 without an upstream: rule 10 does not run, and the remote branch has
/// a commit HEAD lacks. The agent merges origin/feat/x (by the run's branch
/// name, not `@{u}`) and pushes with a plain push; the gate accepts the merge
/// and does not call it a rewrite.
#[test]
fn step18c_diverged_remote_without_upstream_is_merged_and_pushed() {
    let _ = runner_or_skip!();
    let f = Fixture::new(Upstream::Diverged);
    f.git(&f.wt(), &["branch", "-q", "--unset-upstream"]);
    f.set_agent_mode("merge");
    let before = Untouched::capture(&f);
    let remote_before = f.ls_remote(BRANCH);
    let run_head = f.head();
    let run = f.run(&f.pr_review_recipe(&f.wt()), &[], &[]);

    assert_eq!(
        f.agent_calls(),
        vec!["step-18c-push-feedback-changes"],
        "{}",
        run.debug()
    );
    assert_eq!(run.code, 0, "{}", run.debug());
    let head = f.head();
    assert_merged(&f, &remote_before, &run_head);
    assert_recovered(&run, "push_recovery_result", "recovered-environment", &head);
    assert_eq!(f.ls_remote(BRANCH), head, "the plain push reached origin");
    assert!(
        !run.all_text().contains("remote-history-rewritten"),
        "a merge is not a rewrite; {}",
        run.debug()
    );
    before.assert_held(&f, &run);
}

#[test]
fn step18c_environment_fault_is_recovered() {
    let _ = runner_or_skip!();
    let f = Fixture::new(Upstream::Ahead);
    let before = Untouched::capture(&f);
    let run = f.run(&f.pr_review_recipe(&f.wt()), &[], &[]);

    assert_eq!(run.code, 0, "{}", run.debug());
    assert_eq!(run.status("step-18c-push-feedback-changes"), "failed");
    assert_eq!(f.agent_calls(), vec!["step-18c-push-feedback-changes"]);
    assert_eq!(f.remote_ref(BRANCH), f.head());
    let head = f.head();
    assert_recovered(&run, "push_recovery_result", "recovered-environment", &head);
    let snap = snapshot(&run);
    assert_eq!(snap["remote_before_source"], "fetched", "snapshot: {snap}");
    before.assert_held(&f, &run);
}

/// Values of push_recovery and push_recovery_result left by an earlier
/// recovery (step-15's) must not decide step-18c's: its gate judges the
/// fresh agent output, which here says CHANGE.
#[test]
fn step18c_stale_recovery_keys_do_not_reach_the_call() {
    let _ = runner_or_skip!();
    let f = Fixture::new(Upstream::Ahead);
    f.touch("reject-always");
    f.set_agent_mode("change");
    let before = Untouched::capture(&f);
    let base_ref = f.remote_ref(BRANCH);
    let stale_agent = r#"push_recovery={"failed":"x","cause":"ENVIRONMENT","branch":"feat/x","evidence":[""],"actions":[],"push_result":"pushed","explanation":"stale"}"#;
    let stale_gate = r#"push_recovery_result={"pushed":"true","sha":"","branch":"feat/x","reason":"recovered-environment","recovery_attempted":"true"}"#;
    let run = f.run(
        &f.pr_review_recipe(&f.wt()),
        &[stale_agent, stale_gate],
        &[],
    );

    assert_eq!(
        f.agent_calls(),
        vec!["step-18c-push-feedback-changes"],
        "{}",
        run.debug()
    );
    assert_failed_with(&run, "cause-in-change");
    assert_eq!(f.remote_ref(BRANCH), base_ref);
    before.assert_held(&f, &run);
}

// --- what the fake agent saw -----------------------------------------------------

/// A recovery sends exactly one agent prompt, which carries no variable's
/// value, and fires no hook-repair loop.
#[test]
fn recovery_calls_the_agent_once_without_leaking_values() {
    let _ = runner_or_skip!();
    let f = Fixture::new(Upstream::Ahead);
    let run = f.run(&f.publish_recipe("fixture-publish"), &[], &[]);
    assert_eq!(run.code, 0, "{}", run.debug());
    let prompts: BTreeMap<String, String> = fs::read_dir(f.p("prompts"))
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (
                e.file_name().to_string_lossy().into_owned(),
                fs::read_to_string(e.path()).unwrap(),
            )
        })
        .collect();
    assert_eq!(
        prompts.len(),
        1,
        "exactly one agent call: {:?}",
        prompts.keys()
    );
    let prompt = prompts.values().next().unwrap();
    assert!(
        !prompt.contains(INHERITED_ISSUE),
        "the prompt must not carry a variable's value"
    );
    assert!(
        !fs::read_to_string(f.p("amplihack-recipe-calls.log"))
            .unwrap_or_default()
            .contains("recipe run"),
        "no hook-repair loop fires for a remote-side rejection"
    );
}
