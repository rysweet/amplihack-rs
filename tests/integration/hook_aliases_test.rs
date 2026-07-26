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
//! `session-end` and `session-stop` dispatch to the whole-session teardown
//! handler (`SessionStopHook`), alongside `session-stop-event`. They are
//! deliberately NOT aliases for the per-turn `stop`/`agentStop` StopHook: the
//! Signal-channel design tears the per-session group down only at genuine
//! session end, so routing `session-end` to `stop` would either skip teardown
//! or kill the channel mid-session. All three teardown spellings must dispatch
//! to the same `SessionStopHook` (no copy-paste handler that could diverge).
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
    assert!(success, "precommit-prefs must exit 0; got stderr: {stderr}");
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
// session-end / session-stop: whole-session teardown (SessionStopHook)
// ---------------------------------------------------------------------------

#[test]
fn session_end_alias_dispatches_to_stop_handler() {
    // `session-end` routes to the whole-session teardown handler
    // (SessionStopHook). It must be recognized and fail-open (exit 0) so any
    // settings.json or recipe wiring that uses the SessionEnd event keeps
    // working.
    let payload = "{}";
    let (stdout, stderr, success) = invoke("session-end", payload);
    assert!(
        success,
        "session-end must exit 0 (SessionStopHook is fail-open); stderr: {stderr}"
    );
    assert!(
        !stderr.contains("unknown subcommand"),
        "session-end must be a recognized subcommand; stderr: {stderr}"
    );
    // Output must be valid JSON.
    let stdout_json = if stdout.trim().is_empty() {
        "{}"
    } else {
        stdout.as_str()
    };
    let _: serde_json::Value = serde_json::from_str(stdout_json)
        .unwrap_or_else(|e| panic!("session-end stdout must be valid JSON: {e}; got: {stdout}"));
}

#[test]
fn session_stop_alias_dispatches_to_stop_handler() {
    // `session-stop` routes to the SessionStopHook teardown handler, same as
    // `session-end` and `session-stop-event`.
    let payload = "{}";
    let (stdout, stderr, success) = invoke("session-stop", payload);
    assert!(success, "session-stop alias must exit 0; stderr: {stderr}");
    assert!(
        !stderr.contains("unknown subcommand"),
        "session-stop must be a recognized alias; stderr: {stderr}"
    );
    let stdout_json = if stdout.trim().is_empty() {
        "{}"
    } else {
        stdout.as_str()
    };
    let _: serde_json::Value = serde_json::from_str(stdout_json)
        .unwrap_or_else(|e| panic!("session-stop stdout must be valid JSON: {e}; got: {stdout}"));
}

#[test]
fn session_aliases_match_direct_stop_behavior() {
    // The teardown spellings (`session-end`, `session-stop`) must dispatch to
    // the EXACT same handler as the canonical `session-stop-event`
    // (SessionStopHook) — not a copy-paste handler that could diverge, and
    // NOT the per-turn `stop`/`agentStop` StopHook. Equality check: each alias
    // produces the same top-level JSON shape as `session-stop-event` for the
    // same input.
    let payload = "{}";
    let (canonical_out, _, canonical_ok) = invoke("session-stop-event", payload);
    let canonical_src = if canonical_out.trim().is_empty() {
        "{}"
    } else {
        canonical_out.as_str()
    };
    let canonical_json: serde_json::Value =
        serde_json::from_str(canonical_src).expect("session-stop-event stdout must be JSON");
    let canonical_keys = canonical_json
        .as_object()
        .map(|o| o.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();

    for alias in ["session-end", "session-stop"] {
        let (alias_out, _, alias_ok) = invoke(alias, payload);
        assert_eq!(
            canonical_ok, alias_ok,
            "{alias} must mirror session-stop-event's exit status"
        );
        // Top-level keys must match; values may carry nondeterministic fields
        // (timestamps, session ids) so byte-for-byte equality is not asserted.
        let alias_src = if alias_out.trim().is_empty() {
            "{}"
        } else {
            alias_out.as_str()
        };
        let alias_json: serde_json::Value =
            serde_json::from_str(alias_src).expect("alias stdout must be JSON");
        let alias_keys = alias_json
            .as_object()
            .map(|o| o.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        assert_eq!(
            canonical_keys, alias_keys,
            "{alias} must produce the same JSON shape as session-stop-event (shared SessionStopHook)"
        );
    }
}
