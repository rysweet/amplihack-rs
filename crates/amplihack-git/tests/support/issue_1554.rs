//! Disposable real-Git fixtures. Isolate profiles before production resolution.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const CHILD: &str = "AMPLIHACK_1554_CONTRACT_CHILD";

pub fn isolated(name: &str, body: impl FnOnce()) {
    if std::env::var_os(CHILD).as_deref() == Some(std::ffi::OsStr::new(name)) {
        body();
        return;
    }
    // Do not inherit a TMPDIR below a real agent profile.
    let profile = tempfile::tempdir_in("/var/tmp").unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env(CHILD, name)
        .env("TMPDIR", profile.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([name, "--exact", "--nocapture", "--test-threads=1"]);
    for key in [
        "HOME",
        "XDG_CONFIG_HOME",
        "AMPLIHACK_HOME",
        "CLAUDE_CONFIG_DIR",
        "CODEX_HOME",
        "AMPLIHACK_CLAUDE_PROFILE_ROOT",
    ] {
        let path = profile.path().join(key);
        fs::create_dir(&path).unwrap();
        command.env(key, path);
    }
    let out = command.output().unwrap();
    success(&out);
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("test result: ok. 1 passed"),
        "the exact child filter must execute one test: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

pub fn set(key: &str, value: impl AsRef<std::ffi::OsStr>) {
    assert!(std::env::var_os(CHILD).is_some(), "requires isolated child");
    // SAFETY: only this test runs in the child; call before starting workers.
    unsafe { std::env::set_var(key, value) };
}

pub fn remove(key: &str) {
    assert!(std::env::var_os(CHILD).is_some(), "requires isolated child");
    // SAFETY: as in set().
    unsafe { std::env::remove_var(key) };
}

pub fn success(out: &Output) {
    assert!(
        out.status.success(),
        "{}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

pub fn git(root: &Path, args: &[&str]) -> Vec<u8> {
    let out = amplihack_git::command_in(root).args(args).output().unwrap();
    success(&out);
    out.stdout
}

pub fn text(root: &Path, args: &[&str]) -> String {
    String::from_utf8(git(root, args))
        .unwrap()
        .trim()
        .to_owned()
}

pub fn write(path: impl AsRef<Path>, content: impl AsRef<[u8]>) {
    fs::create_dir_all(path.as_ref().parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

pub fn authority(requirements: serde_json::Value, design: serde_json::Value) {
    set("RECIPE_VAR_final_requirements", requirements.to_string());
    set("RECIPE_VAR_design_spec", design.to_string());
}

pub fn source_authority(paths: &[&str]) {
    authority(
        serde_json::json!({"version": 1, "task": "1554", "permitted_source_paths": paths}),
        serde_json::json!({"version": 1, "task": "1554", "files_to_change": paths}),
    );
}

pub struct Repo {
    pub temp: tempfile::TempDir,
    pub root: PathBuf,
    pub remote: PathBuf,
}

impl Repo {
    pub fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("source");
        let remote = temp.path().join("destination.git");
        fs::create_dir(&root).unwrap();
        git(&root, &["init", "--quiet", "--template=", "-b", "main"]);
        git(&root, &["config", "user.name", "Test Fixture"]);
        git(&root, &["config", "user.email", "fixture@example.invalid"]);
        git(&root, &["config", "commit.gpgsign", "false"]);
        write(root.join("source.txt"), "initial source\n");
        write(root.join("protected.txt"), "initial user source\n");
        git(&root, &["add", "--", "source.txt", "protected.txt"]);
        git(&root, &["commit", "--quiet", "-m", "initial source"]);
        git(
            &root,
            &[
                "init",
                "--bare",
                "--quiet",
                "--template=",
                remote.to_str().unwrap(),
            ],
        );
        git(
            &root,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git(
            &root,
            &["push", "--quiet", "--set-upstream", "origin", "main"],
        );
        git(&root, &["checkout", "--quiet", "-b", "feat/task"]);
        Self { temp, root, remote }
    }

    pub fn linked(&self) -> PathBuf {
        let path = self.temp.path().join("linked");
        git(
            &self.root,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "feat/1554",
                path.to_str().unwrap(),
            ],
        );
        path
    }

    pub fn commit(&self, path: &str, content: &str) -> String {
        write(self.root.join(path), content);
        git(&self.root, &["--literal-pathspecs", "add", "--", path]);
        git(&self.root, &["commit", "--quiet", "-m", "fixture change"]);
        text(&self.root, &["rev-parse", "HEAD"])
    }

    pub fn inventory(&self, root: &Path) -> Vec<u8> {
        let out = amplihack_git::command_in(root)
            .env_remove("HOME")
            .env_remove("XDG_CONFIG_HOME")
            .args(["ls-files", "--others", "--exclude-standard", "-z"])
            .output()
            .unwrap();
        success(&out);
        out.stdout
    }
}
