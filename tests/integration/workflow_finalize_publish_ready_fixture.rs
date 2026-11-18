//! Shared, std-only fixture for Rust regression tests and the CI scenario driver.
//! All commands use a cleared environment, local Git transport and private HOME.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

pub const BRANCH: &str = "feat/issue-2787-publication";
pub const PR_URL: &str = "https://github.com/acme/widget/pull/7";
pub const PUBLISH: &str = "step-20b-push-cleanup";
pub const READY: &str = "step-21-pr-ready";

pub fn show(o: &Output) -> String {
    format!(
        "exit={}\nstdout={}\nstderr={}",
        o.status,
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

pub fn success(o: Output) -> String {
    assert!(o.status.success(), "{}", show(&o));
    String::from_utf8(o.stdout).expect("UTF-8 output")
}

pub fn executable(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

pub struct Fixture {
    pub root: PathBuf,
    pub source: PathBuf,
    pub binary: PathBuf,
    pub main: PathBuf,
    pub pr: PathBuf,
    pub remote: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove only this fixture's directory");
    }
}

impl Fixture {
    pub fn new(source: &Path, binary: &Path) -> Self {
        let root = success(
            Command::new("mktemp")
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .args(["-d"])
                .arg(std::env::temp_dir().join("issue-2787-XXXXXXXX"))
                .output()
                .unwrap(),
        );
        let root = fs::canonicalize(root.trim()).unwrap();
        // Recovery relaunches current_exe. Keep an owned pathname to the built
        // inode so concurrent Cargo linking cannot remove that launch target.
        let built = binary.canonicalize().expect("built amplihack binary");
        let snapshot = root.join("built-amplihack");
        match fs::hard_link(&built, &snapshot) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
                fs::copy(&built, &snapshot).expect("copy built binary across filesystems");
            }
            Err(error) => panic!("pin fixture executable {}: {error}", built.display()),
        }
        let f = Self {
            main: root.join("main"),
            pr: root.join("pr"),
            remote: root.join("remotes/acme/widget.git"),
            root,
            source: source.canonicalize().unwrap(),
            binary: snapshot,
        };
        for dir in ["tmp/home", "bin", "runtime", "recovery", "remotes/acme"] {
            fs::create_dir_all(f.root.join(dir)).unwrap();
            fs::set_permissions(f.root.join(dir), fs::Permissions::from_mode(0o700)).unwrap();
        }
        executable(&f.root.join("bin/gh"), GH);
        executable(&f.root.join("bin/amplihack"), AMPLIHACK);
        executable(&f.root.join("bin/recovery-runner"), RECOVERY_RUNNER);
        f.git(
            &f.root,
            &["init", "--bare", "-q", "remotes/acme/widget.git"],
        );
        f.git(&f.root, &["init", "-q", "-b", "main", "main"]);
        f.git(&f.main, &["config", "user.name", "Publication Test"]);
        f.git(&f.main, &["config", "user.email", "test@example.com"]);
        f.git(&f.main, &["config", "commit.gpgsign", "false"]);
        f.git(
            &f.main,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/widget.git",
            ],
        );
        fs::write(f.main.join("README.md"), "baseline\n").unwrap();
        f.git(&f.main, &["add", "README.md"]);
        f.git(&f.main, &["commit", "-qm", "baseline"]);
        f.git(&f.main, &["push", "-q", "-u", "origin", "main"]);
        f.git(&f.main, &["remote", "set-head", "origin", "main"]);
        f.git(
            &f.main,
            &["worktree", "add", "-qb", BRANCH, f.pr.to_str().unwrap()],
        );
        f.git(&f.pr, &["push", "-q", "-u", "origin", BRANCH]);
        fs::write(f.root.join("prior-head"), f.head(&f.pr)).unwrap();
        fs::write(
            f.root.join("candidate.json"),
            format!(
                r##"{{
            "number":7,"url":"{PR_URL}","state":"OPEN","isDraft":true,
            "title":"Fix: publication #2787","body":"Fixes #2787",
            "createdAt":"2026-11-18T04:15:53Z","mergedAt":null,
            "headRefName":"{BRANCH}","baseRefName":"main","headRefOid":"",
            "headRepositoryOwner":{{"login":"acme"}},
            "headRepository":{{"name":"widget","nameWithOwner":"acme/widget"}},
            "isCrossRepository":false,"statusCheckRollup":[]
        }}"##
            ),
        )
        .unwrap();
        let hooks = f.git(
            &f.pr,
            &["rev-parse", "--path-format=absolute", "--git-path", "hooks"],
        );
        executable(
            &Path::new(hooks.trim()).join("pre-commit"),
            "#!/bin/sh\nprintf 'commit-hook\\n' >> \"$FIXTURE/events\"\n",
        );
        executable(
            &f.remote.join("hooks/post-receive"),
            "#!/bin/sh\ncat >/dev/null\nprintf 'published\\n' >> \"$FIXTURE/events\"\n",
        );
        f
    }

    pub fn command(&self, program: impl AsRef<std::ffi::OsStr>, cwd: &Path) -> Command {
        let mut c = Command::new(program);
        c.env_clear()
            .current_dir(cwd)
            .stdin(Stdio::null())
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("bin").display()),
            )
            .env("HOME", self.root.join("tmp/home"))
            .env("CLAUDE_CONFIG_DIR", self.root.join("tmp/home/.claude"))
            .env("CODEX_HOME", self.root.join("tmp/home/.codex"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("XDG_CONFIG_HOME", self.root.join("tmp/home/.config"))
            .env("XDG_STATE_HOME", self.root.join("tmp/home/.local/state"))
            .env("XDG_CACHE_HOME", self.root.join("tmp/home/.cache"))
            .env("XDG_RUNTIME_DIR", self.root.join("runtime"))
            .env("GH_CONFIG_DIR", self.root.join("tmp/home/gh"))
            .env("LANG", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_COUNT", "1")
            .env(
                "GIT_CONFIG_KEY_0",
                format!("url.{}/.insteadOf", self.root.join("remotes").display()),
            )
            .env("GIT_CONFIG_VALUE_0", "https://github.com/")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ALLOW_PROTOCOL", "file")
            .env("FIXTURE", &self.root)
            .env("REAL_AMPLIHACK", &self.binary)
            .env("AMPLIHACK_HOME", &self.source)
            .env("AMPLIHACK_RUNTIME_ROOT", self.root.join("runtime"))
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_DISABLE_BLARIFY", "1")
            .env("REPO_PATH", &self.main)
            .env("REMOTE_HOST_TYPE", "github")
            .env("RECIPE_VAR_pr_number", "7")
            .env("RECIPE_VAR_pr_url", PR_URL)
            .env("RECIPE_VAR_issue_number", "2787")
            .env("RECIPE_VAR_base_branch", "main")
            .env("RECIPE_VAR_worktree_setup", self.worktree_json())
            .env("RECIPE_VAR_step_recovery_dir", "")
            .env(
                "RECIPE_RUNNER_RS_PATH",
                self.root.join("bin/recovery-runner"),
            );
        c
    }

    pub fn worktree_json(&self) -> String {
        // Serialize paths as data using jq, including spaces, quotes and backslashes.
        success(
            Command::new("jq")
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .args(["-cn", "--arg", "p"])
                .arg(&self.pr)
                .arg("{worktree_path:$p}")
                .output()
                .unwrap(),
        )
        .trim()
        .to_owned()
    }

    pub fn git(&self, cwd: &Path, args: &[&str]) -> String {
        success(self.command("git", cwd).args(args).output().unwrap())
    }
    pub fn head(&self, cwd: &Path) -> String {
        self.git(cwd, &["rev-parse", "HEAD"]).trim().to_owned()
    }
    pub fn remote_head(&self) -> String {
        self.git(
            &self.remote,
            &["rev-parse", &format!("refs/heads/{BRANCH}")],
        )
        .trim()
        .to_owned()
    }
    pub fn audit(&self, committed: bool) {
        fs::write(self.pr.join("audit.txt"), "audit fix included\n").unwrap();
        if committed {
            self.git(&self.pr, &["add", "audit.txt"]);
            self.git(&self.pr, &["commit", "-qm", "audit correction"]);
        }
    }
    pub fn tool(&self, name: &str, cwd: &Path) -> Command {
        let mut c = self.command("bash", cwd);
        c.arg(self.source.join("amplifier-bundle/tools").join(name));
        c
    }
    pub fn step_body(&self, id: &str) -> String {
        success(
            self.command("bash", &self.main)
                .args([
                    "-o", "pipefail", "-c",
                    r#""$REAL_AMPLIHACK" recipe show "$2" --format json |
                       jq -er --arg id "$1" '.steps[] | select(.id == $id) | .command | strings | select(length > 0)'"#,
                    "load-recipe-command", id,
                ])
                .arg(
                    self.source
                        .join("amplifier-bundle/recipes/workflow-finalize.yaml"),
                )
                .output()
                .unwrap(),
        )
    }
    pub fn step(&self, id: &str) -> Command {
        let mut c = self.command("bash", &self.main);
        c.args(["-c", &format!("umask 022\n{}", self.step_body(id))]);
        c
    }
    pub fn publish(&self) -> Output {
        self.tool("workflow_finalize_publish.sh", &self.main)
            .output()
            .unwrap()
    }
    pub fn ready(&self) -> Output {
        self.tool("workflow_pr_ready.sh", &self.main)
            .args(["--base", "main"])
            .output()
            .unwrap()
    }
    pub fn patch_candidate(&self, filter: &str) {
        let json = success(
            self.command("jq", &self.main)
                .arg(filter)
                .arg(self.root.join("candidate.json"))
                .output()
                .unwrap(),
        );
        fs::write(self.root.join("candidate.json"), json).unwrap();
    }
    pub fn assert_ready(&self) {
        assert_eq!(
            self.remote_head(),
            self.head(&self.pr),
            "published HEAD must be exact"
        );
        assert!(
            self.root.join("ready").exists(),
            "no validated readiness mutation occurred"
        );
        assert_eq!(
            self.git(
                &self.remote,
                &["show", &format!("refs/heads/{BRANCH}:audit.txt")]
            ),
            "audit fix included\n"
        );
        let events = fs::read_to_string(self.root.join("events")).unwrap();
        let published = events.find("published").expect("push event");
        let inspected = events.find("pr-read").expect("PR lookup event");
        let ready = events.find("ready\n").expect("ready event");
        assert!(
            published < inspected && inspected < ready,
            "event order: {events}"
        );
    }
    pub fn assert_no_mutation(&self) {
        assert!(
            !self.root.join("ready").exists(),
            "unexpected readiness mutation"
        );
        let calls = match fs::read_to_string(self.root.join("gh-calls")) {
            Ok(calls) => calls,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => panic!("cannot verify GitHub mutation history: {error}"),
        };
        assert!(
            !calls
                .lines()
                .any(|s| s.starts_with("pr ready") || s.starts_with("pr comment")),
            "{calls}"
        );
    }
}

const GH: &str = r##"#!/bin/bash
set -euo pipefail
printf '%s\n' "$*" >> "$FIXTURE/gh-calls"
case "${1:-} ${2:-}" in
  'auth status') exit 0 ;;
  'pr view'|'pr list')
    printf 'pr-read\n' >> "$FIXTURE/events"
    if [ -f "$FIXTURE/empty" ] && [ "$2" = list ]; then echo '[]'; exit 0; fi
    head="$(git -C "$FIXTURE/remotes/acme/widget.git" rev-parse refs/heads/feat/issue-2787-publication)"
    [ ! -f "$FIXTURE/stale" ] || head="$(cat "$FIXTURE/prior-head")"
    json="$(jq --arg head "$head" '.headRefOid=$head' "$FIXTURE/candidate.json")"
    # Scoped reads include --repo. Only the readiness boundary's fresh read
    # receives this patch, so tests prove selection cannot validate it for us.
    final_read=true
    for arg in "$@"; do [ "$arg" != --repo ] || final_read=false; done
    if [ "$2" = view ] && [ "$final_read" = true ]; then
      printf 'final-pr-read\n' >> "$FIXTURE/events"
      if [ -f "$FIXTURE/final-filter.jq" ]; then
        json="$(printf '%s' "$json" | jq -f "$FIXTURE/final-filter.jq")"
      fi
    fi
    if [ "$2" = list ]; then json="[$json]"; fi
    filter=""; prev=""
    for arg in "$@"; do
      [ "$prev" != --jq ] && [ "$prev" != -q ] || filter="$arg"
      prev="$arg"
    done
    if [ -n "$filter" ]; then printf '%s\n' "$json" | jq -r "$filter"; else printf '%s\n' "$json"; fi ;;
  'pr ready')
    # Independent oracle: inspect actual remote objects at mutation time.
    [ "${3:-}" = https://github.com/acme/widget/pull/7 ] || exit 91
    remote="$(git -C "$FIXTURE/remotes/acme/widget.git" rev-parse refs/heads/feat/issue-2787-publication)"
    local_head="$(git -C "$FIXTURE/pr" rev-parse HEAD)"
    [ "$remote" = "$local_head" ] || { echo 'mutation before publication' >&2; exit 92; }
    [ "$(git -C "$FIXTURE/remotes/acme/widget.git" show "$remote:audit.txt")" = 'audit fix included' ] || exit 93
    printf 'ready\n' >> "$FIXTURE/events"
    : > "$FIXTURE/ready" ;;
  'pr comment') [ -f "$FIXTURE/ready" ] || exit 94 ;;
  *) echo "unsupported gh command: $*" >&2; exit 99 ;;
esac
"##;

const AMPLIHACK: &str = r#"#!/bin/bash
set -euo pipefail
if [ "${1:-} ${2:-}" = 'hygiene artifact-guard' ]; then
  printf 'artifact-check\n' >> "$FIXTURE/events"
fi
exec "$REAL_AMPLIHACK" "$@"
"#;

const RECOVERY_RUNNER: &str = r##"#!/bin/bash
set -euo pipefail
case "${1:-}" in
  --version) echo fixture; exit 0 ;;
  --capabilities) echo '{"schema_version":1,"version":"fixture","capabilities":["codex_exec","durable_recovery_v1"]}'; exit 0 ;;
esac
printf 'recovery-agent\n' >> "$FIXTURE/events"
for a in "$@"; do
  case "$a" in step_recovery_evidence=*) cp "${a#step_recovery_evidence=}" "$FIXTURE/recovery-evidence.json" ;; esac
done
reply='{"step_recovery":{"choice":"handoff","reason":"Cannot validate publication context","owner":"run-requester","needs":"Restore worktree context or publish audit work"}}'
if [ -x "$FIXTURE/repair" ]; then
  "$FIXTURE/repair"
  reply='{"step_recovery":{"choice":"retry_after_repair","reason":"Repaired local publication state","repair":"Removed the fixture push rejection"}}'
fi
jq -cn --arg reply "$reply" '{recipe_name:"step-failure-recovery",success:true,step_results:[{step_id:"recover",status:"completed",output:$reply}],context:{}}'
"##;
