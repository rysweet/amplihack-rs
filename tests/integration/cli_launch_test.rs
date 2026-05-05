use std::path::PathBuf;
/// Integration tests: CLI launch flow smoke tests.
///
/// These tests exercise the top-level amplihack binary through its argument
/// parsing and command dispatch layer without requiring live external tools
/// (claude, copilot, etc.) to be installed.  They are smoke-level tests
/// that confirm the binary is built, parses flags correctly, and produces
/// expected exit codes / output for basic invocations.
use std::process::Command;

/// Path to the compiled amplihack binary.
fn amplihack_bin() -> PathBuf {
    // Use the debug build during tests for speed; CI uses --release separately.
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // Walk up from tests/ to the workspace root, then into target/.
    path.pop(); // tests/
    path.pop(); // workspace root
    path.push("target/debug/amplihack");
    path
}

/// Assert that a Command produces the expected exit status.
fn assert_exit(cmd: &mut Command, expect_success: bool) {
    cmd.env("AMPLIHACK_SKIP_AUTO_INSTALL", "1");
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("Failed to run command: {e}"));
    if expect_success {
        assert!(status.success(), "Expected success, got: {status}");
    } else {
        assert!(!status.success(), "Expected failure, got: {status}");
    }
}

// ---------------------------------------------------------------------------
// --help and --version smoke tests
// ---------------------------------------------------------------------------

#[test]
fn help_exits_zero() {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }
    assert_exit(Command::new(&bin).arg("--help"), true);
}

#[test]
fn version_exits_zero() {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }
    assert_exit(Command::new(&bin).arg("--version"), true);
}

#[test]
fn version_subcommand_exits_zero() {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }
    assert_exit(Command::new(&bin).arg("version"), true);
}

#[test]
fn unknown_subcommand_exits_nonzero() {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }
    assert_exit(Command::new(&bin).arg("totally-unknown-subcommand"), false);
}

// ---------------------------------------------------------------------------
// Plugin subcommand help
// ---------------------------------------------------------------------------

#[test]
fn plugin_help_exits_zero() {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }
    assert_exit(Command::new(&bin).args(["plugin", "--help"]), true);
}

#[test]
fn memory_help_exits_zero() {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }
    assert_exit(Command::new(&bin).args(["memory", "--help"]), true);
}

#[test]
fn recipe_help_exits_zero() {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }
    assert_exit(Command::new(&bin).args(["recipe", "--help"]), true);
}

#[test]
fn mode_help_exits_zero() {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }
    assert_exit(Command::new(&bin).args(["mode", "--help"]), true);
}

// ---------------------------------------------------------------------------
// Version output content
// ---------------------------------------------------------------------------

#[test]
fn version_output_contains_amplihack() {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }
    let output = Command::new(&bin)
        .arg("--version")
        .output()
        .expect("failed to run binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.to_lowercase().contains("amplihack"),
        "Expected 'amplihack' in version output, got: {combined}"
    );
}
