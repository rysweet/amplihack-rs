//! TDD contract — Signal op error → exit-code taxonomy (#921/#923).
//!
//! Run with: `cargo test -p amplihack-cli --features signal --test signal_error_taxonomy`
//!
//! A single source of truth maps every `SignalOpError` variant to a stable
//! process exit code. Downstream tooling and the fleet orchestrator rely on
//! these codes, so they are contractual:
//!
//!   0  success (not represented as an error)
//!   2  usage / bad arguments
//!   3  unsupported (built without `--features signal`, or an unimplemented
//!      identity mode such as `dedicated-number`)
//!   4  signal-cli detection / installation failure
//!   5  partial fleet rollout (some VMs failed; run is resumable)
//!   6  local daemon / port failure
//!   7  device-linking failure (including Signal's linked-device limit)
#![cfg(feature = "signal")]

use amplihack_cli::commands::signal::error::SignalOpError;

#[test]
fn usage_error_maps_to_2() {
    assert_eq!(SignalOpError::Usage("bad flag".into()).exit_code(), 2);
}

#[test]
fn unsupported_maps_to_3() {
    // Same code used when the binary is built without the `signal` feature and
    // when an unimplemented identity mode is requested.
    assert_eq!(
        SignalOpError::Unsupported("feature disabled".into()).exit_code(),
        3
    );
}

#[test]
fn signal_cli_error_maps_to_4() {
    assert_eq!(
        SignalOpError::SignalCli("signal-cli not found".into()).exit_code(),
        4
    );
}

#[test]
fn partial_rollout_maps_to_5() {
    let err = SignalOpError::Partial {
        succeeded: 2,
        total: 3,
        failures: vec![("vm-c".into(), "link timeout".into())],
    };
    assert_eq!(err.exit_code(), 5);
}

#[test]
fn daemon_error_maps_to_6() {
    assert_eq!(
        SignalOpError::Daemon("port 7583 in use".into()).exit_code(),
        6
    );
}

#[test]
fn link_error_maps_to_7() {
    assert_eq!(
        SignalOpError::Link("linked-device limit reached".into()).exit_code(),
        7
    );
}

#[test]
fn every_error_code_is_nonzero_and_in_taxonomy() {
    let all = [
        SignalOpError::Usage("x".into()),
        SignalOpError::Unsupported("x".into()),
        SignalOpError::SignalCli("x".into()),
        SignalOpError::Partial {
            succeeded: 0,
            total: 1,
            failures: vec![],
        },
        SignalOpError::Daemon("x".into()),
        SignalOpError::Link("x".into()),
    ];
    for e in all {
        let code = e.exit_code();
        assert!(code != 0, "no error variant may map to success (0): {e:?}");
        assert!(
            (2..=7).contains(&code),
            "exit code {code} outside documented taxonomy 2..=7 for {e:?}"
        );
    }
}

#[test]
fn error_implements_std_error_and_display() {
    // Must be usable with anyhow / `?` and produce an actionable message.
    let e = SignalOpError::Daemon("bind 127.0.0.1:7583 failed".into());
    let msg = e.to_string();
    assert!(!msg.is_empty());
    let _dyn: &dyn std::error::Error = &e;
}
