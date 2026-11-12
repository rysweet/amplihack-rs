//! Issue #1971: `launch --skip-update-check` under `AMPLIHACK_SKIP_AUTO_INSTALL`
//! must not start any network, fetch or re-stage work.
//!
//! `skip_update_check_flag::launch_with_skip_update_check_completes_without_npm_subprocess_overhead`
//! (WS3-INT-4) runs the same launch with a recording `npm` stub and owns the
//! assertion that `--skip-update-check` runs no `npm`. This test owns what
//! WS3-INT-4 does not check: that the self-heal bypass starts no `git` or
//! `curl`, creates neither fetch-backoff nor install-lock file, prints none of
//! the fetch or re-stage messages, and does print the bypass diagnostic.
//!
//! It also records `npm`, so that its stub log lists every recorded tool the
//! scenario starts; an `npm` entry here duplicates a WS3-INT-4 failure rather
//! than adding a separate assertion. Neither test makes a timing assertion.
//!
//! The stubs record only `git`, `npm` and `curl` found through `PATH`. A call
//! by absolute path (for example `/usr/bin/git`) would not be recorded; no such
//! call exists in the launch path today.
//!
//! What still runs under the bypass, and is therefore not asserted against:
//! the #1271 bundle-compatibility check, which may write
//! `~/.amplihack/.bundle-compat-cache.json` on a fresh `HOME`. That is a plain
//! file write with no network, lock or subprocess.
//!
//! Each test uses its own `HOME` under `$TMPDIR` so state left in
//! `shared_temp_home()` by other tests cannot reach the marker assertions.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

/// The binary under test; Cargo builds it before this target.
fn require_binary() -> PathBuf {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_amplihack"));
    assert!(bin.exists(), "amplihack binary not found at {bin:?}");
    bin
}

/// A stub `claude` that passes the launch-target health gate (#1266).
const HEALTHY_CLAUDE: &[u8] =
    b"#!/bin/sh\ncase \"$1\" in --version) echo '9.9.9 (stub claude)' ;; esac\nexit 0\n";

/// A stub that records its name and arguments, then fails. Fixed content;
/// `STUB_LOG` must be set, so a stub that ran without it fails loudly.
const RECORDING_STUB: &[u8] =
    b"#!/bin/sh\nprintf '%s %s\\n' \"$0\" \"$*\" >> \"${STUB_LOG:?}\"\nexit 1\n";

/// Tools whose execution would mean the bypass or the skip flag leaked.
const RECORDED_TOOLS: [&str; 3] = ["git", "npm", "curl"];

/// Files under `~/.amplihack` that only the fetch or re-stage path creates.
const FORBIDDEN_MARKERS: [&str; 2] = [".tag-fetch-failed", ".install.lock"];

/// Stderr fragments that only the fetch or re-stage path prints.
const FORBIDDEN_STDERR: [&str; 3] = [
    "keeping the installed bundle",
    "could not fetch the framework bundle",
    "cannot fetch a release tag",
];

/// The #502 R6 bypass diagnostic prefix (self_heal.rs). A fresh `HOME` has no
/// stamp, so the skew branch always prints it.
const BYPASS_DIAGNOSTIC: &str = "amplihack: AMPLIHACK_SKIP_AUTO_INSTALL set; skipping re-stage";

struct Sandbox {
    _root: tempfile::TempDir,
    bin: PathBuf,
    home: PathBuf,
    stub_log: PathBuf,
}

fn write_executable(path: &Path, body: &[u8]) {
    std::fs::write(path, body).unwrap_or_else(|e| panic!("writing {path:?}: {e}"));
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .unwrap_or_else(|e| panic!("chmod {path:?}: {e}"));
}

/// A temp dir under `$TMPDIR` holding `bin/` (stubs only) and `home/`.
fn sandbox() -> Sandbox {
    let root = tempfile::Builder::new()
        .prefix("issue-1971-")
        .tempdir_in(std::env::temp_dir())
        .expect("create sandbox under $TMPDIR");
    let bin = root.path().join("bin");
    let home = root.path().join("home");
    std::fs::create_dir(&bin).expect("create bin/");
    std::fs::create_dir(&home).expect("create home/");
    write_executable(&bin.join("claude"), HEALTHY_CLAUDE);
    for tool in RECORDED_TOOLS {
        write_executable(&bin.join(tool), RECORDING_STUB);
    }
    let stub_log = root.path().join("stub.log");
    Sandbox {
        _root: root,
        bin,
        home,
        stub_log,
    }
}

/// Runs `amplihack launch <extra_args>` with the bypass set and the sandbox
/// as the whole of `HOME` and `PATH`.
fn run_launch(sb: &Sandbox, extra_args: &[&str], noninteractive: bool) -> Output {
    let mut command = crate::isolated_amplihack::isolated(require_binary());
    command
        .env("HOME", &sb.home)
        .env("PATH", &sb.bin)
        .env("STUB_LOG", &sb.stub_log)
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .arg("launch")
        .args(extra_args)
        .stdin(Stdio::null());
    if noninteractive {
        command.env("AMPLIHACK_NONINTERACTIVE", "1");
    }
    command.output().expect("run amplihack launch")
}

/// The stub log, or empty if no stub ran. Any other read error fails the test.
fn stub_log(sb: &Sandbox) -> String {
    match std::fs::read_to_string(&sb.stub_log) {
        Ok(log) => log,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => panic!("reading stub log {:?}: {e}", sb.stub_log),
    }
}

/// The guard assertions shared by both scenarios.
fn assert_guard_held(sb: &Sandbox, output: &Output, scenario: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let log = stub_log(sb);
    let context = format!(
        "scenario: {scenario}\nexit: {}\nstub log:\n{log}\nstderr:\n{stderr}",
        output.status
    );

    assert!(
        log.is_empty(),
        "no git, npm or curl may run under the bypass\n{context}"
    );
    let state = sb.home.join(".amplihack");
    for marker in FORBIDDEN_MARKERS {
        assert!(
            !state.join(marker).exists(),
            "~/.amplihack/{marker} must not be created under the bypass\n{context}"
        );
    }
    for fragment in FORBIDDEN_STDERR {
        assert!(
            !stderr.contains(fragment),
            "stderr must not contain {fragment:?} under the bypass\n{context}"
        );
    }
    assert!(
        stderr.contains(BYPASS_DIAGNOSTIC),
        "the bypass must announce itself on a fresh HOME, proving self-heal took the bypass \
         branch rather than not running at all\n{context}"
    );
}

/// The scenario of WS3-INT-4, with the bypass assertions added.
#[test]
fn skip_flag_and_skip_auto_install_run_no_git_npm_or_curl() {
    let sb = sandbox();
    let output = run_launch(&sb, &["--skip-update-check"], true);
    assert_guard_held(
        &sb,
        &output,
        "--skip-update-check + SKIP_AUTO_INSTALL + NONINTERACTIVE",
    );
}

/// Without `AMPLIHACK_NONINTERACTIVE`, the bypass must still keep git and curl
/// idle. Stdin is not a TTY here, so the update check is skipped as
/// non-interactive whatever the flag (issue #1973); this run does not show
/// that the flag alone suppresses npm. The unit test
/// `tool_update_check::tests::check_wanted_only_without_skip_in_an_interactive_session`
/// covers that.
#[test]
fn skip_flag_without_noninteractive_still_runs_no_git_npm_or_curl() {
    let sb = sandbox();
    let output = run_launch(&sb, &["--skip-update-check"], false);
    assert_guard_held(&sb, &output, "--skip-update-check + SKIP_AUTO_INSTALL");
}

/// The recording stubs must work, or an empty log proves nothing. Runs a
/// stub directly with the same environment the tests above give amplihack.
#[test]
fn recording_stubs_record_when_invoked() {
    let sb = sandbox();
    for tool in RECORDED_TOOLS {
        let status = std::process::Command::new(sb.bin.join(tool))
            .arg("probe")
            .env_clear()
            .env("STUB_LOG", &sb.stub_log)
            .status()
            .unwrap_or_else(|e| panic!("run {tool} stub: {e}"));
        assert!(!status.success(), "{tool} stub must exit non-zero");
    }
    let log = stub_log(&sb);
    for tool in RECORDED_TOOLS {
        assert!(
            log.lines().any(|l| l.contains(&format!("/{tool} probe"))),
            "{tool} stub did not record its call; log:\n{log}"
        );
    }
}
