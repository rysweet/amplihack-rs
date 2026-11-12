//! Issue #2096: a recipe run works in a detached checkout of its own, never
//! in the caller's checkout.
//!
//! Runs the real `amplihack recipe run` with a stub recipe runner (a shell
//! script, through `RECIPE_RUNNER_RS_PATH`) that records where it was started
//! and what git sees there, and can stage and commit a file in its working
//! directory. Every child gets a disposable home and no global or system git
//! configuration; `PATH` is the test's own.
#![cfg(unix)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

#[path = "issue_2096_checkout_units.rs"]
mod checkout_units;
#[path = "issue_2096_contract_support.rs"]
mod contract_support;
#[path = "issue_2096_input_preservation.rs"]
mod input_preservation;
#[path = "issue_2096_ownership_lifecycle.rs"]
mod ownership_lifecycle;
#[path = "issue_2096_real_hooks.rs"]
mod real_hooks;
#[path = "issue_2096_recording.rs"]
mod recording;
#[path = "issue_2096_recording_coverage.rs"]
mod recording_coverage;
#[path = "issue_2096_recording_failures.rs"]
mod recording_failures;
#[path = "issue_2096_review_isolation.rs"]
mod review_isolation;

/// The stub runner. It enters the runner's `-C`, records what it finds and,
/// with `ISSUE_2096_STAGE` set, stages and commits a file there. With
/// `ISSUE_2096_FAIL` set it then exits non-zero. It writes the
/// `--recovery-descriptor` argument, when given, to `descriptor`.
const STUB_RUNNER: &str = r#"#!/bin/sh
set -eu
out="$ISSUE_2096_OBSERVATIONS"
mkdir -p "$out"
printf '%s' "$(pwd -P)" > "$out/startup_cwd"
dir=""
while [ $# -gt 0 ]; do
  case "$1" in
    -C) dir="$2"; shift 2 ;;
    --recovery-descriptor) printf "%s" "$2" > "$out/descriptor"; shift 2 ;;
    *) shift ;;
  esac
done
cd "$dir"
printf '%s' "$(pwd -P)" > "$out/cwd"
if ! git rev-parse --git-dir >/dev/null 2>&1; then
  printf '{"recipe_name":"issue-2096","success":true,"step_results":[],"context":{}}\n'
  exit 0
fi
git rev-parse HEAD > "$out/head"
git symbolic-ref -q HEAD > "$out/branch" || true
git rev-parse --path-format=absolute --git-path index > "$out/index"
printf '%s' "${AMPLIHACK_RUN_CHECKOUT:-}" > "$out/run_checkout"
git worktree list --porcelain | grep -c '^worktree ' > "$out/worktrees" || true
for v in GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR GIT_CONFIG_COUNT GIT_CONFIG_KEY_0; do
  if eval "[ -n \"\${$v+set}\" ]"; then : > "$out/$v"; fi
done
if [ -n "${ISSUE_2096_STAGE:-}" ]; then
  printf 'private output\n' > run-output
  git add -- run-output
  git commit -qm "run's own commit"
fi
if [ -n "${ISSUE_2096_FAIL:-}" ]; then
  echo "issue-2096 stub: failing on request" >&2
  exit 1
fi
printf '{"recipe_name":"issue-2096","success":true,"step_results":[],"context":{}}\n'
"#;

pub(crate) struct Fixture {
    _temp: tempfile::TempDir,
    pub(crate) root: PathBuf,
    pub(crate) repo: PathBuf,
    home: PathBuf,
    runner: PathBuf,
}

impl Fixture {
    pub(crate) fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        let repo = root.join("caller");
        let home = root.join("home");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&home).unwrap();
        let runner = root.join("recipe-runner-rs");
        crate::support::exec_stub::write_executable(&runner, STUB_RUNNER, 0o755);
        let f = Self {
            _temp: temp,
            root,
            repo,
            home,
            runner,
        };
        f.git(&["init", "-q"]);
        f.git(&["config", "user.name", "Isolation Fixture"]);
        f.git(&["config", "user.email", "fixture@example.invalid"]);
        fs::write(f.repo.join("tracked"), b"base\n").unwrap();
        fs::create_dir_all(f.repo.join("sub")).unwrap();
        fs::write(f.repo.join("sub/file"), b"sub\n").unwrap();
        f.git(&["add", "tracked", "sub/file"]);
        f.git(&["commit", "-qm", "fixture"]);
        fs::write(
            f.root.join("recipe.yaml"),
            "name: issue-2096\nsteps:\n  - id: stub\n    type: bash\n    command: \"true\"\n",
        )
        .unwrap();
        f
    }

    /// A disposable home and no global or system git configuration; the
    /// test's own `PATH`.
    pub(crate) fn environment(&self, c: &mut Command) {
        c.env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &self.home)
            .env("TMPDIR", &self.root)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("CLAUDE_CONFIG_DIR", self.home.join(".claude"))
            .env("CODEX_HOME", self.home.join(".codex"))
            .env("AMPLIHACK_HOME", self.home.join(".amplihack"))
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", self.home.join("profiles"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0");
    }

    pub(crate) fn git(&self, args: &[&str]) -> Vec<u8> {
        self.git_in(&self.repo, args)
    }

    pub(crate) fn git_in(&self, dir: &Path, args: &[&str]) -> Vec<u8> {
        let mut c = amplihack_git::command();
        self.environment(&mut c);
        let out = c.current_dir(dir).args(args).output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }

    fn command_in(&self, label: &str, dir: &Path) -> Command {
        self.command_for_recipe(label, dir, &self.root.join("recipe.yaml"))
    }

    fn command_for_recipe(&self, label: &str, dir: &Path, recipe: &Path) -> Command {
        let mut c =
            crate::isolated_amplihack::isolated(crate::support::amplihack_bin::amplihack_bin());
        self.environment(&mut c);
        c.env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
            .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
            .env("AMPLIHACK_WORK_ROOT", self.root.join("work"))
            .env("RECIPE_RUNNER_RS_PATH", &self.runner)
            .env("ISSUE_2096_OBSERVATIONS", self.root.join(label))
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .current_dir(dir)
            .args(["recipe", "run"])
            .arg(recipe)
            .arg("-w")
            .arg(dir);
        c
    }

    fn command(&self, label: &str) -> Command {
        self.command_in(label, &self.repo)
    }

    fn run(&self, label: &str) -> Output {
        self.command(label).output().unwrap()
    }

    fn observed(&self, label: &str, name: &str) -> Vec<u8> {
        fs::read(self.root.join(label).join(name))
            .unwrap_or_else(|e| panic!("observation {label}/{name}: {e}"))
    }

    /// The caller's index, HEAD, a tracked file and its git config.
    fn snapshot(&self) -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) {
        (
            fs::read(self.repo.join(".git/index")).unwrap(),
            self.git(&["rev-parse", "HEAD"]),
            fs::read(self.repo.join("tracked")).unwrap(),
            fs::read(self.repo.join(".git/config")).unwrap(),
        )
    }

    fn worktrees(&self) -> String {
        String::from_utf8(self.git(&["worktree", "list", "--porcelain"])).unwrap()
    }
}

pub(crate) fn succeeds(out: &Output) {
    assert!(
        out.status.success(),
        "status {}\nstdout {}\nstderr {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn runner_starts_in_a_private_detached_checkout_at_caller_head() {
    let f = Fixture::new();
    succeeds(&f.run("one"));
    let cwd = f.observed("one", "cwd");
    assert_eq!(
        f.observed("one", "startup_cwd"),
        cwd,
        "runner initialization must already be in the private checkout"
    );
    assert_ne!(cwd, f.repo.as_os_str().as_encoded_bytes());
    assert!(
        !Path::new(std::str::from_utf8(&cwd).unwrap()).starts_with(&f.repo),
        "the run's checkout must not be inside the caller's checkout"
    );
    assert_eq!(f.observed("one", "head"), f.git(&["rev-parse", "HEAD"]));
    assert!(
        f.observed("one", "branch").is_empty(),
        "the run's checkout must be detached"
    );
    assert_eq!(f.observed("one", "run_checkout"), cwd);
}

#[test]
fn a_step_that_stages_and_commits_leaves_caller_index_head_files_and_config_unchanged() {
    let f = Fixture::new();
    let before = f.snapshot();
    succeeds(
        &f.command("stage")
            .env("ISSUE_2096_STAGE", "1")
            .output()
            .unwrap(),
    );
    assert_eq!(f.snapshot(), before, "the caller's checkout is read-only");
    assert!(!f.repo.join("run-output").exists());
}

#[test]
fn the_run_checkout_is_removed_when_the_run_ends() {
    let f = Fixture::new();
    let before = f.worktrees();
    succeeds(
        &f.command("gone")
            .env("ISSUE_2096_STAGE", "1")
            .output()
            .unwrap(),
    );
    let cwd = String::from_utf8(f.observed("gone", "cwd")).unwrap();
    assert!(!Path::new(&cwd).exists(), "{cwd} was left behind");
    assert_eq!(f.worktrees(), before, "the worktree record was left behind");
}

#[test]
fn concurrent_runs_at_the_same_head_have_distinct_directories_and_indexes() {
    let f = Fixture::new();
    // Each runner acknowledges readiness and blocks on a socket. Neither
    // can finish until both have reached the execution boundary.
    let one = contract_support::Running::start(&f, "one");
    let two = contract_support::Running::start(&f, "two");
    assert_ne!(f.observed("one", "cwd"), f.observed("two", "cwd"));
    assert_ne!(f.observed("one", "index"), f.observed("two", "index"));
    succeeds(&one.finish());
    succeeds(&two.finish());
}

#[test]
fn inherited_git_routing_variables_do_not_reach_the_runner() {
    let f = Fixture::new();
    let before = f.snapshot();
    let out = f
        .command("routing")
        .env("GIT_INDEX_FILE", f.repo.join(".git/index"))
        .env("GIT_DIR", f.repo.join(".git"))
        .env("GIT_WORK_TREE", &f.repo)
        .env("GIT_COMMON_DIR", f.repo.join(".git"))
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.bare")
        .env("GIT_CONFIG_VALUE_0", "false")
        .env("ISSUE_2096_STAGE", "1")
        .output()
        .unwrap();
    succeeds(&out);
    assert_eq!(f.snapshot(), before);
    for key in [
        "GIT_DIR",
        "GIT_INDEX_FILE",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_KEY_0",
    ] {
        assert!(
            !f.root.join("routing").join(key).exists(),
            "inherited {key} reached the runner"
        );
    }
}

#[test]
fn a_subdirectory_working_dir_maps_to_the_same_subdirectory_of_the_run_checkout() {
    let f = Fixture::new();
    let sub = f.repo.join("sub");
    succeeds(&f.command_in("sub", &sub).output().unwrap());
    let cwd = PathBuf::from(String::from_utf8(f.observed("sub", "cwd")).unwrap());
    let root = PathBuf::from(String::from_utf8(f.observed("sub", "run_checkout")).unwrap());
    assert_eq!(cwd, root.join("sub"));
}

#[test]
fn an_unborn_head_stops_the_run_before_the_runner_starts() {
    let f = Fixture::new();
    f.git(&["checkout", "-q", "--orphan", "unborn"]);
    let out = f.run("unborn");
    assert!(!out.status.success(), "an unborn HEAD must stop the run");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("unborn"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !f.root.join("unborn/cwd").exists(),
        "the runner started without a checkout of its own"
    );
}

#[test]
fn a_directory_outside_git_is_used_as_given() {
    let f = Fixture::new();
    fs::remove_dir_all(f.repo.join(".git")).unwrap();
    succeeds(&f.run("non-git"));
    assert_eq!(
        f.observed("non-git", "cwd"),
        f.repo.as_os_str().as_encoded_bytes()
    );
}

#[test]
fn a_failed_run_still_removes_its_checkout() {
    let f = Fixture::new();
    let before = f.worktrees();
    let out = f
        .command("failed")
        .env("ISSUE_2096_STAGE", "1")
        .env("ISSUE_2096_FAIL", "1")
        .output()
        .unwrap();
    assert!(!out.status.success(), "the stub runner was told to fail");
    let cwd = String::from_utf8(f.observed("failed", "cwd")).unwrap();
    assert_ne!(cwd.as_bytes(), f.repo.as_os_str().as_encoded_bytes());
    assert!(
        !Path::new(&cwd).exists(),
        "{cwd} was left behind by a failed run"
    );
    assert_eq!(
        f.worktrees(),
        before,
        "a failed run left its worktree record"
    );
}

#[test]
fn git_config_parameters_naming_the_caller_cannot_reach_its_index() {
    let f = Fixture::new();
    let before = f.snapshot();
    // `core.worktree` and an index path that both name the caller: a runner
    // that kept them would stage into the caller's checkout.
    let params = format!(
        "'core.worktree'='{}' 'core.hooksPath'='{}'",
        f.repo.display(),
        f.root.join("no-hooks").display()
    );
    let out = f
        .command("params")
        .env("GIT_CONFIG_PARAMETERS", params)
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.worktree")
        .env("GIT_CONFIG_VALUE_0", &f.repo)
        .env("GIT_INDEX_FILE", f.repo.join(".git/index"))
        .env("ISSUE_2096_STAGE", "1")
        .output()
        .unwrap();
    succeeds(&out);
    assert_eq!(
        f.snapshot(),
        before,
        "the caller's index, HEAD, files or config changed"
    );
    assert!(!f.repo.join("run-output").exists());
}

/// What an ancestor run would have made: a detached linked worktree at the
/// caller's HEAD.
fn ancestor_run_checkout(f: &Fixture) -> PathBuf {
    let outer = f.root.join("outer-run-checkout");
    f.git(&[
        "worktree",
        "add",
        "-q",
        "--detach",
        outer.to_str().unwrap(),
        "HEAD",
    ]);
    fs::canonicalize(outer).unwrap()
}

#[test]
fn an_independent_launch_inside_a_marked_checkout_owns_fresh_resources() {
    let f = Fixture::new();
    let outer = ancestor_run_checkout(&f);
    let before = f.worktrees();
    let out = f
        .command_in("nested", &outer)
        .env("AMPLIHACK_RUN_CHECKOUT", &outer)
        .output()
        .unwrap();
    succeeds(&out);
    assert_ne!(
        f.observed("nested", "cwd"),
        outer.as_os_str().as_encoded_bytes()
    );
    assert_eq!(
        f.observed("nested", "run_checkout"),
        f.observed("nested", "cwd"),
        "an environment marker does not establish shared logical ownership"
    );
    assert!(
        outer.exists(),
        "a nested run never removes its ancestor's checkout"
    );
    assert_eq!(f.worktrees(), before);
}

#[test]
fn an_inherited_run_checkout_that_is_the_main_checkout_is_ignored() {
    let f = Fixture::new();
    let before = f.snapshot();
    let out = f
        .command("forged-main")
        .env("AMPLIHACK_RUN_CHECKOUT", &f.repo)
        .env("ISSUE_2096_STAGE", "1")
        .output()
        .unwrap();
    succeeds(&out);
    let cwd = f.observed("forged-main", "cwd");
    assert_ne!(
        cwd,
        f.repo.as_os_str().as_encoded_bytes(),
        "the run used the main checkout"
    );
    assert_eq!(f.observed("forged-main", "run_checkout"), cwd);
    assert_eq!(f.snapshot(), before);
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("AMPLIHACK_RUN_CHECKOUT"),
        "a refused value is reported by name: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn an_inherited_run_checkout_on_a_branch_is_ignored() {
    let f = Fixture::new();
    let branch = f.root.join("branch-worktree");
    f.git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "topic",
        branch.to_str().unwrap(),
    ]);
    let branch = fs::canonicalize(branch).unwrap();
    let index = f.git_in(
        &branch,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
    );
    let index = PathBuf::from(String::from_utf8(index).unwrap().trim());
    let index_before = fs::read(&index).unwrap();
    let head_before = f.git_in(&branch, &["rev-parse", "HEAD"]);

    let out = f
        .command_in("forged-branch", &branch)
        .env("AMPLIHACK_RUN_CHECKOUT", &branch)
        .env("ISSUE_2096_STAGE", "1")
        .output()
        .unwrap();
    succeeds(&out);
    assert_ne!(
        f.observed("forged-branch", "cwd"),
        branch.as_os_str().as_encoded_bytes(),
        "a branch worktree is not a run checkout"
    );
    assert_eq!(fs::read(&index).unwrap(), index_before);
    assert_eq!(f.git_in(&branch, &["rev-parse", "HEAD"]), head_before);
    assert!(!branch.join("run-output").exists());
}

#[test]
fn a_dry_run_uses_the_callers_directory_and_makes_no_checkout() {
    let f = Fixture::new();
    let before = f.worktrees();
    let out = f.command("dry").arg("--dry-run").output().unwrap();
    succeeds(&out);
    assert_eq!(
        f.observed("dry", "cwd"),
        f.repo.as_os_str().as_encoded_bytes()
    );
    assert_eq!(
        String::from_utf8(f.observed("dry", "worktrees"))
            .unwrap()
            .trim(),
        "1"
    );
    assert_eq!(f.worktrees(), before);
    assert!(
        f.run_folders().is_empty(),
        "a dry run made run folders: {:?}",
        f.run_folders()
    );
    let mut recordings = Vec::new();
    find_named(&f.root.join("work"), "git-recording.json", &mut recordings);
    assert!(
        recordings.is_empty(),
        "a dry run wrote a git recording: {recordings:?}"
    );
}

impl Fixture {
    /// `<work root>/runs/*`, sorted. A missing `runs/` is empty; any other
    /// read error fails the test, so an isolation check never passes on a
    /// folder it could not read.
    fn run_folders(&self) -> Vec<PathBuf> {
        let runs = self.root.join("work/runs");
        let entries = match fs::read_dir(&runs) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(e) => panic!("read {}: {e}", runs.display()),
        };
        let mut dirs: Vec<PathBuf> = entries
            .map(|e| e.unwrap_or_else(|e| panic!("read {}: {e}", runs.display())))
            .map(|e| e.path())
            .collect();
        dirs.sort();
        dirs
    }

    /// The single run folder made since `before`.
    fn new_run_folder(&self, before: &[PathBuf]) -> PathBuf {
        let new: Vec<PathBuf> = self
            .run_folders()
            .into_iter()
            .filter(|d| !before.contains(d))
            .collect();
        assert_eq!(new.len(), 1, "expected one new run folder: {new:?}");
        new.into_iter().next().unwrap()
    }
}

/// Every file called `name` under `dir`, following no symlink. A missing
/// `dir` adds nothing; any other I/O error fails the test.
fn find_named(dir: &Path, name: &str, found: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => panic!("read {}: {e}", dir.display()),
    };
    for entry in entries {
        let entry = entry.unwrap_or_else(|e| panic!("read {}: {e}", dir.display()));
        let kind = entry
            .file_type()
            .unwrap_or_else(|e| panic!("stat {}: {e}", entry.path().display()));
        if kind.is_dir() {
            find_named(&entry.path(), name, found);
        } else if entry.file_name() == name {
            found.push(entry.path());
        }
    }
}

fn json_file(path: &Path) -> serde_json::Value {
    let bytes = fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

/// The recovery outcome the run's admission finished with, from
/// `<run folder>/recovery/tree.json`.
fn recovery_outcome(run: &Path) -> String {
    let tree = json_file(&run.join("recovery/tree.json"));
    tree["outcome"]
        .as_str()
        .unwrap_or_else(|| panic!("no outcome in {tree:#}"))
        .to_ascii_lowercase()
}

/// A prepare failure (unborn HEAD) happens after admission, so the
/// admission must be finished as a failure, not left running for the next
/// launch to resume.
#[test]
fn an_unborn_head_finishes_the_recovery_admission_as_failure() {
    let f = Fixture::new();
    f.git(&["checkout", "-q", "--orphan", "unborn"]);
    let before = f.run_folders();
    let out = f
        .command("unborn-admission")
        .args(["-c", "issue_number=2096"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "an unborn HEAD must stop the run");
    assert!(
        !f.root.join("unborn-admission/cwd").exists(),
        "the runner started"
    );
    let run = f.new_run_folder(&before);
    assert_eq!(recovery_outcome(&run), "failure");
    let report = json_file(&run.join("run.json"));
    assert_eq!(report["outcome"], "failure", "{report:#}");
    assert!(report["ended_at"].is_string(), "{report:#}");
}

/// A failed run releases in order: checkout, recording, admission,
/// workspace. Each one must have happened.
#[test]
fn a_failed_run_finishes_checkout_recording_admission_and_workspace() {
    let f = Fixture::new();
    let before = f.run_folders();
    let out = f
        .command("released")
        .args(["-c", "issue_number=2096"])
        .env("ISSUE_2096_FAIL", "1")
        .output()
        .unwrap();
    assert!(!out.status.success(), "the stub runner was told to fail");
    let cwd = String::from_utf8(f.observed("released", "cwd")).unwrap();
    assert!(!Path::new(&cwd).exists(), "{cwd} was left behind");
    let run = f.new_run_folder(&before);
    assert_eq!(recovery_outcome(&run), "failure");
    let report = json_file(&run.join("run.json"));
    assert_eq!(report["outcome"], "failure", "{report:#}");
    assert!(report["ended_at"].is_string(), "{report:#}");
    assert!(
        run.join("git-recording.json").is_file(),
        "the git recording was not finished before the workspace"
    );
}

/// A relaunch with the same identity after a failure gets a new private
/// checkout; the first one is gone and is never reused. The recovery
/// descriptor lives in the run folder, never inside the checkout, and the
/// caller's checkout is unchanged by either launch.
#[test]
fn a_relaunch_after_a_failure_gets_a_fresh_checkout_and_leaves_the_caller_unchanged() {
    let f = Fixture::new();
    let caller = f.snapshot();
    let worktrees = f.worktrees();

    let before = f.run_folders();
    let first = f
        .command("first")
        .args(["-c", "issue_number=2096"])
        .env("ISSUE_2096_STAGE", "1")
        .env("ISSUE_2096_FAIL", "1")
        .output()
        .unwrap();
    assert!(!first.status.success(), "the stub runner was told to fail");
    let first_run = f.new_run_folder(&before);
    let first_cwd = PathBuf::from(String::from_utf8(f.observed("first", "cwd")).unwrap());
    assert!(
        !first_cwd.exists(),
        "{} was left behind",
        first_cwd.display()
    );

    let before = f.run_folders();
    succeeds(
        &f.command("second")
            .args(["-c", "issue_number=2096"])
            .output()
            .unwrap(),
    );
    let second_run = f.new_run_folder(&before);
    let second_cwd = PathBuf::from(String::from_utf8(f.observed("second", "cwd")).unwrap());

    assert_ne!(
        first_cwd, second_cwd,
        "the relaunch reused the first checkout"
    );
    assert_ne!(
        second_cwd, f.repo,
        "the relaunch ran in the caller's checkout"
    );
    assert!(
        !second_cwd.exists(),
        "{} was left behind",
        second_cwd.display()
    );
    assert_eq!(
        f.observed("second", "head"),
        f.git(&["rev-parse", "HEAD"]),
        "the relaunch did not start at the caller's HEAD"
    );

    let first_id = json_file(&first_run.join("recovery/descriptor.json"))["identity"].clone();
    let second_id = json_file(&second_run.join("recovery/descriptor.json"))["identity"].clone();
    assert!(
        first_id.as_str().is_some_and(|s| !s.is_empty()),
        "the launch had no recovery identity: {first_id}"
    );
    assert_eq!(first_id, second_id, "both launches must share one identity");
    assert_eq!(recovery_outcome(&first_run), "failure");
    assert_eq!(recovery_outcome(&second_run), "success");

    for (label, run, cwd) in [
        ("first", &first_run, &first_cwd),
        ("second", &second_run, &second_cwd),
    ] {
        let descriptor = PathBuf::from(String::from_utf8(f.observed(label, "descriptor")).unwrap());
        assert!(
            descriptor.starts_with(run),
            "{label}: descriptor {} is not in the run folder {}",
            descriptor.display(),
            run.display()
        );
        assert!(
            !descriptor.starts_with(cwd),
            "{label}: descriptor {} is inside the checkout",
            descriptor.display()
        );
    }

    assert_eq!(
        f.snapshot(),
        caller,
        "a launch changed the caller's checkout"
    );
    assert_eq!(f.worktrees(), worktrees, "a launch left a worktree record");
}
