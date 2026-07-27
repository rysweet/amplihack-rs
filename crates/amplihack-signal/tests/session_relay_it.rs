//! R4 (re-pointed) — end-to-end session relay over the offline fake endpoint.
//!
//! Originally (issue #1002) this drove the now-deleted `SignalSession` /`Inbox`
//! cross-process path. PR-3 of issue #910 replaces that dead code with
//! `SignalChannel` + the generic `amplihack_turn::run_session_loop`, so this
//! e2e coverage is **re-pointed, not lost**: it now ties the real transport, the
//! real `Gate`, and the generic driver loop together against
//! `FakeSignalEndpoint` — the deterministic, offline proof that the
//! bidirectional channel works without ever creating a real Signal group.
//!
//! `enqueue_inbound` is delivered promptly even after the connection is live, so
//! an echo enqueued *after* an outbound post is still exercised through the gate.
//!
//! RED: `SignalChannel` does not exist yet.
#![cfg(feature = "signal")]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use amplihack_signal::chat::turn::PreemptSlot;
use amplihack_signal::config::{ENV_ACCOUNT, ENV_ALLOWLIST, ENV_ENDPOINT, SignalConfig};
use amplihack_signal::fake_endpoint::FakeSignalEndpoint;
use amplihack_signal::signal_channel::SignalChannel;
use amplihack_signal::transport::{GroupId, SignalTransport};
use amplihack_turn::{AgentSession, Channel, NextPrompt, TurnOutput, TurnResult, run_session_loop};

const ACCOUNT: &str = "+15551230000";
const HANG_GUARD: Duration = Duration::from_secs(10);

fn config_for(addr: &str) -> SignalConfig {
    let mut env = HashMap::new();
    env.insert(ENV_ENDPOINT.to_string(), addr.to_string());
    env.insert(ENV_ACCOUNT.to_string(), ACCOUNT.to_string());
    // Operator commands from their own primary phone (device 1) as the account's
    // synced transcript, so allowlist the account number itself.
    env.insert(ENV_ALLOWLIST.to_string(), ACCOUNT.to_string());
    SignalConfig::from_sources(&env, None).expect("valid config")
}

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

fn fresh_preempt() -> PreemptSlot {
    Arc::new(Mutex::new(None))
}

/// A hermetic mock agent session: records every prompt it runs and echoes it
/// back as the turn output. No real `copilot` process.
struct MockSession {
    id: String,
    seen: Arc<Mutex<Vec<String>>>,
}

impl AgentSession for MockSession {
    async fn run_turn(&mut self, prompt: &str) -> TurnResult<TurnOutput> {
        self.seen.lock().unwrap().push(prompt.to_string());
        Ok(TurnOutput::from_text(format!("ran: {prompt}")))
    }
    fn session_id(&self) -> &str {
        &self.id
    }
}

// =============================================================================
// Outbound (REPLAY) — publish_output mirrors / withholds, fail-closed.
// =============================================================================

#[tokio::test]
async fn outbound_post_is_mirrored_to_the_group() {
    let fake = FakeSignalEndpoint::start()
        .await
        .unwrap()
        .with_group_id("grp-relay==")
        .with_group_members_script(vec![vec![ACCOUNT.to_string()]]);
    let transport = SignalTransport::connect(fake.addr()).await.unwrap();
    let cfg = config_for(fake.addr());
    let mut channel = SignalChannel::new(
        transport,
        &cfg,
        GroupId("grp-relay==".to_string()),
        fresh_preempt(),
        16,
    );

    channel
        .publish_output(&TurnOutput::from_text("assistant turn output mirrored"))
        .await
        .expect("publish_output ok");

    assert!(
        fake.sent()
            .iter()
            .any(|(g, b)| g == "grp-relay==" && b == "assistant turn output mirrored"),
        "the session mirror must post assistant output to the group; got {:?}",
        fake.sent()
    );
}

#[tokio::test]
async fn outbound_post_is_withheld_when_membership_is_unverified() {
    // Security invariant: publish_output fails closed. An unexpected extra member
    // must withhold the outbound post — the relay must not leak agent output to a
    // group that is no longer operator-only.
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
        16,
    );

    // Fail closed: publish returns Ok (the withhold is surfaced, not fatal) but
    // nothing is sent to the group.
    channel
        .publish_output(&TurnOutput::from_text("secret assistant output"))
        .await
        .expect("a withheld post is surfaced, not fatal");

    assert!(
        fake.sent().is_empty(),
        "publish must withhold when an unexpected member is present; leaked {:?}",
        fake.sent()
    );
}

// =============================================================================
// Inbound (LISTEN) — accepted instruction is queued; own echo suppressed.
// =============================================================================

#[tokio::test]
async fn inbound_operator_instruction_is_accepted_and_queued() {
    let fake = FakeSignalEndpoint::start()
        .await
        .unwrap()
        .with_group_id("grp-inbox==");
    let transport = SignalTransport::connect(fake.addr()).await.unwrap();
    let cfg = config_for(fake.addr());
    let mut channel = SignalChannel::new(
        transport,
        &cfg,
        GroupId("grp-inbox==".to_string()),
        fresh_preempt(),
        16,
    );

    fake.enqueue_inbound(&operator_inbound("grp-inbox==", "run the tests again"));

    let np = tokio::time::timeout(HANG_GUARD, async {
        loop {
            match channel.next_prompt().await.expect("next_prompt ok") {
                NextPrompt::Idle => tokio::time::sleep(Duration::from_millis(5)).await,
                other => return other,
            }
        }
    })
    .await
    .expect("did not hang");

    match np {
        NextPrompt::Ready(p) => assert_eq!(
            p, "run the tests again",
            "an allowlisted operator instruction must be delivered as the next prompt"
        ),
        other => panic!("expected Ready, got {other:?}"),
    }
}

#[tokio::test]
async fn own_mirrored_message_is_not_reinjected_as_inbound() {
    // Echo-loop guard: after publishing our own mirror line, its synced-back copy
    // must be suppressed rather than re-ingested as an operator instruction.
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
        16,
    );

    channel
        .publish_output(&TurnOutput::from_text("session started"))
        .await
        .expect("publish_output ok");

    // The account's own message syncs back from device 1 in the same group.
    fake.enqueue_inbound(&operator_inbound("grp-echo==", "session started"));
    tokio::time::sleep(Duration::from_millis(200)).await;

    let np = channel.next_prompt().await.expect("next_prompt ok");
    assert!(
        matches!(np, NextPrompt::Idle),
        "our own mirrored message must not be re-injected as inbound, got {np:?}"
    );
}

// =============================================================================
// Full loop e2e — the generic driver processes an accepted instruction and
// stops cleanly on `stop` (new coverage the old SignalSession path never had).
// =============================================================================

#[tokio::test]
async fn run_session_loop_processes_an_accepted_instruction_then_stops() {
    let fake = FakeSignalEndpoint::start()
        .await
        .unwrap()
        .with_group_id("grp-loop==")
        .with_group_members_script(vec![vec![ACCOUNT.to_string()]]);
    let transport = SignalTransport::connect(fake.addr()).await.unwrap();
    let cfg = config_for(fake.addr());
    let mut channel = SignalChannel::new(
        transport,
        &cfg,
        GroupId("grp-loop==".to_string()),
        fresh_preempt(),
        16,
    );

    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut session = MockSession {
        id: "sid-loop".to_string(),
        seen: seen.clone(),
    };

    // Drive the real generic loop against the SignalChannel in a background task.
    let loop_handle = tokio::spawn(async move {
        let result = run_session_loop(&mut session, &mut channel).await;
        (result, session)
    });

    // Deliver one instruction; give the loop time to run + publish the turn.
    fake.enqueue_inbound(&operator_inbound("grp-loop==", "run the tests again"));
    tokio::time::sleep(Duration::from_millis(250)).await;
    // Then stop: the loop must quit the group and return Ok(()).
    fake.enqueue_inbound(&operator_inbound("grp-loop==", "stop"));

    let (result, session) = tokio::time::timeout(HANG_GUARD, loop_handle)
        .await
        .expect("the loop must terminate on `stop`, not hang")
        .expect("loop task must not panic");

    result.expect("run_session_loop must return Ok on a clean stop");
    assert_eq!(
        *seen.lock().unwrap(),
        vec!["run the tests again".to_string()],
        "the accepted instruction must have been run as exactly one turn (session reused)"
    );
    assert_eq!(
        AgentSession::session_id(&session),
        "sid-loop",
        "the same pinned session drives every turn"
    );
    assert!(
        fake.sent()
            .iter()
            .any(|(g, b)| g == "grp-loop==" && b == "ran: run the tests again"),
        "the turn output must have been published to the group; sent={:?}",
        fake.sent()
    );
    assert!(
        fake.quit_groups().iter().any(|g| g == "grp-loop=="),
        "`stop` must quit the operator-only group; quit_groups={:?}",
        fake.quit_groups()
    );
}
