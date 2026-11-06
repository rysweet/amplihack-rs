//! TDD integration tests: `--skip-update-check` flag for `amplihack launch`.
//!
//! ## Why this test exists (WS3)
//!
//! The pre-launch npm update check (`tool_update_check::maybe_print_npm_update_notice`)
//! makes two network calls (npm list, npm show).  In CI, offline environments,
//! or scripted pipelines, this check must be suppressible via:
//!
//!   amplihack launch --skip-update-check
//!
//! These tests verify:
//! 1. The flag is accepted by the CLI without error
//! 2. The flag appears in `amplihack launch --help` output
//! 3. A launch with the flag completes without running npm
//!
//! Point 3 does not show that the flag is what prevents npm. These runs have
//! no TTY on stdin, so `is_noninteractive()` already skips the check, flag or
//! not (issue #1973). The unit test
//! `tool_update_check::tests::check_wanted_only_without_skip_in_an_interactive_session`
//! shows that `skip` alone suppresses the check.
//!
//! ## Failure modes
//!
//! These tests FAIL (red) before WS3 is implemented because:
//! - `--skip-update-check` is not yet in the `Launch` clap struct
//! - The CLI exits 2 ("unrecognized argument") when the flag is passed
//! - The help text does not mention the flag
//!
//! They PASS (green) once the flag is added to the CLI and `run_launch`
//! accepts a `skip_update_check: bool` parameter.

use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Path to the compiled amplihack binary.
fn amplihack_bin() -> PathBuf {
    // Cargo builds the binary as a prerequisite of this [[test]] target and
    // exposes its exact path here, honouring the active profile / target dir.
    PathBuf::from(env!("CARGO_BIN_EXE_amplihack"))
}

/// Assert the binary exists, panicking with a clear message if not.
fn require_binary() -> PathBuf {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!(
            "amplihack binary not found at {bin:?}.\n\
             Run `cargo build` first to compile the debug binary."
        );
    }
    bin
}

/// A stub `claude` that passes the launch-target health gate (issue #1266).
///
/// The gate rejects any candidate whose `--version` does not produce a
/// parseable semver — that is the whole point of it, since the placeholder
/// amplihack used to launch was exactly a script that could not report a
/// version. A stub written as `#!/bin/sh\nexit 0\n` is indistinguishable from
/// that placeholder, so it is no longer launched and resolution falls through
/// to a real claude on the host, which is not what these tests are about. This
/// stub answers `--version`.
///
/// It carries no padding. It used to be padded past a 4 KiB size threshold,
/// which is how a test fixture ends up documenting a production bug instead of
/// catching it: that threshold also rejected `@github/copilot`'s real loader.
fn healthy_stub_script() -> Vec<u8> {
    b"#!/bin/sh\ncase \"$1\" in --version) echo '9.9.9 (stub claude)' ;; esac\nexit 0\n".to_vec()
}

// ---------------------------------------------------------------------------
// WS3-INT-1: --skip-update-check is accepted by `amplihack launch --help`
// ---------------------------------------------------------------------------

/// `amplihack launch --help` must document `--skip-update-check`.
///
/// **FAILS** before WS3: the flag is not in the Launch clap struct, so it
/// doesn't appear in help output.
///
/// **PASSES** once `#[arg(long)] skip_update_check: bool` is in `Launch`.
#[test]
fn launch_help_documents_skip_update_check_flag() {
    let bin = require_binary();
    let output = crate::isolated_amplihack::isolated(&bin)
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .args(["launch", "--help"])
        .output()
        .expect("failed to run amplihack launch --help");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stdout, stderr);

    assert!(
        combined.contains("skip-update-check"),
        "FAIL: 'amplihack launch --help' output does not contain '--skip-update-check'.\n\
         \n\
         This flag must be added to the `Launch` clap struct:\n\
         \n\
           /// Skip the pre-launch npm update check.\n\
           #[arg(long)]\n\
           skip_update_check: bool,\n\
         \n\
         Found in help output:\n{}",
        combined
    );
}

// ---------------------------------------------------------------------------
// WS3-INT-2: `amplihack launch --skip-update-check` exits 0 with a stub tool
// ---------------------------------------------------------------------------

/// When `--skip-update-check` is passed alongside a stub claude binary,
/// the launch must succeed (exit 0) without attempting any npm queries.
///
/// **FAILS** before WS3: clap exits with code 2 ("error: unexpected argument
/// '--skip-update-check'").
///
/// **PASSES** once the flag is wired into the CLI dispatch.
#[test]
fn launch_with_skip_update_check_exits_zero_with_stub_tool() {
    let bin = require_binary();

    // Create a temporary directory with a stub claude binary.
    let tmpdir = tempfile::tempdir().expect("failed to create tempdir");
    let stub_bin = tmpdir.path().join("claude");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&stub_bin, healthy_stub_script()).expect("failed to write stub claude");
        let mut perms = std::fs::metadata(&stub_bin)
            .expect("failed to stat stub")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&stub_bin, perms).expect("failed to set permissions");
    }

    let new_path = format!(
        "{}:{}",
        tmpdir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let status = crate::isolated_amplihack::isolated(&bin)
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .args(["launch", "--skip-update-check"])
        .env("PATH", &new_path)
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .status()
        .expect("failed to run amplihack launch --skip-update-check");

    assert!(
        status.success(),
        "FAIL: `amplihack launch --skip-update-check` exited with status {status}.\n\
         \n\
         Expected exit 0.  Possible causes:\n\
         - Flag not yet added to the Launch clap struct (exit 2)\n\
         - run_launch() not accepting skip_update_check parameter\n\
         - Stub claude binary not found in PATH"
    );
}

// ---------------------------------------------------------------------------
// WS3-INT-3: Without --skip-update-check, the flag absence is not an error
// ---------------------------------------------------------------------------

/// `amplihack launch` without `--skip-update-check` must not exit with
/// "unexpected argument" or similar flag-related errors.
///
/// This is a regression guard: adding the flag must not break invocations
/// that don't use it.
#[test]
fn launch_without_skip_update_check_is_not_an_error() {
    let bin = require_binary();

    // Create a temporary directory with a stub claude binary.
    let tmpdir = tempfile::tempdir().expect("failed to create tempdir");
    let stub_bin = tmpdir.path().join("claude");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&stub_bin, healthy_stub_script()).expect("failed to write stub claude");
        let mut perms = std::fs::metadata(&stub_bin)
            .expect("failed to stat stub")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&stub_bin, perms).expect("failed to chmod stub");
    }

    let new_path = format!(
        "{}:{}",
        tmpdir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = crate::isolated_amplihack::isolated(&bin)
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .arg("launch")
        .env("PATH", &new_path)
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .output()
        .expect("failed to run amplihack launch");

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Must not produce a clap "unexpected argument" error.
    assert!(
        !stderr.contains("unexpected argument"),
        "FAIL: `amplihack launch` (without --skip-update-check) produced an \
         'unexpected argument' error.\n\
         stderr: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// WS3-INT-4: --skip-update-check runs no npm subprocess
// ---------------------------------------------------------------------------

/// With `--skip-update-check`, launch runs no npm subprocess at all.
///
/// Issue #1973: this used to measure elapsed time, which a loaded host
/// could push past any bound. Now a recording `npm` stub is the only `npm`
/// on `PATH`; it appends its arguments to a file in the tempdir and exits 1.
/// The test asserts that file never appeared.
#[cfg(unix)]
#[test]
fn launch_with_skip_update_check_completes_without_npm_subprocess_overhead() {
    let bin = require_binary();

    let tmpdir = tempfile::tempdir().expect("failed to create tempdir");
    crate::exec_stub::write_executable(&tmpdir.path().join("claude"), healthy_stub_script(), 0o755);
    let record = tmpdir.path().join("npm-calls");
    let record_text = record.to_str().expect("temporary paths are UTF-8");
    assert!(
        !record_text.contains(['\'', '\n']),
        "cannot single-quote {record_text:?} for /bin/sh"
    );
    // The record path is written into the stub, so the stub records even if
    // launch gives npm a cleared environment.
    crate::exec_stub::write_executable(
        &tmpdir.path().join("npm"),
        format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{record_text}'\nexit 1\n"),
        0o755,
    );

    // PATH contains only the stub dir: the recording npm is the only npm.
    let isolated_path = tmpdir.path().to_string_lossy().to_string();

    let mut cmd = crate::isolated_amplihack::isolated(&bin);
    cmd.env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .args(["launch", "--skip-update-check"])
        .env("PATH", &isolated_path)
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .stdin(std::process::Stdio::null());
    let out = crate::wait::output_within("`amplihack launch --skip-update-check`", &mut cmd);

    let calls = std::fs::read_to_string(&record).ok();
    assert!(
        calls.is_none(),
        "FAIL: `amplihack launch --skip-update-check` ran npm ({}):\n{}\n\
         Check that maybe_print_npm_update_notice respects the skip parameter.\n\
         stderr: {}",
        out.status,
        calls.unwrap_or_default(),
        String::from_utf8_lossy(&out.stderr)
    );
}

// ---------------------------------------------------------------------------
// WS3-INT-5: --skip-update-check flag appears in top-level --help output
// ---------------------------------------------------------------------------

/// The `launch` subcommand help must show the flag.
/// This is a discoverability requirement.
#[test]
fn amplihack_launch_help_shows_skip_update_check() {
    let bin = require_binary();
    let output = crate::isolated_amplihack::isolated(&bin)
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .args(["launch", "--help"])
        .output()
        .expect("failed to run amplihack launch --help");

    assert!(
        output.status.success(),
        "FAIL: `amplihack launch --help` exited non-zero.\n\
         Status: {}",
        output.status
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stdout, stderr);

    // The flag must appear in help output
    assert!(
        combined.contains("skip-update-check"),
        "FAIL: '--skip-update-check' not found in `amplihack launch --help` output.\n\
         Full output:\n{}",
        combined
    );
}
