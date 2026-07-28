//! TDD (RED) regression tests for #1108 — the bounded turn-failure error tail
//! must be routed through relay redaction **before** it is embedded into the
//! `io::Error` returned by a failed turn.
//!
//! Written **first**: these FAIL until `amplihack-turn` depends on
//! `amplihack-redact` and redacts the captured child stdout/stderr tail before
//! formatting it into the `io::Error::other("copilot turn failed (...): ...")`
//! message (and before the equivalent `tracing::debug!(output = ...)` field).
//!
//! Why here and not at the CLI: `turn.rs` builds the error string at
//! `crates/amplihack-turn/src/turn.rs` (the `!status.success()` branch). That
//! string is the root artifact every downstream sink (the Signal relay
//! `TurnOutput`, the `eprintln!` stderr sink in
//! `amplihack-cli/src/commands/signal/chat.rs`, and DEBUG logs) formats. If it
//! is redacted at the source, no sink can re-expose the secret.
//!
//! Constraints preserved: the `io::Error` *shape* (kind + "copilot turn failed"
//! prefix) and the #1092/#1107 bounded-tail truncation logic are unchanged —
//! only the secret bytes inside the tail are scrubbed.
//!
//! Run: `cargo test -p amplihack-turn --test turn_error_redaction_it`.

use std::sync::{Arc, Mutex};

use amplihack_turn::{CopilotTurnRunner, PreemptSlot, TurnRunner};

/// Spawn `sh -c <script>` as the turn's child program and return the error a
/// non-zero exit produces. `CopilotTurnRunner::run_argv` forwards argv directly
/// to the program (verified by the existing `normal_turn_returns_stdout` test).
async fn run_failing_turn(script: &str) -> std::io::Error {
    let slot: PreemptSlot = Arc::new(Mutex::new(None));
    let runner = CopilotTurnRunner::new("sh", slot);
    runner
        .run_argv(vec!["-c".to_string(), script.to_string()])
        .await
        .expect_err("a non-zero exit must resolve to an error")
}

#[tokio::test]
async fn github_token_in_failing_turn_stdout_is_redacted_in_error() {
    let secret = "ghp_0123456789abcdefghij0123";
    let err = run_failing_turn(&format!("printf 'token {secret}'; exit 3")).await;
    let msg = err.to_string();
    assert!(
        !msg.contains(secret),
        "the failing-turn error tail must be redacted before embedding: {msg:?}"
    );
    assert!(
        msg.contains("[REDACTED"),
        "a redaction placeholder must appear in place of the secret: {msg:?}"
    );
}

#[tokio::test]
async fn azure_devops_pat_in_failing_turn_stderr_is_redacted_in_error() {
    // 52-char AzDO PAT written to stderr (the failing branch concatenates both
    // stdout and stderr into the error tail).
    let secret = "abcd1234efgh5678ijkl9012mnop3456qrst7890uvwx1234yzAB";
    let err = run_failing_turn(&format!(
        "printf 'AZURE_DEVOPS_EXT_PAT={secret}' 1>&2; exit 1"
    ))
    .await;
    let msg = err.to_string();
    assert!(
        !msg.contains(secret),
        "an AzDO PAT in the stderr tail must not survive in the error: {msg:?}"
    );
}

#[tokio::test]
async fn error_shape_is_preserved_when_no_secret_present() {
    // Redaction must be transparent for ordinary output: the existing
    // "copilot turn failed" contract (and non-Interrupted kind) is unchanged,
    // and benign text is passed through verbatim.
    let err = run_failing_turn("printf 'ordinary failure detail'; exit 2").await;
    assert_ne!(
        err.kind(),
        std::io::ErrorKind::Interrupted,
        "an ordinary failed turn must not masquerade as a pre-emption"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("copilot turn failed"),
        "the failure prefix contract must be preserved: {msg:?}"
    );
    assert!(
        msg.contains("ordinary failure detail"),
        "benign output must pass through un-redacted: {msg:?}"
    );
}
