//! TDD contract tests for the `amplihack signal chat <topic>` subcommand
//! (CLI glue). Written **first**; expected to FAIL to compile until the
//! `Chat(SignalChatArgs)` clap variant and the `commands::signal::chat`
//! module exist.
//!
//! Run: `cargo test -p amplihack-cli --features signal --test cli_integration signal_chat_it::`.
//!
//! Gated on the `signal` feature so a default build compiles it away (the
//! subcommand's implementation only exists behind `--features signal`).
#![cfg(feature = "signal")]

use amplihack_cli::{SignalChatArgs, SignalCommands};
use clap::Parser;

/// Minimal wrapper so the `SignalCommands` subcommand tree can be parsed in
/// isolation, exactly as it will be reached under `amplihack signal ...`.
#[derive(Parser, Debug)]
struct Wrapper {
    #[command(subcommand)]
    cmd: SignalCommands,
}

fn parse_chat(args: &[&str]) -> SignalChatArgs {
    let mut argv = vec!["amplihack-signal-test"];
    argv.extend_from_slice(args);
    match Wrapper::try_parse_from(argv)
        .expect("chat args must parse")
        .cmd
    {
        SignalCommands::Chat(a) => a,
        other => panic!("expected Chat subcommand, got {other:?}"),
    }
}

#[test]
fn topic_is_a_required_positional() {
    let a = parse_chat(&["chat", "review PR 3967"]);
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
    let err = Wrapper::try_parse_from(["amplihack-signal-test", "chat"]);
    assert!(
        err.is_err(),
        "topic is required; omitting it must fail to parse"
    );
}

#[test]
fn allow_tool_is_repeatable_and_ordered() {
    let a = parse_chat(&[
        "chat",
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
    let a = parse_chat(&["chat", "topic", "--dangerous-all-tools"]);
    assert!(a.dangerous_all_tools);
}

#[test]
fn retry_budget_and_inbox_capacity_are_overridable() {
    let a = parse_chat(&[
        "chat",
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
    let a = parse_chat(&[
        "chat",
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
    let a = parse_chat(&["chat", "topic", "--unsafe-remote-endpoint"]);
    assert!(
        a.unsafe_remote_endpoint,
        "non-loopback endpoints require an explicit documented opt-in"
    );
}

#[test]
fn chat_variant_reuses_the_shared_six_code_exit_contract() {
    // The CLI maps chat failures through amplihack-signal's ChatError so the
    // documented 6-code exit contract has a single source of truth.
    use amplihack_signal::chat::ChatError;
    assert_eq!(ChatError::RemoteEndpointRejected.exit_code(), 2);
    assert_eq!(ChatError::DaemonUnavailable.exit_code(), 4);
    assert_eq!(ChatError::ResumeProbeFailed.exit_code(), 5);
}

// ---------------------------------------------------------------------------
// FIX 3 (F6) — re-verify group membership before EVERY outbound post.
//
// TDD contract, written **first**. `verify_and_post` currently verifies
// membership ONCE and then sends every redacted chunk. The security posture
// promises verification "before EVERY post": a member added mid-body must not
// receive later chunks. After F6, membership is re-checked (fail-closed)
// immediately before EACH `send_group` chunk; on any mid-body verification
// failure the remaining chunks are withheld (surfaced, never silently dropped).
//
// These tests drive the REAL `verify_and_post` against the in-process
// `FakeSignalEndpoint` (loopback-only, no Signal network). They require:
//   * `verify_and_post` to be reachable (exposed `pub` behind `signal`), and
//   * the fake to answer `listGroups` from a per-call script of member sets,
//     so a membership change can occur *between* chunk posts.
// Both are part of the F6 implementation; until then this file is RED.
//
// Run: `cargo test -p amplihack-cli --features signal --test cli_integration signal_chat_it::`.
mod fix3_per_post_reverification {
    use amplihack_cli::commands::signal::chat::verify_and_post;
    use amplihack_signal::chat::outbound::redact_and_chunk;
    use amplihack_signal::config::{ENV_ACCOUNT, ENV_ALLOWLIST, ENV_ENDPOINT, SignalConfig};
    use amplihack_signal::fake_endpoint::FakeSignalEndpoint;
    use amplihack_signal::gating::Gate;
    use amplihack_signal::transport::{GroupId, SignalTransport};
    use std::collections::HashMap;

    const GROUP: &str = "grp-post==";
    const ACCOUNT: &str = "+15551230000";
    const OPERATOR: &str = "+12065551234";

    fn config_for(addr: &str) -> SignalConfig {
        let mut env = HashMap::new();
        env.insert(ENV_ENDPOINT.to_string(), addr.to_string());
        env.insert(ENV_ACCOUNT.to_string(), ACCOUNT.to_string());
        env.insert(ENV_ALLOWLIST.to_string(), OPERATOR.to_string());
        SignalConfig::from_sources(&env, None).expect("valid config")
    }

    /// The operator-only expected set: allowlist + account.
    fn expected() -> Vec<String> {
        vec![OPERATOR.to_string(), ACCOUNT.to_string()]
    }

    /// A body large enough to chunk into several outbound messages (each capped
    /// at 2000 bytes), so "before EVERY post" is observable. Plain ASCII text so
    /// redaction leaves it untouched.
    fn multi_chunk_body() -> String {
        "a".repeat(5000)
    }

    #[tokio::test]
    async fn membership_change_mid_body_withholds_remaining_chunks() {
        let body = multi_chunk_body();
        let chunk_count = redact_and_chunk(&body).len();
        assert!(chunk_count >= 2, "test body must span multiple chunks");

        // listGroups script: the FIRST verification sees the correct operator
        // set (Verified → first chunk posts); the SECOND sees an unexpected
        // extra member (Unverified → remaining chunks withheld).
        let ok = expected();
        let mut altered = ok.clone();
        altered.push("+19998887777".to_string());

        let fake = FakeSignalEndpoint::start()
            .await
            .unwrap()
            .with_group_id(GROUP)
            .with_group_members_script(vec![ok.clone(), altered]);

        let mut transport = SignalTransport::connect(fake.addr()).await.unwrap();
        let cfg = config_for(fake.addr());
        let mut gate = Gate::new(&cfg, GROUP);

        verify_and_post(
            &mut transport,
            &GroupId(GROUP.to_string()),
            &expected(),
            &mut gate,
            &body,
        )
        .await;

        let sent = fake.sent();
        assert_eq!(
            sent.len(),
            1,
            "membership changed after the first post: later chunks must be withheld, got {sent:?}"
        );
        assert!(
            sent.iter().all(|(g, _)| g == GROUP),
            "the single posted chunk must target the session group"
        );
    }

    #[tokio::test]
    async fn stable_verified_membership_posts_all_chunks() {
        // Control: if membership stays verified across every re-check, per-post
        // verification must NOT over-withhold — all chunks are delivered.
        let body = multi_chunk_body();
        let chunk_count = redact_and_chunk(&body).len();

        let ok = expected();
        // A single script entry repeats for every listGroups call → always
        // Verified, however many times verification runs.
        let fake = FakeSignalEndpoint::start()
            .await
            .unwrap()
            .with_group_id(GROUP)
            .with_group_members_script(vec![ok.clone()]);

        let mut transport = SignalTransport::connect(fake.addr()).await.unwrap();
        let cfg = config_for(fake.addr());
        let mut gate = Gate::new(&cfg, GROUP);

        verify_and_post(
            &mut transport,
            &GroupId(GROUP.to_string()),
            &expected(),
            &mut gate,
            &body,
        )
        .await;

        assert_eq!(
            fake.sent().len(),
            chunk_count,
            "stable verified membership must deliver every chunk"
        );
    }

    #[tokio::test]
    async fn first_verification_failure_withholds_all_chunks() {
        // If membership is already unverifiable before the first post, nothing
        // is sent at all (fail-closed from the very first chunk).
        let mut wrong = expected();
        wrong.push("+19998887777".to_string());

        let fake = FakeSignalEndpoint::start()
            .await
            .unwrap()
            .with_group_id(GROUP)
            .with_group_members_script(vec![wrong]);

        let mut transport = SignalTransport::connect(fake.addr()).await.unwrap();
        let cfg = config_for(fake.addr());
        let mut gate = Gate::new(&cfg, GROUP);

        verify_and_post(
            &mut transport,
            &GroupId(GROUP.to_string()),
            &expected(),
            &mut gate,
            &multi_chunk_body(),
        )
        .await;

        assert!(
            fake.sent().is_empty(),
            "an unverified group before the first post must receive nothing"
        );
    }
}
