//! Integration test for the `amplihack-hooks signal-subscriber` subcommand.
//!
//! Registered as an explicit `[[test]]` target and resolves the real binary via
//! `env!("CARGO_BIN_EXE_amplihack-hooks")` so it exercises the actual detached
//! subscriber entrypoint rather than an in-process stub.
//!
//! Compiled only under the `signal` feature; otherwise this is an empty crate.
#![cfg(feature = "signal")]

use std::process::Command;

const HOOKS_BIN: &str = env!("CARGO_BIN_EXE_amplihack-hooks");

/// The `signal-subscriber` subcommand must be recognized by the dispatcher —
/// i.e. it must NOT fall through to the "unknown subcommand" arm.
#[test]
fn signal_subscriber_is_a_recognized_subcommand() {
    // Run with NO signal config in the environment. Per the non-fatal contract,
    // a config-load failure must be logged as a warning and the process must
    // exit 0 — never a hard error and never "unknown subcommand".
    let output = Command::new(HOOKS_BIN)
        .arg("signal-subscriber")
        .arg("--session-id")
        .arg("it-test-session")
        // Explicitly clear any inherited Signal config so config load fails
        // cleanly (exercising the non-fatal path).
        .env_remove("AMPLIHACK_SIGNAL_ENDPOINT")
        .env_remove("AMPLIHACK_SIGNAL_ACCOUNT")
        .env_remove("AMPLIHACK_SIGNAL_ALLOWLIST")
        .env_remove("AMPLIHACK_SIGNAL_CONFIG")
        .output()
        .expect("failed to spawn amplihack-hooks");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("unknown subcommand"),
        "signal-subscriber must be a recognized subcommand; stderr was:\n{stderr}"
    );
    assert!(
        output.status.success(),
        "non-fatal contract: config-load failure must still exit 0 (got {:?})\nstderr:\n{stderr}",
        output.status.code()
    );
}

/// With a well-formed config but an unreachable daemon, the subscriber must
/// still honor the non-fatal contract (connection failure ⇒ warning + exit 0),
/// never hang the caller or return a hard error.
#[test]
fn signal_subscriber_unreachable_daemon_is_non_fatal() {
    let output = Command::new(HOOKS_BIN)
        .arg("signal-subscriber")
        .arg("--session-id")
        .arg("it-test-session-2")
        // 127.0.0.1:1 is a reserved port with no listener → connect fails fast.
        .env("AMPLIHACK_SIGNAL_ENDPOINT", "127.0.0.1:1")
        .env("AMPLIHACK_SIGNAL_ACCOUNT", "+15551230000")
        .env("AMPLIHACK_SIGNAL_ALLOWLIST", "+15551230001")
        .env_remove("AMPLIHACK_SIGNAL_CONFIG")
        .output()
        .expect("failed to spawn amplihack-hooks");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("unknown subcommand"),
        "signal-subscriber must be recognized; stderr:\n{stderr}"
    );
    assert!(
        output.status.success(),
        "unreachable daemon must be non-fatal (exit 0), got {:?}\nstderr:\n{stderr}",
        output.status.code()
    );
}
