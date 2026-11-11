//! Shared fixture for the issue #1641 open-PR finalization tests.
//!
//! Each test gets a private temp directory (under `$TMPDIR`, which tempfile
//! honours) holding:
//! - `repo/`: a real git repo on branch `feat/x`, origin
//!   `https://github.com/rysweet/fixture.git`, clean worktree;
//! - `bin/gh`: a stub that logs argv to `gh-calls.log`, refuses every mutating
//!   or `api` call with exit 99, and answers `pr view` / `pr list` from canned
//!   files;
//! - `home/`, `scratch/`: private HOME and TMPDIR for the child;
//! - `remotes/`: every `https://github.com/` fetch is rewritten here, so the
//!   open-PR check's fetch (#1714) never leaves the machine.
//!
//! The child runs with `env_clear()`, so no ambient recipe context
//! (`RECIPE_VAR_*`, `PR_URL`, `BRANCH_NAME`, ...) from an enclosing workflow run
//! can reach the validator. Only what a test passes is visible.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

pub use super::builders::*;
use serde_json::Value;
use tempfile::TempDir;

pub const BRANCH: &str = "feat/x";
pub const REPO: &str = "rysweet/fixture";
pub const PR_URL: &str = "https://github.com/rysweet/fixture/pull/7";
/// A credential-bearing string the stub prints on gh failure; it must never
/// reach the validator's stdout or stderr.
pub const LEAK_MARKER: &str = "https://token-1641@example.com/secret";

pub fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

pub fn tool_path(name: &str) -> PathBuf {
    workspace_root().join("amplifier-bundle/tools").join(name)
}

pub fn tool_text(name: &str) -> String {
    let path = tool_path(name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

const GH_STUB: &str = r#"#!/usr/bin/env bash
printf '%s\n' "$*" >> "$GH_FIXTURE_DIR/gh-calls.log"
case "${1:-} ${2:-}" in
  "pr create"|"pr edit"|"pr merge"|"pr comment"|"pr ready"|"pr close"|"pr review")
    echo "unexpected gh mutation: $*" >&2; exit 99 ;;
esac
[ "${1:-}" = "api" ] && { echo "unexpected gh api call" >&2; exit 99; }
case "${1:-} ${2:-}" in
  "pr view") kind=view ;;
  "pr list") kind=list ;;
  "auth status") exit 0 ;;
  *) echo "unexpected gh call: $*" >&2; exit 99 ;;
esac
code=$(cat "$GH_FIXTURE_DIR/$kind.exit" 2>/dev/null || echo 0)
if [ "$code" != 0 ]; then
  echo "fixture failure $GH_LEAK_MARKER" >&2
  exit "$code"
fi
cat "$GH_FIXTURE_DIR/$kind.json" 2>/dev/null || { echo "no canned $kind" >&2; exit 98; }
"#;

pub struct Fixture {
    pub tmp: TempDir,
    pub repo: PathBuf,
}

/// The validator outcome: exit status, raw streams and the parsed result.
pub struct Outcome {
    pub output: Output,
    pub stdout: String,
    pub stderr: String,
}

impl Outcome {
    pub fn json(&self) -> Value {
        serde_json::from_str(self.stdout.trim()).unwrap_or_else(|e| {
            panic!(
                "validator must emit one JSON result: {e}\nstdout:\n{}\nstderr:\n{}",
                self.stdout, self.stderr
            )
        })
    }

    pub fn field(&self, key: &str) -> String {
        self.json()[key]
            .as_str()
            .unwrap_or_else(|| panic!("result field `{key}` must be a String: {}", self.stdout))
            .to_owned()
    }

    pub fn success(&self) -> bool {
        self.output.status.success()
    }

    pub fn describe(&self) -> String {
        format!(
            "exit={:?}\nstdout:\n{}\nstderr:\n{}",
            self.output.status.code(),
            self.stdout,
            self.stderr
        )
    }
}

impl Fixture {
    pub fn new() -> Self {
        Self::on_branch(BRANCH)
    }

    pub fn on_branch(branch: &str) -> Self {
        let tmp = TempDir::new().expect("tempdir under $TMPDIR");
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).expect("repo dir");
        for dir in ["home", "scratch", "bin", "remotes"] {
            fs::create_dir_all(tmp.path().join(dir)).expect("fixture dir");
        }
        let gh = tmp.path().join("bin/gh");
        fs::write(&gh, GH_STUB).expect("write gh stub");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).expect("chmod gh");
        }
        let fixture = Self { tmp, repo };
        fixture.git(&["init", "-q", "-b", "main"]);
        fixture.git(&["config", "user.email", "fixture@example.com"]);
        fixture.git(&["config", "user.name", "Fixture"]);
        fixture.git(&["config", "commit.gpgsign", "false"]);
        fixture.git(&[
            "remote",
            "add",
            "origin",
            "https://github.com/rysweet/fixture.git",
        ]);
        fs::write(fixture.repo.join("README.md"), "base\n").expect("seed file");
        fixture.git(&["add", "README.md"]);
        fixture.git(&["commit", "-q", "-m", "base"]);
        if branch != "main" {
            fixture.git(&["switch", "-q", "-c", branch]);
        }
        fixture
    }

    pub(crate) fn isolate(&self, command: &mut Command) {
        let parent_path = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into());
        command
            .env_clear()
            .current_dir(&self.repo)
            .env("HOME", self.tmp.path().join("home"))
            .env("TMPDIR", self.tmp.path().join("scratch"))
            .env("XDG_CONFIG_HOME", self.tmp.path().join("home/.config"))
            .env("GH_CONFIG_DIR", self.tmp.path().join("home/.config/gh"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_COUNT", "1")
            .env(
                "GIT_CONFIG_KEY_0",
                format!("url.{}/.insteadOf", self.remotes().display()),
            )
            .env("GIT_CONFIG_VALUE_0", "https://github.com/")
            .env("GH_FIXTURE_DIR", self.tmp.path())
            .env("GH_LEAK_MARKER", LEAK_MARKER)
            .env(
                "PATH",
                format!("{}:{parent_path}", self.tmp.path().join("bin").display()),
            );
    }

    pub fn git(&self, args: &[&str]) -> String {
        let mut command = amplihack_git::command();
        self.isolate(&mut command);
        let output = command.args(args).output().expect("run git");
        assert!(
            output.status.success(),
            "fixture git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Where `https://github.com/<owner>/<repo>` fetches are redirected.
    pub fn remotes(&self) -> PathBuf {
        self.tmp.path().join("remotes")
    }

    pub fn repo_path(&self) -> String {
        self.repo.to_string_lossy().into_owned()
    }

    /// The fixture repo's current local HEAD commit id.
    pub fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"]).trim().to_owned()
    }

    /// Canned `gh pr view` answer. An object without `headRefOid` gets the
    /// current local HEAD, i.e. the PR head was pushed; committing after this
    /// call leaves local HEAD ahead of the PR.
    pub fn gh_view(&self, body: &Value) {
        let mut body = body.clone();
        if let Some(object) = body.as_object_mut() {
            object
                .entry("headRefOid")
                .or_insert_with(|| Value::String(self.head()));
        }
        fs::write(self.tmp.path().join("view.json"), body.to_string()).expect("view.json");
    }

    /// Canned raw `gh pr view` stdout (for malformed-JSON cases).
    pub fn gh_view_raw(&self, body: &str) {
        fs::write(self.tmp.path().join("view.json"), body).expect("view.json");
    }

    /// Canned `gh pr list` answer.
    pub fn gh_list(&self, body: &Value) {
        fs::write(self.tmp.path().join("list.json"), body.to_string()).expect("list.json");
    }

    /// Make `gh pr <kind>` exit non-zero with credential-bearing stderr.
    pub fn gh_fail(&self, kind: &str, code: i32) {
        fs::write(
            self.tmp.path().join(format!("{kind}.exit")),
            code.to_string(),
        )
        .expect("exit file");
    }

    pub fn gh_calls(&self) -> Vec<String> {
        fs::read_to_string(self.tmp.path().join("gh-calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    pub fn assert_gh_not_called(&self, why: &str) {
        let calls = self.gh_calls();
        assert!(
            calls.is_empty(),
            "{why}: gh must not be called, got {calls:?}"
        );
    }

    /// Every gh call must be a read-only `pr view`/`pr list` scoped to the
    /// origin repo with `--repo`.
    pub fn assert_gh_reads_only(&self) {
        for call in self.gh_calls() {
            assert!(
                call.starts_with("pr view ") || call.starts_with("pr list "),
                "only read-only pr view/list calls are allowed, got `{call}`"
            );
            assert!(
                call.contains(&format!("--repo {REPO}")),
                "every gh call must pass --repo {REPO}, got `{call}`"
            );
        }
    }

    /// Run `workflow_agentic_finalization.sh validate` with exactly `envs`.
    pub fn validate(&self, envs: &[(&str, &str)]) -> Outcome {
        self.run_dispatcher("validate", envs)
    }

    pub fn run_dispatcher(&self, mode: &str, envs: &[(&str, &str)]) -> Outcome {
        let mut command = Command::new("bash");
        self.isolate(&mut command);
        command
            .arg(tool_path("workflow_agentic_finalization.sh"))
            .arg(mode);
        for (key, value) in envs {
            command.env(key, value);
        }
        let output = command
            .output()
            .expect("run workflow_agentic_finalization.sh");
        Outcome {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            output,
        }
    }
}
