//! tests/integration/issue_1575_publish_defects_test.rs
//!
//! TDD contract for issue #1575, three default-workflow publish defects:
//!
//! 1. A run that continues an existing PR (resume_checkpoint, existing_branch,
//!    pr_number) reached `probe-terminal-state` with unpushed commits.
//!    `workflow_sync_pr_head.sh` fast-forwards the PR head first.
//! 2. Step 15's `git add -A` swept in version churn and generated
//!    `.github/hooks/` stubs under a task-description subject.
//!    `workflow_stage_task_changes.sh` and `workflow_commit_message.sh` fix that.
//! 3. Step 21 failed on a transient `gh auth status` drop.
//!    `workflow_gh_auth_gate.sh` retries with backoff and checks push permission.
//!
//! The strings asserted here are the contract documented in
//! `docs/reference/workflow-publish-helpers.md`; change both together.
//!
//! Isolation: every child process gets a temporary `HOME`, an isolated git
//! config, a stub `gh` on `PATH`, no GitHub tokens or askpass helpers, and only
//! local bare remotes. Nothing here may touch the real `~/.claude`,
//! `~/.claude-profiles`, `~/.config/gh` or a real login.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

#[path = "issue_1575_publish_defects/auth.rs"]
mod auth;
#[path = "issue_1575_publish_defects/auth_pr_ready.rs"]
mod auth_pr_ready;
#[path = "issue_1575_publish_defects/message.rs"]
mod message;
#[path = "issue_1575_publish_defects/stage.rs"]
mod stage;
#[path = "issue_1575_publish_defects/stage_hooks.rs"]
mod stage_hooks;
#[path = "issue_1575_publish_defects/static_checks.rs"]
mod static_checks;
#[path = "issue_1575_publish_defects/step15.rs"]
mod step15;
#[path = "issue_1575_publish_defects/sync.rs"]
mod sync;
#[path = "issue_1575_publish_defects/sync_probe.rs"]
mod sync_probe;

pub(super) fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

pub(super) fn helper_path(name: &str) -> PathBuf {
    workspace_root()
        .join("amplifier-bundle")
        .join("tools")
        .join(name)
}

pub(super) fn recipe_path(name: &str) -> PathBuf {
    workspace_root()
        .join("amplifier-bundle")
        .join("recipes")
        .join(format!("{name}.yaml"))
}

pub(super) fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

pub(super) fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|e| panic!("create {}: {e}", parent.display()));
    }
    fs::write(path, content).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

pub(super) fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .unwrap_or_else(|e| panic!("chmod {}: {e}", path.display()));
}

pub(super) fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Panic message suffix with both streams of a finished command.
pub(super) fn dump(output: &Output) -> String {
    format!(
        "exit={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        text(&output.stdout),
        text(&output.stderr)
    )
}

/// Ambient variables that would let a child reach real credentials, a real
/// gh account, another git repository, or change a helper's decision.
const SCRUBBED_ENV: &[&str] = &[
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
    "GH_HOST",
    "GH_CONFIG_DIR",
    "GIT_ASKPASS",
    "SSH_ASKPASS",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_PREFIX",
    "CLAUDE_CONFIG_DIR",
    "REPO_PATH",
    "RECIPE_VAR_repo_path",
    "BASE_BRANCH",
    "RECIPE_VAR_base_branch",
    "ALLOW_VERSION_BUMP",
    "RECIPE_VAR_allow_version_bump",
    "VERSION_BUMP_NEW_VERSION",
    "VERSION_BUMP_CURRENT_VERSION",
    "VERSION_BUMP_CHANGE_CLASSIFICATION",
    "RECIPE_VAR_version_bump__new_version",
    "RECIPE_VAR_version_bump__current_version",
    "RECIPE_VAR_version_bump__change_classification",
    "WORKFLOW_GH_AUTH_ATTEMPTS",
    "WORKFLOW_GH_AUTH_BASE_DELAY",
    "WORKFLOW_GH_AUTH_MAX_DELAY",
    "WORKFLOW_GH_AUTH_GATE_HELPER",
    "WORKFLOW_GH_RETRY_HELPER",
    "WORKFLOW_PR_SCOPE_HELPER",
    "TASK_DESCRIPTION",
    "ISSUE_NUMBER",
    "RECIPE_VAR_issue_number",
    "PR_URL",
    "RECIPE_VAR_pr_url",
    "PR_NUMBER",
    "RECIPE_VAR_pr_number",
    "PR_PUBLISH_RESULT_PR_URL",
    "RECIPE_VAR_pr_publish_result__pr_url",
    "PR_PUBLISH_RESULT_PR_NUMBER",
    "RECIPE_VAR_pr_publish_result__pr_number",
    "RECIPE_VAR_existing_branch",
    "RECIPE_VAR_resume_checkpoint",
    "EXPECTED_PR_TITLE_PREFIX",
    "PR_EXPECTED_TITLE_PREFIX",
    "WORKFLOW_STARTED_AT",
    "RECIPE_STARTED_AT",
    "TASK_STARTED_AT",
    "WORKTREE_SETUP_WORKTREE_PATH",
    "RECIPE_VAR_worktree_setup__worktree_path",
];

/// A private HOME, git config and `bin/` directory for one test.
pub(super) struct Sandbox {
    pub tmp: TempDir,
}

impl Sandbox {
    pub fn new() -> Self {
        let tmp = TempDir::new().expect("tempdir");
        for dir in ["home", "bin", "scratch", "state"] {
            fs::create_dir_all(tmp.path().join(dir)).expect("sandbox dir");
        }
        write_file(
            &tmp.path().join("gitconfig"),
            "[user]\n\temail = test@example.com\n\tname = Workflow Test\n\
             [init]\n\tdefaultBranch = main\n[advice]\n\tdetachedHead = false\n\
             [url \"file:///nonexistent-network-guard/\"]\n\tinsteadOf = https://github.com/\n",
        );
        let sandbox = Sandbox { tmp };
        // Default gh stub: authenticated, push=true, login `tester`.
        sandbox.write_gh(DEFAULT_GH_STUB);
        sandbox
    }

    pub fn root(&self) -> &Path {
        self.tmp.path()
    }

    pub fn bin(&self) -> PathBuf {
        self.root().join("bin")
    }

    pub fn home(&self) -> PathBuf {
        self.root().join("home")
    }

    pub fn state(&self) -> PathBuf {
        self.root().join("state")
    }

    pub fn gh_log(&self) -> PathBuf {
        self.root().join("gh-calls.log")
    }

    pub fn gh_calls(&self) -> String {
        fs::read_to_string(self.gh_log()).unwrap_or_default()
    }

    pub fn write_gh(&self, script: &str) {
        let gh = self.bin().join("gh");
        write_file(&gh, script);
        make_executable(&gh);
    }

    /// Replace `sleep` so backoff delays are recorded instead of waited.
    pub fn stub_sleep(&self) -> PathBuf {
        let log = self.root().join("sleep-calls.log");
        let sleep = self.bin().join("sleep");
        write_file(
            &sleep,
            "#!/usr/bin/env bash\nprintf '%s\\n' \"$*\" >> \"$SLEEP_CALL_LOG\"\n",
        );
        make_executable(&sleep);
        log
    }

    /// Apply the isolated environment to any command.
    pub fn isolate(&self, command: &mut Command) {
        for key in SCRUBBED_ENV {
            command.env_remove(key);
        }
        let path = format!(
            "{}:{}",
            self.bin().display(),
            crate::amplihack_path::path_with_amplihack_bin()
        );
        command
            .env("HOME", self.home())
            .env("TMPDIR", self.root().join("scratch"))
            .env("XDG_CONFIG_HOME", self.home().join(".config"))
            .env("GIT_CONFIG_GLOBAL", self.root().join("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_AUTHOR_NAME", "Workflow Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Workflow Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .env("PATH", path)
            .env("AMPLIHACK_HOME", workspace_root())
            .env("GH_CALL_LOG", self.gh_log())
            .env("GH_STUB_STATE", self.state())
            .env("SLEEP_CALL_LOG", self.root().join("sleep-calls.log"))
            .env("WORKFLOW_GH_AUTH_BASE_DELAY", "0")
            .env("GIT_PAGER", "cat")
            .env("PAGER", "cat");
    }

    pub fn command(&self, program: &str, dir: &Path) -> Command {
        let mut command = Command::new(program);
        command.current_dir(dir);
        self.isolate(&mut command);
        command
    }

    pub fn git_output(&self, dir: &Path, args: &[&str]) -> Output {
        self.command("git", dir)
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("run git {args:?} in {}: {e}", dir.display()))
    }

    pub fn git(&self, dir: &Path, args: &[&str]) -> String {
        let output = self.git_output(dir, args);
        assert!(
            output.status.success(),
            "git {args:?} failed in {}\n{}",
            dir.display(),
            dump(&output)
        );
        text(&output.stdout).trim_end_matches('\n').to_string()
    }

    pub fn commit_file(&self, dir: &Path, rel: &str, content: &str, message: &str) {
        write_file(&dir.join(rel), content);
        self.git(dir, &["add", "--", rel]);
        self.git(dir, &["commit", "-q", "-m", message]);
    }

    /// Run a bundled helper with `bash` in `dir`.
    pub fn run_helper(
        &self,
        dir: &Path,
        helper: &str,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> Output {
        let mut command = self.command("bash", dir);
        command.arg(helper_path(helper)).args(args);
        for (key, value) in env {
            command.env(key, value);
        }
        command
            .output()
            .unwrap_or_else(|e| panic!("run {helper} {args:?}: {e}"))
    }

    /// A bare `origin.git` and a clone `repo` with `main` and a published
    /// `feature` branch (upstream set). `repo` is left on `feature`.
    pub fn remote_fixture(&self) -> RemoteFixture {
        let origin = self.root().join("origin.git");
        let repo = self.root().join("repo");
        self.git(
            self.root(),
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                origin.to_str().unwrap(),
            ],
        );
        self.git(
            self.root(),
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                repo.to_str().unwrap(),
            ],
        );
        self.git(&repo, &["switch", "-q", "-c", "main"]);
        self.commit_file(&repo, "README.md", "base\n", "base");
        self.git(&repo, &["push", "-q", "-u", "origin", "main"]);
        self.git(&repo, &["switch", "-q", "-c", "feature"]);
        self.commit_file(&repo, "feature.txt", "f1\n", "feature commit 1");
        self.git(&repo, &["push", "-q", "-u", "origin", "feature"]);
        RemoteFixture { origin, repo }
    }

    pub fn bare_ref(&self, origin: &Path, branch: &str) -> Option<String> {
        let output = self.git_output(
            origin,
            &[
                "rev-parse",
                "--verify",
                "-q",
                &format!("refs/heads/{branch}"),
            ],
        );
        output
            .status
            .success()
            .then(|| text(&output.stdout).trim().to_string())
    }

    pub fn head(&self, dir: &Path) -> String {
        self.git(dir, &["rev-parse", "HEAD"])
    }
}

pub(super) struct RemoteFixture {
    pub origin: PathBuf,
    pub repo: PathBuf,
}

/// Authenticated, `push=true`, login `tester`; logs every call; refuses any
/// mutation. Individual tests replace it when they need other answers.
const DEFAULT_GH_STUB: &str = r#"#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$GH_CALL_LOG"
case "${1:-} ${2:-}" in
  "auth status") exit 0 ;;
  "api user") echo tester; exit 0 ;;
  "api repos/"*) echo true; exit 0 ;;
esac
echo "unexpected gh call: $*" >&2
exit 99
"#;

/// Assert that `haystack` contains `needle` as one whole line.
pub(super) fn assert_line(haystack: &str, needle: &str, context: &str) {
    assert!(
        haystack.lines().any(|line| line == needle),
        "{context}: expected the exact line\n  {needle}\nin:\n{haystack}"
    );
}

/// The single stdout line printed by `workflow_sync_pr_head.sh --mode step15`.
pub(super) fn step15_line(output: &Output) -> String {
    let stdout = text(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "step15 mode must print exactly one stdout line\n{}",
        dump(output)
    );
    lines[0].to_string()
}
