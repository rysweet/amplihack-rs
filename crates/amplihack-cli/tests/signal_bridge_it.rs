//! TDD contract tests for the `amplihack signal bridge <topic>` subcommand
//! (CLI glue). Written **first**; expected to FAIL to compile until the
//! `Bridge(SignalBridgeArgs)` clap variant and the `commands::signal::bridge`
//! module exist.
//!
//! Run: `cargo test -p amplihack-cli --features signal --test signal_bridge_it`.
//!
//! Gated on the `signal` feature so a default build compiles it away (the
//! subcommand's implementation only exists behind `--features signal`).
#![cfg(feature = "signal")]

use amplihack_cli::{SignalBridgeArgs, SignalCommands};
use clap::Parser;

/// Minimal wrapper so the `SignalCommands` subcommand tree can be parsed in
/// isolation, exactly as it will be reached under `amplihack signal ...`.
#[derive(Parser, Debug)]
struct Wrapper {
    #[command(subcommand)]
    cmd: SignalCommands,
}

fn parse_bridge(args: &[&str]) -> SignalBridgeArgs {
    let mut argv = vec!["amplihack-signal-test"];
    argv.extend_from_slice(args);
    match Wrapper::try_parse_from(argv)
        .expect("bridge args must parse")
        .cmd
    {
        SignalCommands::Bridge(a) => a,
        other => panic!("expected Bridge subcommand, got {other:?}"),
    }
}

#[test]
fn topic_is_a_required_positional() {
    let a = parse_bridge(&["bridge", "review PR 3967"]);
    assert_eq!(a.topic, "review PR 3967");
    // Sensible least-privilege defaults when nothing else is passed.
    assert!(
        a.allow_tool.is_empty(),
        "no --allow-tool ⇒ read-only default later"
    );
    assert!(!a.dangerous_all_tools);
    assert!(!a.unsafe_remote_endpoint);
}

#[test]
fn missing_topic_is_a_parse_error() {
    let err = Wrapper::try_parse_from(["amplihack-signal-test", "bridge"]);
    assert!(
        err.is_err(),
        "topic is required; omitting it must fail to parse"
    );
}

#[test]
fn allow_tool_is_repeatable_and_ordered() {
    let a = parse_bridge(&[
        "bridge",
        "topic",
        "--allow-tool",
        "edit",
        "--allow-tool",
        "shell(git commit)",
    ]);
    assert_eq!(a.allow_tool, vec!["edit", "shell(git commit)"]);
    assert!(!a.dangerous_all_tools);
}

#[test]
fn dangerous_all_tools_is_an_explicit_opt_in_flag() {
    let a = parse_bridge(&["bridge", "topic", "--dangerous-all-tools"]);
    assert!(a.dangerous_all_tools);
}

#[test]
fn retry_budget_and_inbox_capacity_are_overridable() {
    let a = parse_bridge(&[
        "bridge",
        "topic",
        "--retry-budget",
        "5",
        "--inbox-capacity",
        "64",
    ]);
    assert_eq!(a.retry_budget, Some(5));
    assert_eq!(a.inbox_capacity, Some(64));
}

#[test]
fn group_and_host_naming_overrides_are_accepted() {
    let a = parse_bridge(&[
        "bridge",
        "topic",
        "--group-name",
        "amplihack-custom",
        "--host",
        "myhost",
    ]);
    assert_eq!(a.group_name.as_deref(), Some("amplihack-custom"));
    assert_eq!(a.host.as_deref(), Some("myhost"));
}

#[test]
fn unsafe_remote_endpoint_is_an_explicit_opt_in_flag() {
    let a = parse_bridge(&["bridge", "topic", "--unsafe-remote-endpoint"]);
    assert!(
        a.unsafe_remote_endpoint,
        "non-loopback endpoints require an explicit documented opt-in"
    );
}

#[test]
fn bridge_variant_reuses_the_shared_six_code_exit_contract() {
    // The CLI maps bridge failures through amplihack-signal's BridgeError so the
    // documented 6-code exit contract has a single source of truth.
    use amplihack_signal::bridge::BridgeError;
    assert_eq!(BridgeError::RemoteEndpointRejected.exit_code(), 2);
    assert_eq!(BridgeError::DaemonUnavailable.exit_code(), 4);
    assert_eq!(BridgeError::ResumeProbeFailed.exit_code(), 5);
}
