//! Issue #1619 (TDD): the small-diff combined review.
//!
//! The `review-mode` step of `workflow-refactor-review` prints `true` when the
//! code, security and philosophy passes may run as one combined review, and
//! `false` when they run separately. Its body is
//! `amplifier-bundle/tools/workflow_review_mode_step.sh`.
//!
//! Each case installs that script, and the worktree-path helper it calls,
//! into a temporary `$AMPLIHACK_HOME` as an installed bundle holds them. It
//! builds a git repository of its own whose branch changes a known number of
//! lines, writes the user settings into a temporary home, runs the script and
//! checks stdout and the exit status. The script reads the settings through
//! the `amplihack` binary under test, which is first on `PATH`.
//!
//! The setting is user-only and off by default, and every failure answers
//! `false`: a review mode is a preference, never a reason to stop the phase,
//! and a repository under review cannot fold its own security pass away.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::isolated_amplihack::isolated;

/// The step body under test.
const SCRIPT: &str = "workflow_review_mode_step.sh";
/// The helper the body calls to find the run's worktree.
const WORKTREE_TOOL: &str = "worktree_setup_path.sh";

/// The bundle's tool scripts in this checkout.
fn bundle_tools() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../amplifier-bundle/tools")
}

/// `PATH` for the script: the directory of the `amplihack` binary under test
/// first, then the system directories that hold `git`, `jq` and `awk`. The
/// caller's own `PATH` is not used, so an installed `amplihack` is never the
/// one that answers.
fn script_path() -> String {
    let bin_dir = Path::new(env!("CARGO_BIN_EXE_amplihack"))
        .parent()
        .expect("the amplihack binary has a parent directory")
        .display()
        .to_string();
    format!("{bin_dir}:/usr/local/bin:/usr/bin:/bin")
}

/// Git in `dir`, with no system or global config and a fixed identity.
fn git(dir: &Path, args: &[&str]) {
    let output = amplihack_git::command_in(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn private_dir(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
}

fn private_file(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

/// One isolated stage: a home, an `$AMPLIHACK_HOME` holding the bundle's two
/// scripts, and a repository whose branch `feat` changes `lines` lines
/// against `main`.
struct Case {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    amplihack_home: PathBuf,
    repo: PathBuf,
}

impl Case {
    fn new(lines: usize) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        private_dir(&root);
        let home = root.join("home");
        let amplihack_home = root.join("amplihack-home");
        let tools = amplihack_home.join("amplifier-bundle").join("tools");
        let repo = root.join("repo");
        for dir in [
            &home,
            &home.join(".amplihack"),
            &amplihack_home,
            &tools,
            &repo,
        ] {
            private_dir(dir);
        }
        for name in [SCRIPT, WORKTREE_TOOL] {
            let source = bundle_tools().join(name);
            let target = tools.join(name);
            fs::copy(&source, &target).unwrap_or_else(|error| {
                panic!(
                    "cannot install {} from the bundle: {error}. The review-mode step \
                     runs this script.",
                    source.display()
                )
            });
            fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
        }

        git(&repo, &["init", "-q", "-b", "main"]);
        fs::write(repo.join("README.md"), "base\n").unwrap();
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "-q", "-m", "base"]);
        git(&repo, &["checkout", "-q", "-b", "feat"]);
        let body: String = (0..lines).map(|i| format!("line {i}\n")).collect();
        fs::write(repo.join("change.txt"), body).unwrap();
        git(&repo, &["add", "change.txt"]);
        git(&repo, &["commit", "-q", "-m", "change"]);

        Self {
            _tmp: tmp,
            home,
            amplihack_home,
            repo,
        }
    }

    /// The user's own `~/.amplihack/config.toml`, private as the loader
    /// requires before it honours a user-only key.
    fn user_config(&self, toml: &str) -> &Self {
        private_file(&self.home.join(".amplihack").join("config.toml"), toml);
        self
    }

    /// A `.amplihack/config.toml` inside the repository under review,
    /// untracked so it adds nothing to the diff.
    fn project_config(&self, toml: &str) -> &Self {
        let dir = self.repo.join(".amplihack");
        private_dir(&dir);
        private_file(&dir.join("config.toml"), toml);
        self
    }

    /// The `worktree_setup` object the run's step-04 hands later steps.
    fn worktree_setup(&self) -> String {
        serde_json::json!({ "worktree_path": self.repo }).to_string()
    }

    /// Move the worktree-path helper from `$AMPLIHACK_HOME` to
    /// `~/.amplihack`, the last place the script searches.
    fn helper_only_in_home_amplihack(&self) -> &Self {
        let from = self
            .amplihack_home
            .join("amplifier-bundle")
            .join("tools")
            .join(WORKTREE_TOOL);
        let tools = self
            .home
            .join(".amplihack")
            .join("amplifier-bundle")
            .join("tools");
        private_dir(tools.parent().unwrap());
        private_dir(&tools);
        fs::rename(&from, tools.join(WORKTREE_TOOL)).unwrap();
        self
    }

    /// Run the installed script as the runner runs a bash step: from the
    /// working directory, with the recipe's variables in the environment.
    fn run_with(&self, worktree_setup: Option<&str>) -> Output {
        self.command(worktree_setup)
            .output()
            .expect("run the review-mode script")
    }

    fn command(&self, worktree_setup: Option<&str>) -> Command {
        let mut command = isolated("bash");
        command
            .arg(
                self.amplihack_home
                    .join("amplifier-bundle")
                    .join("tools")
                    .join(SCRIPT),
            )
            .current_dir(&self.repo)
            .env("HOME", &self.home)
            .env("AMPLIHACK_HOME", &self.amplihack_home)
            .env("PATH", script_path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("RECIPE_VAR_base_branch", "main")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1");
        match worktree_setup {
            Some(value) => command.env("RECIPE_VAR_worktree_setup", value),
            None => command.env_remove("RECIPE_VAR_worktree_setup"),
        };
        command
    }

    fn run(&self) -> Output {
        self.run_with(Some(&self.worktree_setup()))
    }
}

/// The settings that turn the combined review on, with `threshold` as the
/// TOML value of the line limit.
fn enabled(threshold: &str) -> String {
    format!("[review]\ncombined_small_diff = true\ncombined_diff_threshold = {threshold}\n")
}

/// Assert the script exited 0 and printed exactly `want` on stdout.
fn assert_answer(output: &Output, want: &str, why: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "{why}: a review mode never stops the phase, so the step exits 0. \
         exit {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code()
    );
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        [want],
        "{why}\nstderr:\n{stderr}"
    );
}

#[test]
fn the_setting_is_off_by_default_and_the_passes_run_separately() {
    let case = Case::new(10);
    assert_answer(
        &case.run(),
        "false",
        "with no setting, a 10-line diff still gets its three separate passes",
    );
}

#[test]
fn a_diff_at_the_threshold_gets_one_combined_review() {
    let case = Case::new(100);
    case.user_config(&enabled("100"));
    assert_answer(
        &case.run(),
        "true",
        "100 changed lines against a threshold of 100 counts as small",
    );
}

#[test]
fn a_diff_over_the_threshold_gets_separate_passes() {
    let case = Case::new(150);
    case.user_config(&enabled("100"));
    assert_answer(
        &case.run(),
        "false",
        "150 changed lines against a threshold of 100 is not small",
    );
}

#[test]
fn a_threshold_that_is_not_a_number_falls_back_to_200() {
    let case = Case::new(150);
    case.user_config(&enabled("\"lots\""));
    let output = case.run();
    assert_answer(
        &output,
        "true",
        "a threshold that is not a whole number is dropped and the default of 200 \
         applies, under which 150 lines is small",
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("threshold 200"),
        "the step says which threshold it measured against. stderr:\n{stderr}"
    );
}

#[test]
fn with_no_worktree_to_measure_the_passes_run_separately() {
    let case = Case::new(10);
    case.user_config(&enabled("100"));
    let output = case.run_with(None);
    assert_answer(
        &output,
        "false",
        "with RECIPE_VAR_worktree_setup unset there is no diff to measure, so the \
         step fails closed to separate passes",
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("worktree"),
        "the reason names the missing worktree. stderr:\n{stderr}"
    );
}

#[test]
fn a_setting_in_the_repository_under_review_is_ignored() {
    // `review.*` is user-only: a cloned repository must not replace its own
    // security and philosophy passes with one generalist pass.
    let case = Case::new(10);
    case.project_config(&enabled("100"));
    assert_answer(
        &case.run(),
        "false",
        "a project-file setting does not turn the combined review on",
    );
}

#[test]
fn a_helper_planted_in_the_repository_under_review_is_never_run() {
    // The script finds `worktree_setup_path.sh` under `$AMPLIHACK_HOME`,
    // `~/.copilot` and `~/.amplihack` only. A copy in the repository under
    // review, in the step's working directory or under `REPO_PATH` is the
    // repository's own code and must not run with the step's rights.
    let case = Case::new(10);
    case.user_config(&enabled("100"))
        .helper_only_in_home_amplihack();
    let marker = case.home.parent().unwrap().join("planted-helper-ran");
    let planted = format!(
        "#!/usr/bin/env bash\ntouch '{}'\nexit 0\n",
        marker.display()
    );
    let repo_path = case.home.parent().unwrap().join("repo-path");
    for root in [&case.repo, &repo_path] {
        let tools = root.join("amplifier-bundle").join("tools");
        fs::create_dir_all(&tools).unwrap();
        private_file(&tools.join(WORKTREE_TOOL), &planted);
        fs::set_permissions(tools.join(WORKTREE_TOOL), fs::Permissions::from_mode(0o700)).unwrap();
    }

    let output = case
        .command(Some(&case.worktree_setup()))
        .env("REPO_PATH", &repo_path)
        .output()
        .expect("run the review-mode script");

    assert!(
        !marker.exists(),
        "a helper planted in the repository or under REPO_PATH ran"
    );
    assert_answer(
        &output,
        "true",
        "the installed helper in ~/.amplihack found the worktree, and 10 lines \
         against a threshold of 100 is small",
    );
}
