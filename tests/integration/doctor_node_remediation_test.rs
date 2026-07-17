//! Node.js remediation CLI contracts for Copilot CLI compatibility.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn amplihack_bin() -> PathBuf {
    // Hard `env!` (not `var_os` with a fallback): Cargo builds the binary as a
    // prerequisite of this [[test]] target and exposes its exact path, so there
    // is no build race and no stale `target/debug` fallback to break under
    // release / cross profiles.
    PathBuf::from(env!("CARGO_BIN_EXE_amplihack"))
}

fn write_executable(path: &Path, content: &str) {
    let mut file = std::fs::File::create(path).expect("create executable");
    file.write_all(content.as_bytes())
        .expect("write executable");
    drop(file);
    let mut perms = std::fs::metadata(path).expect("metadata").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).expect("chmod executable");
}

fn run_with_fake_node(node_version_output: &str, args: &[&str]) -> Output {
    let temp = tempfile::tempdir().expect("tempdir");
    let bin_dir = temp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("mkdir bin");
    write_executable(
        &bin_dir.join("node"),
        &format!("#!/bin/sh\nprintf '%s\\n' '{node_version_output}'\n"),
    );
    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    Command::new(amplihack_bin())
        .args(args)
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .env("PATH", path)
        .output()
        .expect("run amplihack")
}

#[test]
fn doctor_help_exposes_node_and_copilot_ensure_node_paths() {
    let output = Command::new(amplihack_bin())
        .args(["doctor", "--help"])
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .output()
        .expect("run amplihack doctor --help");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "doctor --help must succeed; stderr:\n{stderr}"
    );
    assert!(stdout.contains("node"), "doctor help must list `node`");
    assert!(
        stdout.contains("copilot"),
        "doctor help must list `copilot` diagnostics"
    );
    assert!(
        stdout.contains("--ensure-node") || stdout.contains("--ensure"),
        "doctor help must expose explicit Node remediation flags"
    );
}

#[test]
fn doctor_node_accepts_node_24_or_newer() {
    let output = run_with_fake_node("v24.1.0", &["doctor", "node"]);
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "`doctor node` must pass for Node v24+; output:\n{combined}"
    );
    assert!(combined.contains("v24.1.0"));
    assert!(combined.contains(">=24.0.0"));
}

#[test]
fn doctor_node_reports_manual_remediation_for_old_node_without_ensure() {
    let output = run_with_fake_node("v22.11.0", &["doctor", "node"]);
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "`doctor node` must fail visibly for old Node; output:\n{combined}"
    );
    assert!(combined.contains("Required: >=24.0.0"));
    assert!(combined.contains("nvm install 24"));
    assert!(combined.contains("amplihack doctor node"));
}

#[test]
fn doctor_node_ensure_does_not_mutate_when_safe_manager_is_absent() {
    let output = run_with_fake_node("v22.11.0", &["doctor", "node", "--ensure"]);
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "`doctor node --ensure` must fail with instructions when no safe manager is detected; output:\n{combined}"
    );
    assert!(combined.contains("Automatic install was not attempted"));
    assert!(combined.contains("nvm install 24"));
}

#[test]
fn doctor_copilot_ensure_node_delegates_to_node_remediation() {
    let output = run_with_fake_node("not-a-version", &["doctor", "copilot", "--ensure-node"]);
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "`doctor copilot --ensure-node` must run Node validation before Copilot readiness; output:\n{combined}"
    );
    assert!(combined.contains("Node.js"));
    assert!(combined.contains("malformed") || combined.contains("invalid"));
}
