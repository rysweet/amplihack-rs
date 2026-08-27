//! Regression — FIX 1: the inbound receive path must be **cancel-safe**.
//!
//! The chat's subscriber loop polls `transport.receive()` as one arm of a
//! `biased` `tokio::select!`. When a competing arm wins while `receive()` is
//! suspended mid-frame, its future is dropped. The old `read_line` cleared its
//! frame buffer at the top of each call and `consume()`d partial chunks before
//! seeing the terminating newline, so the already-consumed prefix bytes of a
//! frame split across TCP segments were silently lost — a large operator
//! message fragmented across TCP segments would vanish, violating the feature's
//! "never silent" promise.
//!
//! The fix makes `read_line` persist the in-progress frame across a dropped
//! future (it clears only after a complete frame is assembled), and has the
//! shared-connection RPC path (`request`, used by `group_members`/`send_group`)
//! **queue** any inbound `receive` notification it encounters while awaiting its
//! reply — delivered by the next `receive()`, never dropped.
//!
//! This test drives the real adversarial interleaving the chat can produce —
//! a notification fragmented across two TCP segments, the `receive()` future
//! dropped mid-frame (a competing select arm wins), and an intervening
//! `group_members()` RPC on the SAME connection — and asserts the notification
//! is still delivered **intact** and **exactly once**, and the RPC still
//! succeeds.
//!
//! Wire ordering note: signal-cli writes each JSON-RPC frame atomically, so a
//! notification whose transmission has already begun delivers its remaining
//! bytes *before* the response to a request we send mid-frame. The fake server
//! below reproduces exactly that ordering (notification tail first, then the
//! RPC response) — the physically realistic case, not a frame-splitting one
//! that no real daemon emits.
//!
//! Run: `cargo test -p amplihack-signal --features signal --test
//! transport_cancel_safe_it`.
#![cfg(feature = "signal")]

use std::time::Duration;

use amplihack_signal::transport::{GroupId, SignalTransport};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

/// One inbound group `receive` notification. It is split across two TCP writes
/// at a byte boundary that falls in the middle of the JSON (so neither half is
/// a valid frame on its own, and the newline only arrives with the second
/// segment).
const FRAME: &str = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"+15551230001","sourceDevice":1,"dataMessage":{"message":"a large pasted prompt fragmented across TCP segments","groupInfo":{"groupId":"grp-frag=="}}}}}"#;

/// A `listGroups` response whose members are a valid, positively-known set —
/// enough for the intervening `group_members()` RPC to succeed.
const LIST_GROUPS_RESULT: &str =
    r#"[{"id":"grp-frag==","members":[{"number":"+15551230001"},{"number":"+15551230000"}]}]"#;

#[tokio::test]
async fn fragmented_inbound_frame_survives_cancellation_and_interleaved_request() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    // Server drives the realistic adversarial interleaving:
    //   1. write the first half of the notification (no newline) and flush;
    //   2. block until it reads the client's `listGroups` request line — the
    //      synchronization point proving the client has already dropped its
    //      mid-frame `receive()` future and issued the intervening RPC;
    //   3. write the SECOND half of the notification + newline, completing the
    //      atomic notification frame (this precedes the response, exactly as a
    //      real daemon that had begun writing the notification would do);
    //   4. answer `listGroups`.
    let server = tokio::spawn(async move {
        let (sock, _) = listener.accept().await.unwrap();
        let (read_half, mut write_half) = sock.into_split();
        let mut reader = BufReader::new(read_half);

        let split = FRAME.len() / 2;
        write_half
            .write_all(&FRAME.as_bytes()[..split])
            .await
            .unwrap();
        write_half.flush().await.unwrap();

        // Wait for the intervening RPC request (listGroups) to arrive.
        let mut req = String::new();
        let id = loop {
            req.clear();
            let n = reader.read_line(&mut req).await.unwrap();
            assert_ne!(n, 0, "client closed before issuing the intervening request");
            let trimmed = req.trim();
            if trimmed.is_empty() {
                continue;
            }
            let value: serde_json::Value = serde_json::from_str(trimmed).unwrap();
            let method = value.get("method").and_then(serde_json::Value::as_str);
            assert_eq!(method, Some("listGroups"), "expected the intervening RPC");
            break value.get("id").cloned().unwrap_or(serde_json::Value::Null);
        };

        // Complete the atomic notification frame first (tail + newline)...
        write_half
            .write_all(&FRAME.as_bytes()[split..])
            .await
            .unwrap();
        write_half.write_all(b"\n").await.unwrap();
        write_half.flush().await.unwrap();

        // ...then answer the intervening RPC.
        let result: serde_json::Value = serde_json::from_str(LIST_GROUPS_RESULT).unwrap();
        let resp = serde_json::json!({"jsonrpc":"2.0","id":id,"result":result});
        let mut line = serde_json::to_string(&resp).unwrap();
        line.push('\n');
        write_half.write_all(line.as_bytes()).await.unwrap();
        write_half.flush().await.unwrap();

        // Keep the socket open so the client can read before EOF.
        tokio::time::sleep(Duration::from_millis(300)).await;
    });

    let mut transport = SignalTransport::connect(&addr.to_string()).await.unwrap();

    // Round 1: model the biased select where a competing event wins while
    // `receive()` is suspended mid-frame. The competing arm fires first, so the
    // `receive()` future is dropped after it has consumed only the first
    // segment. This is exactly the cancellation the chat's select can cause.
    let dropped = tokio::select! {
        biased;
        () = tokio::time::sleep(Duration::from_millis(100)) => true,
        _ = transport.receive() => false,
    };
    assert!(
        dropped,
        "test precondition: the competing arm must win so receive() is cancelled mid-frame"
    );

    // Intervening RPC on the SAME connection. Under a cancel-safe design this
    // must not destroy the half-read notification: its remaining bytes are read
    // as part of the same frame and the completed notification is queued, not
    // dropped, while the RPC still returns its own response.
    let members = transport
        .group_members(&GroupId("grp-frag==".to_string()))
        .await
        .expect("intervening group_members RPC must succeed");
    assert!(
        members.contains(&"+15551230001".to_string()),
        "sanity: the intervening RPC returned the expected member set"
    );

    // Round 2: the fragmented notification must now be delivered INTACT.
    let env = tokio::time::timeout(Duration::from_secs(2), transport.receive())
        .await
        .expect("receive() must not hang: the fragmented frame was lost")
        .expect("receive ok")
        .expect("the fragmented inbound envelope must be delivered, never dropped");

    assert_eq!(env.source.as_deref(), Some("+15551230001"));
    assert_eq!(env.group_id.as_deref(), Some("grp-frag=="));
    assert_eq!(
        env.body.as_deref(),
        Some("a large pasted prompt fragmented across TCP segments"),
        "the reassembled body must match the original exactly"
    );

    // Exactly once: a further receive must not re-emit the same frame.
    //
    // Two outcomes both prove that, and the original assertion accepted only
    // one of them. A timeout means nothing more arrived within the window. An
    // EOF — `Ok(Ok(None))` — means nothing more *can* arrive, because the
    // server finished and dropped its socket. EOF is the stronger evidence of
    // the two, and it was being reported as a duplicate.
    //
    // The comment this replaces asserted "the socket is still open", which is
    // exactly the assumption that does not hold: whether the server task has
    // finished by now is a race against a 300 ms timer, so under load the test
    // failed on its own fixture shutting down cleanly (issue #1385).
    //
    // Only an actual second envelope is a duplicate.
    let dup = tokio::time::timeout(Duration::from_millis(300), transport.receive()).await;
    match dup {
        Err(_elapsed) => {}
        Ok(Ok(None)) => {}
        other => panic!(
            "the fragmented frame must be delivered exactly once, but a second \
             receive produced {other:?}"
        ),
    }

    server.await.unwrap();
}
