//! Behavioral red-phase contracts. No invented production API or source scans.
//! All launched CLI/Git children receive disposable homes and no inherited Git config.
#![cfg(unix)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::OnceLock,
};

#[path = "issue_2096_git_recording.rs"]
mod git_recording;
#[path = "issue_2096_review_isolation.rs"]
mod review_isolation;

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
    home: PathBuf,
    git: PathBuf,
}

fn probe() -> &'static Path {
    static PROBE: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    &PROBE
        .get_or_init(|| {
            let dir = tempfile::tempdir().unwrap();
            let bin = dir.path().join("recipe-runner-rs");
            let result = Command::new("rustc")
                .arg(
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("tests/fixtures/issue_2096/runner.rs"),
                )
                .args(["--edition=2024", "-o"])
                .arg(&bin)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "probe compile: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            (dir, bin)
        })
        .1
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        let repo = root.join("caller");
        let home = root.join("home");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&home).unwrap();
        let git = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|p| p.join("git"))
            .find(|p| p.is_file())
            .unwrap();
        let f = Self {
            _temp: temp,
            root,
            repo,
            home,
            git,
        };
        f.git(&["init", "-q"]);
        f.git(&["config", "user.name", "Isolation Fixture"]);
        f.git(&["config", "user.email", "fixture@example.invalid"]);
        fs::write(f.repo.join("tracked"), b"base\n").unwrap();
        f.git(&["add", "tracked"]);
        f.git(&["commit", "-qm", "fixture"]);
        fs::write(f.root.join("recipe.yaml"), "name: issue-2096\nsteps: []\n").unwrap();
        f
    }

    fn environment(&self, c: &mut Command) {
        c.env_clear()
            .env("PATH", "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin")
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

    fn git(&self, args: &[&str]) -> Vec<u8> {
        let mut c = Command::new(&self.git);
        self.environment(&mut c);
        let out = c.current_dir(&self.repo).args(args).output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }

    fn command(&self, label: &str) -> Command {
        let mut c =
            crate::isolated_amplihack::isolated(crate::support::amplihack_bin::amplihack_bin());
        self.environment(&mut c);
        c.env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
            .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
            .env("AMPLIHACK_WORK_ROOT", self.root.join("work"))
            .env("RECIPE_RUNNER_RS_PATH", probe())
            .env("ISSUE_2096_OBSERVATIONS", self.root.join(label))
            .env("ISSUE_2096_GIT", &self.git)
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .current_dir(&self.repo)
            .args(["recipe", "run"])
            .arg(self.root.join("recipe.yaml"))
            .arg("-w")
            .arg(&self.repo);
        c
    }

    fn run(&self, label: &str) -> Output {
        self.command(label).output().unwrap()
    }
    fn observed(&self, label: &str, name: &str) -> Vec<u8> {
        fs::read(self.root.join(label).join(name)).unwrap()
    }
    fn snapshot(&self) -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) {
        (
            fs::read(self.repo.join(".git/index")).unwrap(),
            self.git(&["rev-parse", "HEAD"]),
            fs::read(self.repo.join("tracked")).unwrap(),
            fs::read(self.repo.join(".git/config")).unwrap(),
        )
    }
}

fn succeeds(out: &Output) {
    assert!(
        out.status.success(),
        "status {}\nstdout {}\nstderr {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn first_executable_step_has_private_detached_checkout() {
    let f = Fixture::new();
    succeeds(&f.run("one"));
    assert_ne!(
        f.observed("one", "cwd"),
        f.repo.as_os_str().as_encoded_bytes()
    );
    assert_eq!(f.observed("one", "head"), f.git(&["rev-parse", "HEAD"]));
    assert!(
        f.observed("one", "branch").is_empty(),
        "execution HEAD must be detached"
    );
}

#[test]
fn first_step_can_stage_without_changing_caller_index_head_files_or_config() {
    let f = Fixture::new();
    let before = f.snapshot();
    succeeds(
        &f.command("stage")
            .env("ISSUE_2096_STAGE", "1")
            .output()
            .unwrap(),
    );
    assert_eq!(f.snapshot(), before, "caller state is read-only");
    assert!(!f.repo.join("run-output").exists());
}

#[test]
fn staged_unstaged_untracked_and_binary_inputs_are_preserved_separately() {
    let f = Fixture::new();
    fs::write(f.repo.join("tracked"), b"staged\n").unwrap();
    f.git(&["add", "tracked"]);
    fs::write(f.repo.join("tracked"), b"unstaged\n").unwrap();
    fs::write(f.repo.join("untracked input"), b"input\n").unwrap();
    fs::write(f.repo.join("binary"), [0, 255, 10, 128]).unwrap();
    let before = f.snapshot();
    succeeds(&f.run("inputs"));
    assert_eq!(f.observed("inputs", "staged"), b"staged\n");
    assert_eq!(f.observed("inputs", "tracked"), b"unstaged\n");
    assert_eq!(f.observed("inputs", "untracked input"), b"input\n");
    assert_eq!(f.observed("inputs", "binary"), [0, 255, 10, 128]);
    assert_eq!(f.snapshot(), before);
    assert_ne!(
        f.observed("inputs", "cwd"),
        f.repo.as_os_str().as_encoded_bytes()
    );
}

#[test]
fn concurrent_runs_at_same_head_have_distinct_directories_and_indexes() {
    let f = Fixture::new();
    // Spawn both before waiting; no timers or polling windows.
    let one = f.command("one").spawn().unwrap();
    let two = f.command("two").spawn().unwrap();
    succeeds(&one.wait_with_output().unwrap());
    succeeds(&two.wait_with_output().unwrap());
    assert_ne!(f.observed("one", "cwd"), f.observed("two", "cwd"));
    assert_ne!(f.observed("one", "index"), f.observed("two", "index"));
}

#[test]
fn inherited_alternate_index_cannot_route_execution_to_caller() {
    let f = Fixture::new();
    let index = f.repo.join(".git/index");
    let before = fs::read(&index).unwrap();
    let out = f
        .command("routing")
        .env("GIT_INDEX_FILE", &index)
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
    assert_eq!(fs::read(index).unwrap(), before);
    for key in [
        "GIT_DIR",
        "GIT_INDEX_FILE",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_CONFIG_COUNT",
    ] {
        assert!(
            !f.root.join("routing").join(key).exists(),
            "inherited {key} reached runner"
        );
    }
}

#[test]
fn unborn_head_fails_before_launch() {
    let f = Fixture::new();
    f.git(&["checkout", "--orphan", "unborn"]);
    let out = f.run("unborn");
    assert!(!out.status.success(), "unborn HEAD must fail closed");
    assert!(
        !f.root.join("unborn/cwd").exists(),
        "runner started before validation"
    );
}

#[test]
fn unsupported_assume_unchanged_input_fails_before_launch() {
    let f = Fixture::new();
    f.git(&["update-index", "--assume-unchanged", "tracked"]);
    fs::write(f.repo.join("tracked"), "hidden edit").unwrap();
    let out = f.run("unsupported");
    assert!(
        !out.status.success(),
        "unsupported index flags must fail explicitly"
    );
    assert!(!f.root.join("unsupported/cwd").exists());
}

#[test]
fn non_git_execution_keeps_original_directory() {
    let f = Fixture::new();
    fs::remove_dir_all(f.repo.join(".git")).unwrap();
    succeeds(&f.run("non-git"));
    assert_eq!(
        f.observed("non-git", "cwd"),
        f.repo.as_os_str().as_encoded_bytes()
    );
}

#[test]
fn repository_context_path_maps_into_execution_checkout() {
    let f = Fixture::new();
    succeeds(
        &f.command("context")
            .arg("-c")
            .arg(format!("repo_path={}", f.repo.display()))
            .output()
            .unwrap(),
    );
    assert_eq!(
        f.observed("context", "repo_path"),
        f.observed("context", "cwd")
    );
    assert_ne!(
        f.observed("context", "cwd"),
        f.repo.as_os_str().as_encoded_bytes()
    );
}

#[test]
fn untracked_symlink_into_caller_is_rejected_before_execution() {
    let f = Fixture::new();
    std::os::unix::fs::symlink(&f.repo, f.repo.join("escape")).unwrap();
    let out = f.run("escape");
    assert!(
        !out.status.success(),
        "unsafe symlink inputs require explicit rejection"
    );
    assert!(!f.root.join("escape/cwd").exists());
}
