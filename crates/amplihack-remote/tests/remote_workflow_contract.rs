//! crates/amplihack-remote/tests/remote_workflow_contract.rs
//!
//! Issue #536 workflow contracts for Rust remote parity. These tests avoid real
//! Azure access; a fake azlin executable records whether provisioning/allocation
//! would have been attempted.

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use amplihack_remote::{VMOptions, execute_remote_workflow};

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvVarGuard {
    key: &'static str,
    old: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl Into<OsString>) -> Self {
        let old = std::env::var_os(key);
        unsafe { std::env::set_var(key, value.into()) };
        Self { key, old }
    }

    fn remove(key: &'static str) -> Self {
        let old = std::env::var_os(key);
        unsafe { std::env::remove_var(key) };
        Self { key, old }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.old {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

fn collect_python_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in
        fs::read_dir(dir).unwrap_or_else(|e| panic!("failed to read {}: {e}", dir.display()))
    {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            collect_python_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "py") {
            out.push(path);
        }
    }
}

fn run_git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(repo)
        .status()
        .unwrap_or_else(|e| panic!("failed to run git {args:?}: {e}"));
    assert!(
        status.success(),
        "git {args:?} failed in {}",
        repo.display()
    );
}

fn init_minimal_git_repo(repo: &Path) {
    fs::create_dir_all(repo.join(".claude")).unwrap();
    fs::write(repo.join(".claude/settings.json"), "{}\n").unwrap();
    fs::write(repo.join("README.md"), "# test repo\n").unwrap();
    run_git(repo, &["init", "-q"]);
    run_git(
        repo,
        &["config", "user.email", "remote-test@example.invalid"],
    );
    run_git(repo, &["config", "user.name", "Remote Contract Test"]);
    run_git(repo, &["add", "."]);
    run_git(repo, &["commit", "-q", "-m", "init"]);
}

fn install_fake_azlin(bin_dir: &Path) -> PathBuf {
    fs::create_dir_all(bin_dir).unwrap();
    let azlin = bin_dir.join("azlin");
    fs::write(
        &azlin,
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$AZLIN_MARKER"
case "$1" in
  --version) echo "azlin fake 0.0.0"; exit 0 ;;
  list)
    if [ "$2" = "--json" ]; then echo "[]"; fi
    exit 0
    ;;
  new|cp|connect|kill) exit 0 ;;
  *) exit 0 ;;
esac
"#,
    )
    .unwrap();
    let mut perms = fs::metadata(&azlin).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&azlin, perms).unwrap();
    azlin
}

#[test]
fn legacy_python_remote_files_are_deleted_after_native_port() {
    let legacy_remote = workspace_root()
        .join("amplifier-bundle")
        .join("tools")
        .join("amplihack")
        .join("remote");
    let mut python_files = Vec::new();
    collect_python_files(&legacy_remote, &mut python_files);
    python_files.sort();

    assert!(
        python_files.is_empty(),
        "issue #536 final implementation must delete every .py file under {}.\nRemaining files:\n{}",
        legacy_remote.display(),
        python_files
            .iter()
            .map(|p| format!(
                "  - {}",
                p.strip_prefix(workspace_root()).unwrap_or(p).display()
            ))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn workflow_validates_anthropic_api_key_before_azlin_provisioning_or_allocation() {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_minimal_git_repo(&repo);

    let marker = temp.path().join("azlin-was-called.log");
    let fake_bin = temp.path().join("bin");
    install_fake_azlin(&fake_bin);
    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let new_path = format!(
        "{}:{}",
        fake_bin.display(),
        PathBuf::from(old_path).display()
    );

    let _path_guard = EnvVarGuard::set("PATH", new_path);
    let _marker_guard = EnvVarGuard::set("AZLIN_MARKER", marker.as_os_str());
    let _api_key_guard = EnvVarGuard::remove("ANTHROPIC_API_KEY");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let result = runtime.block_on(execute_remote_workflow(
        &repo,
        "auto",
        "ship it",
        10,
        &VMOptions::default(),
        1,
        true,
    ));

    let err = match result {
        Ok(_) => panic!("missing ANTHROPIC_API_KEY must fail the workflow"),
        Err(err) => err,
    };
    assert!(
        err.to_string().contains("ANTHROPIC_API_KEY"),
        "credential validation should be the surfaced failure, got: {err}"
    );
    assert!(
        !marker.exists(),
        "ANTHROPIC_API_KEY must be validated before provisioning/allocation side effects.\nFake azlin calls:\n{}",
        fs::read_to_string(&marker).unwrap_or_default()
    );
}
