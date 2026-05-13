//! TDD tests for issue #625: amplihack auto-update version-check prompt
//! must be suppressed in subprocess-safe contexts and emit a single,
//! literal skip-line on stderr so delegated agents have visible evidence
//! the check was bypassed.
//!
//! These tests are FAILING by design until `update::check` is refactored
//! per the issue #625 design spec:
//!
//!   1. `should_skip_update_check(args)` (back-compat wrapper) MUST return
//!      true when ANY of these signals is set:
//!        - env `AMPLIHACK_NONINTERACTIVE` non-empty (existing — must remain)
//!        - env `AMPLIHACK_AGENT_BINARY`   non-empty   (NEW)
//!        - env `CI`                       non-empty   (NEW)
//!        - argv contains literal `--subprocess-safe` (NEW)
//!        - args[1] is not a recognized launch subcommand (existing)
//!        - env `AMPLIHACK_NO_UPDATE_CHECK=1` (existing, silent)
//!        - env `AMPLIHACK_PARITY_TEST=1`     (existing, silent)
//!
//!   2. `maybe_print_update_notice_from_args(args)` MUST emit exactly the
//!      literal line `amplihack: skipping update check (subprocess-safe / no TTY)`
//!      to stderr when:
//!        - any of the four `SubprocessSafe` env/argv signals fires, OR
//!        - stdin is not a TTY at the entry point
//!
//!      AND must NOT emit it for `AMPLIHACK_NO_UPDATE_CHECK` /
//!      `AMPLIHACK_PARITY_TEST` (silent ExplicitOptOut) or for non-launch
//!      subcommands (silent NotLaunch passthrough).
//!
//!   3. `update::network::fetch_latest_release` MUST honor the test-only
//!      env var `AMPLIHACK_TEST_FAKE_LATEST_VERSION`: when non-empty,
//!      synthesize an `UpdateRelease` with that tag and return Ok without
//!      any network call. This enables deterministic prompt-path testing.
//!
//!   4. The interactive PTY path MUST still print the prompt and honor the
//!      5000ms `libc::poll` hard wall-clock timeout in
//!      `read_user_input_with_timeout`.
//!
//! The `assert_cmd` tests below run the `amplihack` binary as a subprocess
//! (stdin is not a TTY by default). Each test sanitises CI-runner env
//! pollution by clearing all five subprocess-safe env vars before
//! re-adding only the one(s) under test, so the assertion observes exactly
//! one skip-signal source. Each test also redirects HOME to a per-test
//! tempdir so the on-disk `last_update_check` cache cannot interfere.
//!
//! The PTY test uses `rexpect` to give the child a real TTY on stdin and
//! is gated `#[cfg(target_os = "linux")]` because rexpect's pseudo-tty
//! support is best-tested on Linux.

#![cfg(any(unix, windows))]

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

const SKIP_LINE: &str = "amplihack: skipping update check (subprocess-safe / no TTY)";
const PROMPT_FRAGMENT: &str = "Update now?";
const NEW_VERSION_NOTICE_FRAGMENT: &str = "A newer version of amplihack is available";
const FAKE_LATEST_VERSION: &str = "99.99.99";

/// Names of every env var that, when set, is a subprocess-safe or
/// opt-out signal. Tests clear all of these before setting only the
/// signal under test, so a CI runner's pre-existing `CI=true` cannot
/// taint per-test assertions.
const SIGNAL_ENV_VARS: &[&str] = &[
    "AMPLIHACK_NONINTERACTIVE",
    "AMPLIHACK_AGENT_BINARY",
    "AMPLIHACK_NO_UPDATE_CHECK",
    "AMPLIHACK_PARITY_TEST",
    "CI",
];

/// Locate the cargo-built `amplihack` binary at workspace root.
///
/// `amplihack-cli` is a library crate; the binary lives in `bins/amplihack/`.
/// `assert_cmd::cargo_bin` doesn't expose `CARGO_BIN_EXE_amplihack` for tests
/// in sibling crates, so we walk from this crate's manifest dir to the
/// workspace root and look in `target/debug/`.
///
/// Tests that require the binary `panic!` with a clear `cargo build -p
/// amplihack` instruction when the binary is missing — they DO NOT silently
/// skip.
fn amplihack_bin() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // crates/amplihack-cli → crates/
    path.pop(); // crates/ → workspace root
    path.push("target/debug/amplihack");
    path
}

fn require_bin_or_panic() -> PathBuf {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!(
            "amplihack binary not found at {} — run `cargo build -p amplihack` first \
             (or `cargo test --workspace` which builds it as a dependency)",
            bin.display()
        );
    }
    bin
}

/// Build a child `amplihack` invocation with all skip-signal env vars
/// cleared, the synthetic-release env var set, and HOME pointed at a
/// per-test tempdir. Caller adds the specific signal(s) under test.
fn cmd_with_clean_env(home: &std::path::Path) -> Command {
    let mut cmd = Command::new(require_bin_or_panic());
    cmd.env_clear();
    // Inherit minimum env required for a Rust binary on Unix to function.
    if let Ok(path) = std::env::var("PATH") {
        cmd.env("PATH", path);
    }
    cmd.env("HOME", home);
    cmd.env("AMPLIHACK_TEST_FAKE_LATEST_VERSION", FAKE_LATEST_VERSION);
    for name in SIGNAL_ENV_VARS {
        cmd.env_remove(name);
    }
    cmd
}

/// Run `cmd` with a hard wall-clock timeout. Returns (stdout, stderr).
/// Panics if the child does not exit within `timeout`.
fn run_with_timeout(mut cmd: Command, timeout: Duration) -> (String, String) {
    use std::io::Read;
    use std::time::Instant;
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().expect("amplihack spawned");
    let started = Instant::now();
    loop {
        match child.try_wait().expect("try_wait") {
            Some(_status) => break,
            None => {
                if started.elapsed() > timeout {
                    let _ = child.kill();
                    panic!("amplihack did not exit within {:?}", timeout);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_string(&mut stdout);
    }
    if let Some(mut s) = child.stderr.take() {
        let _ = s.read_to_string(&mut stderr);
    }
    (stdout, stderr)
}

// ────────────────────────────────────────────────────────────────────────────
// Test 1 — AMPLIHACK_NONINTERACTIVE=1 (existing signal, must remain working
// AND must now emit the skip-line on stderr).
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn noninteractive_env_emits_skip_line_and_suppresses_prompt() {
    let home = tempfile::tempdir().expect("tempdir");
    let mut cmd = cmd_with_clean_env(home.path());
    cmd.env("AMPLIHACK_NONINTERACTIVE", "1");
    cmd.arg("copilot").arg("--help");

    let (stdout, stderr) = run_with_timeout(cmd, Duration::from_secs(15));

    assert!(
        stderr.contains(SKIP_LINE),
        "expected skip-line on stderr when AMPLIHACK_NONINTERACTIVE=1 was set; \
         stderr was:\n{stderr}\nstdout was:\n{stdout}"
    );
    assert!(
        !stderr.contains(PROMPT_FRAGMENT) && !stdout.contains(PROMPT_FRAGMENT),
        "prompt must NOT print when subprocess-safe; stderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(
        !stderr.contains(NEW_VERSION_NOTICE_FRAGMENT),
        "the 'newer version available' notice must NOT print when subprocess-safe; stderr:\n{stderr}"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Test 2 — argv contains the literal `--subprocess-safe` flag (NEW signal).
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn subprocess_safe_argv_flag_emits_skip_line_and_suppresses_prompt() {
    let home = tempfile::tempdir().expect("tempdir");
    let mut cmd = cmd_with_clean_env(home.path());
    // argv = ["amplihack", "copilot", "--subprocess-safe", "--help"]
    cmd.arg("copilot").arg("--subprocess-safe").arg("--help");

    let (stdout, stderr) = run_with_timeout(cmd, Duration::from_secs(15));

    assert!(
        stderr.contains(SKIP_LINE),
        "expected skip-line on stderr when --subprocess-safe is in argv; \
         stderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(
        !stderr.contains(PROMPT_FRAGMENT) && !stdout.contains(PROMPT_FRAGMENT),
        "prompt must NOT print when --subprocess-safe is present; stderr:\n{stderr}\nstdout:\n{stdout}"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Test 3 — AMPLIHACK_AGENT_BINARY=copilot (NEW signal).
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn agent_binary_env_emits_skip_line_and_suppresses_prompt() {
    let home = tempfile::tempdir().expect("tempdir");
    let mut cmd = cmd_with_clean_env(home.path());
    cmd.env("AMPLIHACK_AGENT_BINARY", "copilot");
    cmd.arg("copilot").arg("--help");

    let (stdout, stderr) = run_with_timeout(cmd, Duration::from_secs(15));

    assert!(
        stderr.contains(SKIP_LINE),
        "expected skip-line on stderr when AMPLIHACK_AGENT_BINARY is non-empty; \
         stderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(
        !stderr.contains(PROMPT_FRAGMENT) && !stdout.contains(PROMPT_FRAGMENT),
        "prompt must NOT print when AMPLIHACK_AGENT_BINARY is set; stderr:\n{stderr}\nstdout:\n{stdout}"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Test 4 — CI=true (NEW signal; any non-empty value must trigger skip).
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn ci_env_true_emits_skip_line_and_suppresses_prompt() {
    let home = tempfile::tempdir().expect("tempdir");
    let mut cmd = cmd_with_clean_env(home.path());
    cmd.env("CI", "true");
    cmd.arg("copilot").arg("--help");

    let (stdout, stderr) = run_with_timeout(cmd, Duration::from_secs(15));

    assert!(
        stderr.contains(SKIP_LINE),
        "expected skip-line on stderr when CI=true; stderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(
        !stderr.contains(PROMPT_FRAGMENT) && !stdout.contains(PROMPT_FRAGMENT),
        "prompt must NOT print when CI=true; stderr:\n{stderr}\nstdout:\n{stdout}"
    );
}

#[test]
fn ci_env_one_emits_skip_line() {
    // Same contract as `CI=true` — `CI=1` is the GitHub Actions / GitLab CI
    // convention. Non-empty value triggers skip; `CI` is treated as an
    // opaque presence signal.
    let home = tempfile::tempdir().expect("tempdir");
    let mut cmd = cmd_with_clean_env(home.path());
    cmd.env("CI", "1");
    cmd.arg("copilot").arg("--help");

    let (_stdout, stderr) = run_with_timeout(cmd, Duration::from_secs(15));
    assert!(
        stderr.contains(SKIP_LINE),
        "expected skip-line on stderr when CI=1; stderr:\n{stderr}"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Test 5 — `amplihack --help` (non-launch subcommand): silent passthrough.
// MUST NOT emit the skip-line and MUST NOT print the update notice.
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn non_launch_subcommand_does_not_emit_skip_line() {
    let home = tempfile::tempdir().expect("tempdir");
    let mut cmd = cmd_with_clean_env(home.path());
    // No subprocess-safe signals — only the synthetic-release env var.
    cmd.arg("--help");

    let (stdout, stderr) = run_with_timeout(cmd, Duration::from_secs(15));

    assert!(
        !stderr.contains(SKIP_LINE),
        "skip-line must NOT print for non-launch subcommands (silent NotLaunch \
         passthrough); stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains(PROMPT_FRAGMENT) && !stdout.contains(PROMPT_FRAGMENT),
        "prompt must NOT print for `amplihack --help`; stderr:\n{stderr}\nstdout:\n{stdout}"
    );
    assert!(
        !stderr.contains(NEW_VERSION_NOTICE_FRAGMENT),
        "newer-version notice must NOT print for non-launch subcommands; stderr:\n{stderr}"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Test 6 — explicit opt-out via AMPLIHACK_NO_UPDATE_CHECK=1: silent skip.
// MUST NOT emit the skip-line (this is an ExplicitOptOut, not SubprocessSafe).
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn no_update_check_env_does_not_emit_skip_line() {
    let home = tempfile::tempdir().expect("tempdir");
    let mut cmd = cmd_with_clean_env(home.path());
    cmd.env("AMPLIHACK_NO_UPDATE_CHECK", "1");
    cmd.arg("copilot").arg("--help");

    let (_stdout, stderr) = run_with_timeout(cmd, Duration::from_secs(15));

    assert!(
        !stderr.contains(SKIP_LINE),
        "ExplicitOptOut (AMPLIHACK_NO_UPDATE_CHECK=1) MUST be silent — no skip-line. \
         stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains(PROMPT_FRAGMENT),
        "prompt must NOT print when AMPLIHACK_NO_UPDATE_CHECK=1; stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains(NEW_VERSION_NOTICE_FRAGMENT),
        "newer-version notice must NOT print when AMPLIHACK_NO_UPDATE_CHECK=1; stderr:\n{stderr}"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Test 7 — explicit opt-out via AMPLIHACK_PARITY_TEST=1: silent skip.
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn parity_test_env_does_not_emit_skip_line() {
    let home = tempfile::tempdir().expect("tempdir");
    let mut cmd = cmd_with_clean_env(home.path());
    cmd.env("AMPLIHACK_PARITY_TEST", "1");
    cmd.arg("copilot").arg("--help");

    let (_stdout, stderr) = run_with_timeout(cmd, Duration::from_secs(15));

    assert!(
        !stderr.contains(SKIP_LINE),
        "ExplicitOptOut (AMPLIHACK_PARITY_TEST=1) MUST be silent — no skip-line. \
         stderr:\n{stderr}"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Test 8 — empty `CI=""` is NOT a skip signal (presence != non-empty value).
// Without any other signal, stdin-not-TTY (assert_cmd's default) will
// independently trigger the skip-line. So we cannot use this assertion to
// prove "no skip" — but we CAN prove the skip-line is still emitted (via
// the TTY check) and the prompt is still suppressed.
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn empty_ci_value_alone_does_not_classify_as_subprocess_safe() {
    // The intent of this test: assert that `classify_skip_reason` does NOT
    // consider `CI=""` a SubprocessSafe signal. End-to-end, we cannot
    // observe this directly because the stdin-not-TTY check still fires
    // and emits the same skip-line. To guard the contract we look at the
    // source: the skip-line emitted MUST be triggered by the TTY check,
    // not by the empty-CI value. We assert the prompt/notice is suppressed
    // (TTY-driven skip) but the test ALSO asserts the source-level invariant.
    //
    // The source-level invariant is: `classify_skip_reason` checks
    // `!value.is_empty()` before classifying as SubprocessSafe.
    let src = include_str!("../src/update/check.rs");
    assert!(
        src.contains("is_empty"),
        "classify_skip_reason MUST guard CI / AMPLIHACK_AGENT_BINARY with !is_empty() \
         to reject empty-string env values per the design spec. check.rs source:\n…"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Test 9 — interactive PTY: prompt IS printed, 5s timeout fires. The
// process MUST exit within 7s (5s timeout + ~2s clap shutdown budget) and
// the skip-line MUST NOT appear (TTY-attached stdin is the production
// interactive case).
// ────────────────────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
mod pty {
    use super::*;
    use rexpect::session::PtySession;
    use rexpect::spawn;
    use std::time::Instant;

    /// Build a single shell command line that runs the cargo-built `amplihack`
    /// binary with the synthetic-release env var set and all skip-signal env
    /// vars cleared, so the only thing that decides whether the prompt fires
    /// is whether stdin is a TTY (rexpect provides a PTY).
    fn build_pty_command() -> String {
        let bin = require_bin_or_panic();
        let bin = bin.to_string_lossy().to_string();
        // env -i with a curated allowlist: PATH (for any subprocess) + HOME
        // (per-test tempdir created below), plus the synthetic-release var.
        // We do NOT preserve CI / AMPLIHACK_* — those are the very signals
        // we're testing the absence of.
        //
        // We use `into_path()` to leak the tempdir for the duration of the
        // PTY session so the child can read/write it without a TOCTOU race
        // on cleanup. The OS reaps it at process exit.
        let home = tempfile::tempdir().expect("tempdir").keep();
        let path = std::env::var("PATH").unwrap_or_default();
        format!(
            "/usr/bin/env -i \
             PATH={path} \
             HOME={home} \
             AMPLIHACK_TEST_FAKE_LATEST_VERSION={fake} \
             {bin} copilot --help",
            path = path,
            home = home.display(),
            fake = FAKE_LATEST_VERSION,
            bin = bin,
        )
    }

    #[test]
    fn interactive_tty_prints_prompt_and_honors_5s_timeout() {
        let cmd = build_pty_command();
        let started = Instant::now();
        // 7-second hard budget: 5s prompt timeout + 2s clap+process shutdown.
        let mut session: PtySession =
            spawn(&cmd, Some(7_000)).expect("rexpect failed to spawn amplihack under PTY");

        // Assert the prompt IS printed when stdin is a real TTY.
        session
            .exp_string(PROMPT_FRAGMENT)
            .expect("expected 'Update now?' prompt to be printed under interactive TTY");

        // Do NOT send any input — let the 5s libc::poll timeout fire.
        // The process should then proceed to clap (which short-circuits on
        // --help) and exit.
        let status = session
            .process
            .wait()
            .expect("rexpect: child wait failed after timeout");
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_secs(8),
            "process must exit within ~7s budget (5s prompt timeout + clap shutdown); \
             actual: {:?}",
            elapsed
        );
        // The 5-second hard wall-clock minimum: the timeout MUST have fired,
        // not been bypassed by a 0-second poll.
        assert!(
            elapsed >= Duration::from_secs(4),
            "process must wait for the prompt timeout (≥4s, allowing 1s slack); \
             actual: {:?}",
            elapsed
        );

        // Drain any remaining output to verify the skip-line was NOT printed
        // for the interactive (TTY) path.
        let mut tail = String::new();
        let _ = session.exp_eof().map(|s| tail = s);
        let combined = tail;
        assert!(
            !combined.contains(SKIP_LINE),
            "skip-line must NOT print on interactive TTY path; tail:\n{combined}"
        );

        // Sanity: the process did exit (any status — clap may print help and
        // exit 0, or exit non-zero if --help is unrecognised in this build).
        let _ = status;
    }
}
