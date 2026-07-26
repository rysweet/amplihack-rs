//! R4 — end-to-end session relay over the offline fake endpoint (issue #1002).
//!
//! Ties the pieces together against `FakeSignalEndpoint`: a `SignalSession`
//! posts outbound mirror messages (recorded by the fake) and accepts a genuine
//! allowlisted operator instruction while dropping the account's own synced-back
//! echo. This is the deterministic, offline proof that the bidirectional channel
//! works without ever creating a real Signal group.
//!
//! `enqueue_inbound` is delivered promptly even after the connection is live, so
//! an echo enqueued *after* an outbound post is still exercised through the gate.
//!
//! RED: `FakeSignalEndpoint` does not exist yet.
#![cfg(feature = "signal")]

use std::collections::HashMap;

use amplihack_signal::config::{ENV_ACCOUNT, ENV_ALLOWLIST, ENV_ENDPOINT, SignalConfig};
use amplihack_signal::fake_endpoint::FakeSignalEndpoint;
use amplihack_signal::session_channel::{Inbox, SignalSession};
use amplihack_signal::transport::{GroupId, SignalTransport};
use tempfile::TempDir;

fn config_for(addr: &str) -> SignalConfig {
    let mut env = HashMap::new();
    env.insert(ENV_ENDPOINT.to_string(), addr.to_string());
    env.insert(ENV_ACCOUNT.to_string(), "+15551230000".to_string());
    // Operator commands from their own primary phone (device 1) as the account's
    // synced transcript, so allowlist the account number itself.
    env.insert(ENV_ALLOWLIST.to_string(), "+15551230000".to_string());
    SignalConfig::from_sources(&env, None).expect("valid config")
}

#[tokio::test]
async fn outbound_post_is_mirrored_to_the_group() {
    let fake = FakeSignalEndpoint::start()
        .await
        .unwrap()
        .with_group_id("grp-relay==");
    let transport = SignalTransport::connect(fake.addr()).await.unwrap();
    let dir = TempDir::new().unwrap();
    let inbox = Inbox::new(dir.path().join("inbox.json"), 16);

    let cfg = config_for(fake.addr());
    let mut session =
        SignalSession::new(transport, &cfg, GroupId("grp-relay==".to_string()), inbox);

    session
        .post("assistant turn output mirrored")
        .await
        .unwrap();

    assert!(
        fake.sent()
            .iter()
            .any(|(g, b)| g == "grp-relay==" && b == "assistant turn output mirrored"),
        "the whole-session mirror must post assistant output to the group; got {:?}",
        fake.sent()
    );
}

#[tokio::test]
async fn inbound_operator_instruction_is_accepted_and_queued() {
    let fake = FakeSignalEndpoint::start()
        .await
        .unwrap()
        .with_group_id("grp-inbox==");
    let transport = SignalTransport::connect(fake.addr()).await.unwrap();
    let dir = TempDir::new().unwrap();
    let inbox = Inbox::new(dir.path().join("inbox.json"), 16);
    let cfg = config_for(fake.addr());
    let mut session =
        SignalSession::new(transport, &cfg, GroupId("grp-inbox==".to_string()), inbox);

    // Operator types on their primary phone → account's own sync transcript,
    // device 1, in this session's group: a legitimate instruction.
    fake.enqueue_inbound(
        r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{
            "source":"+15551230000","sourceDevice":1,
            "syncMessage":{"sentMessage":{"message":"run the tests again",
                "groupInfo":{"groupId":"grp-inbox=="}}}
        }}}"#,
    );

    let accepted = session.pump_once().await.expect("pump ok");
    assert_eq!(
        accepted.as_deref(),
        Some("run the tests again"),
        "an allowlisted operator instruction must be accepted"
    );
    assert_eq!(
        session.drain().unwrap(),
        vec!["run the tests again".to_string()],
        "accepted instruction must be queued for injection into the agent"
    );
}

#[tokio::test]
async fn own_mirrored_message_is_not_reinjected_as_inbound() {
    // Echo-loop guard: after posting our own mirror line, its synced-back copy
    // must be suppressed rather than re-ingested as an operator instruction.
    let fake = FakeSignalEndpoint::start()
        .await
        .unwrap()
        .with_group_id("grp-echo==");
    let transport = SignalTransport::connect(fake.addr()).await.unwrap();
    let dir = TempDir::new().unwrap();
    let inbox = Inbox::new(dir.path().join("inbox.json"), 16);
    let cfg = config_for(fake.addr());
    let mut session = SignalSession::new(transport, &cfg, GroupId("grp-echo==".to_string()), inbox);

    session.post("session started").await.unwrap();

    // The account's own message syncs back from device 1 in the same group.
    fake.enqueue_inbound(
        r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{
            "source":"+15551230000","sourceDevice":1,
            "syncMessage":{"sentMessage":{"message":"session started",
                "groupInfo":{"groupId":"grp-echo=="}}}
        }}}"#,
    );

    let pumped = session.pump_once().await.expect("pump ok");
    // Gate returns None-instruction (rendered as an empty accepted string) for a
    // suppressed echo; either way it must never enqueue our own words.
    assert!(
        session.drain().unwrap().is_empty(),
        "our own mirrored message must not be re-injected as inbound (got pumped={pumped:?})"
    );
}
