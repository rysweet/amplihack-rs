//! TDD tests for issue #522: native Rust subcommands replacing 7 Python hook shims.
//!
//! Status: FAILING until implementation lands.
//!
//! These tests assert the contract that the `amplihack-hooks` binary exposes
//! native equivalents for every subcommand the deleted Python shims used to
//! delegate to:
//!
//! | Deleted .py file              | Native dispatch target              |
//! |-------------------------------|-------------------------------------|
//! | precommit_prefs.py            | `amplihack-hooks precommit-prefs`   |
//! | session_end.py                | `amplihack-hooks session-end`       |
//! | session_stop.py               | `amplihack-hooks session-stop`      |
//! | stop.py                       | `amplihack-hooks stop`              |
//! | post_tool_use.py              | `amplihack-hooks post-tool-use`     |
//! | user_prompt_submit.py         | `amplihack-hooks user-prompt-submit`|
//! | _shim.py                      | (no native; helper only)            |
//!
//! `session-end` and `session-stop` are clap aliases for `stop` (per design
//! spec A3) — both must dispatch to the same StopHook handler the legacy
//! Python shims forwarded to.
//!
//! `precommit-prefs` is a no-op: it drains stdin and exits 0. It must NOT
//! echo, log, or otherwise persist the stdin payload (security: stdin may
//! contain user prompts or secrets — see design spec security_considerations).

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Path to the compiled amplihack-hooks binary. Uses the Cargo-provided
/// `CARGO_BIN_EXE_<name>` env var so the test honors `CARGO_TARGET_DIR`
/// overrides instead of hard-coding `target/debug/`.
fn hooks_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_amplihack-hooks"))
}

/// Spawn the hooks binary with the given subcommand and stdin payload.
/// Returns (stdout, stderr, exit_success).
fn invoke(subcommand: &str, stdin_payload: &str) -> (String, String, bool) {
    let bin = hooks_bin();
    assert!(
        bin.exists(),
        "amplihack-hooks binary not built at {} — run `cargo build -p amplihack-hooks-bin` first",
        bin.display()
    );

    let mut child = Command::new(&bin)
        .arg(subcommand)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn {}: {e}", bin.display()));

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(stdin_payload.as_bytes());
    }

    let output = child.wait_with_output().expect("wait_with_output failed");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    )
}

// ---------------------------------------------------------------------------
// precommit-prefs: no-op subcommand
// ---------------------------------------------------------------------------

#[test]
fn precommit_prefs_subcommand_is_recognized() {
    // Regression for design spec A2: a `precommit-prefs` subcommand must
    // exist on the dispatcher. Before the port this exits non-zero with
    // "unknown subcommand" on stderr.
    let (_stdout, stderr, success) = invoke("precommit-prefs", "");
    assert!(
        success,
        "precommit-prefs must exit 0; got stderr: {stderr}"
    );
    assert!(
        !stderr.contains("unknown subcommand"),
        "precommit-prefs must be a registered subcommand; stderr: {stderr}"
    );
}

#[test]
fn precommit_prefs_drains_stdin_and_exits_zero() {
    // Mirrors the Python shim's `delegate(None)` no-op behavior: read whatever
    // arrives on stdin (so the parent does not block on a full pipe) and
    // exit 0 with empty / minimal stdout.
    let payload = "{\"tool\":\"git-commit\",\"sensitive\":\"would-be-secret\"}";
    let (stdout, stderr, success) = invoke("precommit-prefs", payload);
    assert!(
        success,
        "precommit-prefs must exit 0 even with stdin payload; stderr: {stderr}"
    );
    // Stdout must not contain the stdin payload — that would mean the
    // subcommand echoed sensitive data. Per security_considerations this
    // hook must not log or echo stdin.
    assert!(
        !stdout.contains("would-be-secret"),
        "precommit-prefs must not echo stdin contents to stdout; got: {stdout}"
    );
    assert!(
        !stderr.contains("would-be-secret"),
        "precommit-prefs must not echo stdin contents to stderr; got: {stderr}"
    );
}

#[test]
fn precommit_prefs_handles_empty_stdin() {
    // Pre-commit hooks may run with no stdin payload at all (depending on
    // git wrapper). Must still exit 0 cleanly.
    let (_stdout, stderr, success) = invoke("precommit-prefs", "");
    assert!(
        success,
        "precommit-prefs must exit 0 with empty stdin; stderr: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// session-end / session-stop: aliases for stop
// ---------------------------------------------------------------------------

#[test]
fn session_end_alias_dispatches_to_stop_handler() {
    // Replaces session_end.py which called `delegate("stop")`. The Rust
    // dispatcher must accept `session-end` as a synonym for `stop` so any
    // settings.json or recipe wiring that uses the SessionEnd event keeps
    // working without inventing a separate native handler.
    let payload = "{}";
    let (stdout, stderr, success) = invoke("session-end", payload);
    assert!(
        success,
        "session-end alias must exit 0 (StopHook is fail-open); stderr: {stderr}"
    );
    assert!(
        !stderr.contains("unknown subcommand"),
        "session-end must be a recognized alias; stderr: {stderr}"
    );
    // Output must be valid JSON — same contract as direct `stop` invocation.
    let stdout_json = if stdout.trim().is_empty() { "{}" } else { stdout.as_str() };
    let _: serde_json::Value = serde_json::from_str(stdout_json)
        .unwrap_or_else(|e| panic!("session-end stdout must be valid JSON: {e}; got: {stdout}"));
}

#[test]
fn session_stop_alias_dispatches_to_stop_handler() {
    // Replaces session_stop.py. Same alias-to-stop semantics as session-end.
    let payload = "{}";
    let (stdout, stderr, success) = invoke("session-stop", payload);
    assert!(
        success,
        "session-stop alias must exit 0; stderr: {stderr}"
    );
    assert!(
        !stderr.contains("unknown subcommand"),
        "session-stop must be a recognized alias; stderr: {stderr}"
    );
    let stdout_json = if stdout.trim().is_empty() { "{}" } else { stdout.as_str() };
    let _: serde_json::Value = serde_json::from_str(stdout_json)
        .unwrap_or_else(|e| panic!("session-stop stdout must be valid JSON: {e}; got: {stdout}"));
}

#[test]
fn session_aliases_match_direct_stop_behavior() {
    // The aliases must dispatch to the EXACT same handler as `stop` (not a
    // copy-paste handler that could diverge — see security_considerations).
    // Equality check: both produce the same JSON shape for the same input.
    let payload = "{}";
    let (stop_out, _, stop_ok) = invoke("stop", payload);
    let (alias_out, _, alias_ok) = invoke("session-end", payload);
    assert_eq!(stop_ok, alias_ok, "alias must mirror stop's exit status");

    // Both should produce valid JSON. We don't assert byte-for-byte equality
    // because StopHook may include nondeterministic fields (timestamps,
    // session ids), but the top-level keys must match.
    let stop_json_src = if stop_out.trim().is_empty() { "{}" } else { stop_out.as_str() };
    let alias_json_src = if alias_out.trim().is_empty() { "{}" } else { alias_out.as_str() };
    let stop_json: serde_json::Value =
        serde_json::from_str(stop_json_src).expect("stop stdout must be JSON");
    let alias_json: serde_json::Value =
        serde_json::from_str(alias_json_src).expect("session-end stdout must be JSON");
    assert_eq!(
        stop_json
            .as_object()
            .map(|o| o.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default(),
        alias_json
            .as_object()
            .map(|o| o.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default(),
        "session-end must produce the same JSON shape as direct stop invocation"
    );
}
