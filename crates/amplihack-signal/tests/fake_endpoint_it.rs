//! R3 — hermetic offline testing via a fake signal-cli JSON-RPC endpoint
//! (issue #1002).
//!
//! signal-cli exposes a JSON-RPC 2.0 daemon (newline-delimited JSON over a
//! socket). To exercise `create-group` / `send` / `quit-group` / `receive` and
//! the inbound subscriber loop deterministically in CI — **without ever touching
//! the real Signal network or creating a real group** — we stand up a local
//! loopback fake that speaks the same wire protocol and records what the
//! transport sent.
//!
//! `FakeSignalEndpoint` binds `127.0.0.1:0` (an ephemeral loopback port), so no
//! test can reach an external host. These tests drive the real
//! `SignalTransport` against the fake and assert on the recorded RPCs.
//!
//! RED: `amplihack_signal::fake_endpoint::FakeSignalEndpoint` does not exist yet.
#![cfg(feature = "signal")]

use amplihack_signal::fake_endpoint::FakeSignalEndpoint;
use amplihack_signal::transport::{GroupId, SignalTransport};

#[tokio::test]
async fn endpoint_binds_loopback_only() {
    // Hard guarantee: the fake is loopback-only, so no test can hit the network.
    let fake = FakeSignalEndpoint::start().await.expect("start fake");
    assert!(
        fake.addr().starts_with("127.0.0.1:"),
        "fake endpoint must bind loopback, got {}",
        fake.addr()
    );
}

#[tokio::test]
async fn create_group_returns_fake_group_id_and_is_recorded() {
    let fake = FakeSignalEndpoint::start()
        .await
        .expect("start fake")
        .with_group_id("grp-fake-001==");

    let mut transport = SignalTransport::connect(fake.addr())
        .await
        .expect("connect to fake");

    let gid = transport
        .create_group("amplihack-session-xyz")
        .await
        .expect("create_group over fake");
    assert_eq!(gid, GroupId("grp-fake-001==".to_string()));

    assert!(
        fake.created_groups()
            .contains(&"amplihack-session-xyz".to_string()),
        "fake must record the create-group name; got {:?}",
        fake.created_groups()
    );
}

#[tokio::test]
async fn send_group_is_recorded_with_group_and_body() {
    let fake = FakeSignalEndpoint::start()
        .await
        .expect("start fake")
        .with_group_id("grp-send==");

    let mut transport = SignalTransport::connect(fake.addr()).await.unwrap();
    let gid = GroupId("grp-send==".to_string());
    transport
        .send_group(&gid, "session started")
        .await
        .expect("send over fake");

    assert!(
        fake.sent()
            .contains(&("grp-send==".to_string(), "session started".to_string())),
        "fake must record (group_id, body); got {:?}",
        fake.sent()
    );
}

#[tokio::test]
async fn quit_group_is_recorded() {
    let fake = FakeSignalEndpoint::start()
        .await
        .expect("start fake")
        .with_group_id("grp-quit==");

    let mut transport = SignalTransport::connect(fake.addr()).await.unwrap();
    let gid = GroupId("grp-quit==".to_string());
    transport.quit_group(&gid).await.expect("quit over fake");

    assert!(
        fake.quit_groups().contains(&"grp-quit==".to_string()),
        "fake must record the quit-group; got {:?}",
        fake.quit_groups()
    );
}

#[tokio::test]
async fn receive_parses_an_enqueued_inbound_envelope() {
    // The inbound subscriber loop reads envelopes via `receive`. Enqueue a
    // group dataMessage on the fake and confirm the transport parses it.
    let fake = FakeSignalEndpoint::start().await.expect("start fake");
    fake.enqueue_inbound(
        r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{
            "source":"+15551230001","sourceDevice":1,
            "dataMessage":{"message":"do the thing","groupInfo":{"groupId":"grp-inbound=="}}
        }}}"#,
    );

    let mut transport = SignalTransport::connect(fake.addr()).await.unwrap();
    let env = transport
        .receive()
        .await
        .expect("receive ok")
        .expect("an envelope was delivered");

    assert_eq!(env.source.as_deref(), Some("+15551230001"));
    assert_eq!(env.group_id.as_deref(), Some("grp-inbound=="));
    assert_eq!(env.body.as_deref(), Some("do the thing"));
    assert!(!env.is_sync);
}
