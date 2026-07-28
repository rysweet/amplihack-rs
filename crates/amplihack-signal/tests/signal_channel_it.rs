//! TDD (RED) contract tests for `SignalChannel` — PR-3 of issue #910.
//!
//! Written **first**: these FAIL to compile until PR-3 adds
//! `crates/amplihack-signal/src/signal_channel.rs` with a `SignalChannel` that
//! implements `amplihack_turn::Channel`. See
//! `docs/signal-channel-turn-loop.md`.
//!
//! Behaviour these lock (must be identical to today's hand-rolled
//! `run_chat_async` loop — behaviour-preserving PR):
//!   * `next_prompt()` yields accepted prompts in FIFO order.
//!   * the bounded turn queue evicts the OLDEST at capacity (operator policy;
//!     no new fixed cap) and keeps the newest.
//!   * `next_prompt()` returns `Idle` when there is nothing to run (the loop
//!     waits, NO wall-clock timeout) and `Closed` once shut down.
//!   * inbound gating is FAIL CLOSED: a non-allowlisted sender is rejected and
//!     never enqueued; an empty allowlist denies ALL inbound.
//!   * a `status` command posts the status line and does NOT enqueue a turn.
//!   * a `stop` command quits the group and drives the channel to `Closed`.
//!   * `publish_output` re-verifies membership FAIL CLOSED before every post,
//!     redacts secrets before chunking, and records the send in the echo window
//!     so our own synced-back copy is not re-ingested.
//!   * `id()` is the group id.
//!
//! Hermetic: everything runs against the in-process loopback `FakeSignalEndpoint`
//! — no real Signal network, no real group.
//!
//! Run: `cargo test -p amplihack-signal --features signal --test signal_channel_it`.
#![cfg(feature = "signal")]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use amplihack_signal::chat::turn::PreemptSlot;
use amplihack_signal::config::{ENV_ACCOUNT, ENV_ALLOWLIST, ENV_ENDPOINT, SignalConfig};
use amplihack_signal::fake_endpoint::FakeSignalEndpoint;
use amplihack_signal::signal_channel::SignalChannel;
use amplihack_signal::transport::{GroupId, SignalTransport};
use amplihack_turn::{Channel, NextPrompt, TurnOutput};

/// Defensive hang guard: a mis-implemented `Idle` wait that never resolves must
/// fail the test rather than wedge the whole suite.
const HANG_GUARD: Duration = Duration::from_secs(10);

const ACCOUNT: &str = "+15551230000";

/// Operator-only config: the account number is both the account and the sole
/// allowlisted sender (operator types on their own primary phone → device 1
/// synced transcript), mirroring `session_relay_it`.
fn config_for(addr: &str) -> SignalConfig {
    let mut env = HashMap::new();
    env.insert(ENV_ENDPOINT.to_string(), addr.to_string());
    env.insert(ENV_ACCOUNT.to_string(), ACCOUNT.to_string());
    env.insert(ENV_ALLOWLIST.to_string(), ACCOUNT.to_string());
    SignalConfig::from_sources(&env, None).expect("valid config")
}

/// Config whose allowlist is EMPTY → fail-closed deny-all inbound.
fn config_empty_allowlist(addr: &str) -> SignalConfig {
    let mut env = HashMap::new();
    env.insert(ENV_ENDPOINT.to_string(), addr.to_string());
    env.insert(ENV_ACCOUNT.to_string(), ACCOUNT.to_string());
    env.insert(ENV_ALLOWLIST.to_string(), String::new());
    SignalConfig::from_sources(&env, None).expect("valid config")
}

/// A legitimate operator instruction: the account's own synced transcript
/// (device 1) in this session's group.
fn operator_inbound(group_id: &str, message: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","method":"receive","params":{{"envelope":{{
            "source":"{ACCOUNT}","sourceDevice":1,
            "syncMessage":{{"sentMessage":{{"message":{message},
                "groupInfo":{{"groupId":"{group_id}"}}}}}}
        }}}}}}"#,
        message = serde_json::to_string(message).unwrap(),
    )
}

/// An inbound from a NON-allowlisted stranger in the same group.
fn stranger_inbound(group_id: &str, message: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","method":"receive","params":{{"envelope":{{
            "source":"+19998887777","sourceDevice":1,
            "dataMessage":{{"message":{message},
                "groupInfo":{{"groupId":"{group_id}"}}}}
        }}}}}}"#,
        message = serde_json::to_string(message).unwrap(),
    )
}

fn fresh_preempt() -> PreemptSlot {
    Arc::new(Mutex::new(None))
}

/// Poll `next_prompt` until it yields a non-`Idle` answer (or hangs). This is
/// exactly what `run_session_loop` does: `Idle` means "wait, no timeout".
async fn next_non_idle(channel: &mut SignalChannel) -> NextPrompt {
    tokio::time::timeout(HANG_GUARD, async {
        loop {
            match channel
                .next_prompt()
                .await
                .expect("next_prompt must not error")
            {
                NextPrompt::Idle => tokio::time::sleep(Duration::from_millis(5)).await,
                other => return other,
            }
        }
    })
    .await
    .expect("next_prompt never resolved to Ready/Closed (idle-wait hang?)")
}

fn expect_ready(np: NextPrompt) -> String {
    match np {
        NextPrompt::Ready(p) => p,
        other => panic!("expected NextPrompt::Ready, got {other:?}"),
    }
}

/// Sleep long enough for the background I/O actor to drain the fake's enqueued
/// inbound frames and settle the turn queue before we assert on it.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(200)).await;
}

// =============================================================================
// LISTEN: next_prompt / queue semantics
// =============================================================================

#[tokio::test]
async fn next_prompt_returns_accepted_prompts_in_fifo_order() {
    let fake = FakeSignalEndpoint::start()
        .await
        .unwrap()
        .with_group_id("grp-fifo==")
        .with_group_members_script(vec![vec![ACCOUNT.to_string()]]);
    let transport = SignalTransport::connect(fake.addr()).await.unwrap();
    let cfg = config_for(fake.addr());
    let mut channel = SignalChannel::new(
        transport,
        &cfg,
        GroupId("grp-fifo==".to_string()),
        fresh_preempt(),
        16,
    );

    fake.enqueue_inbound(&operator_inbound("grp-fifo==", "first prompt"));
    fake.enqueue_inbound(&operator_inbound("grp-fifo==", "second prompt"));

    assert_eq!(
        expect_ready(next_non_idle(&mut channel).await),
        "first prompt"
    );
    assert_eq!(
        expect_ready(next_non_idle(&mut channel).await),
        "second prompt",
        "accepted prompts must be delivered in FIFO order"
    );
}

#[tokio::test]
async fn queue_evicts_oldest_at_capacity() {
    // Capacity 2, three accepted prompts flooded before we drain: the OLDEST is
    // evicted (operator-configurable bounded queue), the two NEWEST survive in
    // order. No new fixed cap is introduced.
    let fake = FakeSignalEndpoint::start()
        .await
        .unwrap()
        .with_group_id("grp-cap==")
        .with_group_members_script(vec![vec![ACCOUNT.to_string()]]);
    let transport = SignalTransport::connect(fake.addr()).await.unwrap();
    let cfg = config_for(fake.addr());
    let mut channel = SignalChannel::new(
        transport,
        &cfg,
        GroupId("grp-cap==".to_string()),
        fresh_preempt(),
        2,
    );

    fake.enqueue_inbound(&operator_inbound("grp-cap==", "oldest"));
    fake.enqueue_inbound(&operator_inbound("grp-cap==", "middle"));
    fake.enqueue_inbound(&operator_inbound("grp-cap==", "newest"));
    // Let the actor accept & enqueue all three before we start draining.
    settle().await;

    assert_eq!(
        expect_ready(next_non_idle(&mut channel).await),
        "middle",
        "the oldest prompt must be evicted at capacity; the next-oldest survivor is first"
    );
    assert_eq!(
        expect_ready(next_non_idle(&mut channel).await),
        "newest",
        "the newest prompt must survive eviction"
    );
}

#[tokio::test]
async fn next_prompt_is_idle_when_queue_empty() {
    // Nothing enqueued: next_prompt must report Idle (never spin, never Closed,
    // never block forever) so the loop simply waits — with NO wall-clock cap.
    let fake = FakeSignalEndpoint::start()
        .await
        .unwrap()
        .with_group_id("grp-idle==");
    let transport = SignalTransport::connect(fake.addr()).await.unwrap();
    let cfg = config_for(fake.addr());
    let mut channel = SignalChannel::new(
        transport,
        &cfg,
        GroupId("grp-idle==".to_string()),
        fresh_preempt(),
        8,
    );

    let np = tokio::time::timeout(HANG_GUARD, channel.next_prompt())
        .await
        .expect("next_prompt must return promptly when idle, not hang")
        .expect("next_prompt must not error");
    assert!(
        matches!(np, NextPrompt::Idle),
        "an empty queue must yield NextPrompt::Idle, got {np:?}"
    );
}

// =============================================================================
// FAIL-CLOSED inbound gate
// =============================================================================

#[tokio::test]
async fn gate_fail_closed_rejects_unauthorized_inbound() {
    // A stranger (not on the allowlist) sends a message in the group. The gate
    // must reject it: it is never enqueued, so next_prompt stays Idle.
    let fake = FakeSignalEndpoint::start()
        .await
        .unwrap()
        .with_group_id("grp-reject==");
    let transport = SignalTransport::connect(fake.addr()).await.unwrap();
    let cfg = config_for(fake.addr());
    let mut channel = SignalChannel::new(
        transport,
        &cfg,
        GroupId("grp-reject==".to_string()),
        fresh_preempt(),
        8,
    );

    fake.enqueue_inbound(&stranger_inbound("grp-reject==", "please run rm -rf /"));
    settle().await;

    let np = channel.next_prompt().await.expect("next_prompt ok");
    assert!(
        matches!(np, NextPrompt::Idle),
        "a non-allowlisted sender must be rejected fail-closed and never enqueued, got {np:?}"
    );
}

#[tokio::test]
async fn empty_allowlist_denies_all_inbound() {
    // With an empty allowlist EVERY inbound is denied — even the account's own
    // synced transcript. Fail-closed deny-all must be preserved verbatim.
    let fake = FakeSignalEndpoint::start()
        .await
        .unwrap()
        .with_group_id("grp-deny==");
    let transport = SignalTransport::connect(fake.addr()).await.unwrap();
    let cfg = config_empty_allowlist(fake.addr());
    let mut channel = SignalChannel::new(
        transport,
        &cfg,
        GroupId("grp-deny==".to_string()),
        fresh_preempt(),
        8,
    );

    fake.enqueue_inbound(&operator_inbound("grp-deny==", "do the thing"));
    settle().await;

    let np = channel.next_prompt().await.expect("next_prompt ok");
    assert!(
        matches!(np, NextPrompt::Idle),
        "an empty allowlist must deny all inbound (nothing enqueued), got {np:?}"
    );
}

// =============================================================================
// Control commands: status (no enqueue) and stop (quit + Closed)
// =============================================================================

#[tokio::test]
async fn status_command_posts_status_without_enqueuing_a_turn() {
    let fake = FakeSignalEndpoint::start()
        .await
        .unwrap()
        .with_group_id("grp-status==")
        // Membership verifies so the status line is actually posted.
        .with_group_members_script(vec![vec![ACCOUNT.to_string()]]);
    let transport = SignalTransport::connect(fake.addr()).await.unwrap();
    let cfg = config_for(fake.addr());
    let mut channel = SignalChannel::new(
        transport,
        &cfg,
        GroupId("grp-status==".to_string()),
        fresh_preempt(),
        8,
    );

    fake.enqueue_inbound(&operator_inbound("grp-status==", "status"));
    settle().await;

    // The status line was posted to the group ...
    assert!(
        fake.sent()
            .iter()
            .any(|(g, b)| g == "grp-status==" && b.contains("status")),
        "a `status` command must post a status line to the group; got {:?}",
        fake.sent()
    );
    // ... and it did NOT enqueue a turn.
    let np = channel.next_prompt().await.expect("next_prompt ok");
    assert!(
        matches!(np, NextPrompt::Idle),
        "`status` must not enqueue a turn prompt, got {np:?}"
    );
}

#[tokio::test]
async fn stop_command_quits_group_and_closes_the_channel() {
    let fake = FakeSignalEndpoint::start()
        .await
        .unwrap()
        .with_group_id("grp-stop==")
        .with_group_members_script(vec![vec![ACCOUNT.to_string()]]);
    let transport = SignalTransport::connect(fake.addr()).await.unwrap();
    let cfg = config_for(fake.addr());
    let mut channel = SignalChannel::new(
        transport,
        &cfg,
        GroupId("grp-stop==".to_string()),
        fresh_preempt(),
        8,
    );

    fake.enqueue_inbound(&operator_inbound("grp-stop==", "stop"));

    let np = next_non_idle(&mut channel).await;
    assert!(
        matches!(np, NextPrompt::Closed),
        "`stop` must drive the channel to NextPrompt::Closed, got {np:?}"
    );
    settle().await;
    assert!(
        fake.quit_groups().iter().any(|g| g == "grp-stop=="),
        "`stop` must quit the operator-only group; quit_groups={:?}",
        fake.quit_groups()
    );
}

#[tokio::test]
async fn publish_output_is_a_silent_noop_after_stop_closes_the_channel() {
    // After `stop` quits the group and shuts the actor down, an in-flight turn
    // that was preempted still hands its (empty) output to `publish_output`.
    // The channel is already closed, so this must be a silent no-op — no post,
    // no spurious `cannot post — actor shut down` diagnostic.
    let fake = FakeSignalEndpoint::start()
        .await
        .unwrap()
        .with_group_id("grp-stopnoop==")
        .with_group_members_script(vec![vec![ACCOUNT.to_string()]]);
    let transport = SignalTransport::connect(fake.addr()).await.unwrap();
    let cfg = config_for(fake.addr());
    let mut channel = SignalChannel::new(
        transport,
        &cfg,
        GroupId("grp-stopnoop==".to_string()),
        fresh_preempt(),
        8,
    );

    fake.enqueue_inbound(&operator_inbound("grp-stopnoop==", "stop"));
    let np = next_non_idle(&mut channel).await;
    assert!(
        matches!(np, NextPrompt::Closed),
        "`stop` must drive the channel to NextPrompt::Closed, got {np:?}"
    );
    settle().await;

    let sent_before = fake.sent().len();
    channel
        .publish_output(&TurnOutput::from_text(""))
        .await
        .expect("publish_output on a closed channel must return Ok");
    settle().await;

    assert_eq!(
        fake.sent().len(),
        sent_before,
        "publish_output on a closed channel must not post anything; sent={:?}",
        fake.sent()
    );
}

#[tokio::test]
async fn status_word_inside_a_sentence_stays_a_prompt() {
    // Control words match only as the entire trimmed body. A prompt that merely
    // *mentions* "status" must be enqueued as a normal turn, not intercepted.
    let fake = FakeSignalEndpoint::start()
        .await
        .unwrap()
        .with_group_id("grp-word==")
        .with_group_members_script(vec![vec![ACCOUNT.to_string()]]);
    let transport = SignalTransport::connect(fake.addr()).await.unwrap();
    let cfg = config_for(fake.addr());
    let mut channel = SignalChannel::new(
        transport,
        &cfg,
        GroupId("grp-word==".to_string()),
        fresh_preempt(),
        8,
    );

    fake.enqueue_inbound(&operator_inbound(
        "grp-word==",
        "what is the status of the build?",
    ));
    assert_eq!(
        expect_ready(next_non_idle(&mut channel).await),
        "what is the status of the build?",
        "a sentence merely containing a control word must remain a normal prompt"
    );
}

// =============================================================================
// REPLAY: publish_output — fail-closed membership, redaction, echo window
// =============================================================================

#[tokio::test]
async fn publish_output_posts_when_membership_is_verified() {
    let fake = FakeSignalEndpoint::start()
        .await
        .unwrap()
        .with_group_id("grp-pub==")
        .with_group_members_script(vec![vec![ACCOUNT.to_string()]]);
    let transport = SignalTransport::connect(fake.addr()).await.unwrap();
    let cfg = config_for(fake.addr());
    let mut channel = SignalChannel::new(
        transport,
        &cfg,
        GroupId("grp-pub==".to_string()),
        fresh_preempt(),
        8,
    );

    channel
        .publish_output(&TurnOutput::from_text("assistant turn output"))
        .await
        .expect("publish_output must succeed on a verified group");

    assert!(
        fake.sent()
            .iter()
            .any(|(g, b)| g == "grp-pub==" && b == "assistant turn output"),
        "verified membership must post the turn output; sent={:?}",
        fake.sent()
    );
}

#[tokio::test]
async fn publish_output_withholds_when_membership_unverified() {
    // An unexpected extra member fails the pre-post re-check. publish_output must
    // withhold (nothing sent) and NOT crash — the withhold is surfaced, not fatal,
    // so it returns Ok (matching today's verify_and_post behaviour).
    let fake = FakeSignalEndpoint::start()
        .await
        .unwrap()
        .with_group_id("grp-leak==")
        .with_group_members_script(vec![vec![ACCOUNT.to_string(), "+15559999999".to_string()]]);
    let transport = SignalTransport::connect(fake.addr()).await.unwrap();
    let cfg = config_for(fake.addr());
    let mut channel = SignalChannel::new(
        transport,
        &cfg,
        GroupId("grp-leak==".to_string()),
        fresh_preempt(),
        8,
    );

    channel
        .publish_output(&TurnOutput::from_text("secret assistant output"))
        .await
        .expect("a withheld post is surfaced, not a fatal error");

    assert!(
        fake.sent().is_empty(),
        "publish_output must withhold when an unexpected member is present; leaked {:?}",
        fake.sent()
    );
}

#[tokio::test]
async fn publish_output_redacts_secrets_before_posting() {
    // All outbound bodies are redacted before chunking. A credential in the turn
    // output must never reach the group verbatim.
    let fake = FakeSignalEndpoint::start()
        .await
        .unwrap()
        .with_group_id("grp-redact==")
        .with_group_members_script(vec![vec![ACCOUNT.to_string()]]);
    let transport = SignalTransport::connect(fake.addr()).await.unwrap();
    let cfg = config_for(fake.addr());
    let mut channel = SignalChannel::new(
        transport,
        &cfg,
        GroupId("grp-redact==".to_string()),
        fresh_preempt(),
        8,
    );

    let secret = "AKIAIOSFODNN7EXAMPLE";
    channel
        .publish_output(&TurnOutput::from_text(format!(
            "here is the key {secret} ok"
        )))
        .await
        .expect("publish_output ok");

    let sent = fake.sent();
    assert!(
        !sent.iter().any(|(_, b)| b.contains(secret)),
        "the raw AWS key must never be posted; sent={sent:?}"
    );
    assert!(
        sent.iter().any(|(_, b)| b.contains("[REDACTED-AWS-KEY]")),
        "the secret must be replaced by its redaction placeholder; sent={sent:?}"
    );
}

#[tokio::test]
async fn own_published_message_is_not_reingested_as_inbound() {
    // Echo-loop guard: after publish_output records the outbound in the echo
    // window, the account's synced-back copy of that exact text must be
    // suppressed by the gate rather than re-ingested as an operator prompt.
    let fake = FakeSignalEndpoint::start()
        .await
        .unwrap()
        .with_group_id("grp-echo==")
        .with_group_members_script(vec![vec![ACCOUNT.to_string()]]);
    let transport = SignalTransport::connect(fake.addr()).await.unwrap();
    let cfg = config_for(fake.addr());
    let mut channel = SignalChannel::new(
        transport,
        &cfg,
        GroupId("grp-echo==".to_string()),
        fresh_preempt(),
        8,
    );

    channel
        .publish_output(&TurnOutput::from_text("session started"))
        .await
        .expect("publish_output ok");

    // Our own post syncs back from device 1 in the same group.
    fake.enqueue_inbound(&operator_inbound("grp-echo==", "session started"));
    settle().await;

    let np = channel.next_prompt().await.expect("next_prompt ok");
    assert!(
        matches!(np, NextPrompt::Idle),
        "our own mirrored message must not be re-ingested as an inbound prompt, got {np:?}"
    );
}

#[tokio::test]
async fn id_is_the_group_id() {
    let fake = FakeSignalEndpoint::start()
        .await
        .unwrap()
        .with_group_id("grp-id==");
    let transport = SignalTransport::connect(fake.addr()).await.unwrap();
    let cfg = config_for(fake.addr());
    let channel = SignalChannel::new(
        transport,
        &cfg,
        GroupId("grp-id==".to_string()),
        fresh_preempt(),
        8,
    );
    assert_eq!(
        channel.id().to_string(),
        "grp-id==",
        "the channel id must be the resolved operator-only group id"
    );
}
