//! Integration tests for the bounded connect-retry resilience path.
//!
//! `SignalTransport::connect_with_retry` must (a) return the last underlying
//! I/O error after exhausting all attempts against a dead endpoint, and (b)
//! recover when the signal-cli daemon becomes reachable a moment after the
//! first attempt (the realistic startup-race case). This locks in the
//! external-service resilience contract for the transport client.
#![cfg(feature = "signal")]

use std::time::Duration;

use amplihack_signal::transport::SignalTransport;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

/// Bind an ephemeral port, then immediately release it so subsequent connects
/// are refused. Returns the (now-free) address string.
async fn reserved_dead_addr() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener); // free the port: connects now refuse
    addr.to_string()
}

#[tokio::test]
async fn connect_with_retry_exhausts_and_returns_last_error() {
    let addr = reserved_dead_addr().await;

    // Fast, deterministic: 3 attempts, tiny backoff, no server ever comes up.
    let result = SignalTransport::connect_with_retry(
        &addr,
        3,
        Duration::from_millis(1),
        Duration::from_millis(5),
    )
    .await;

    // The surfaced error is the real connect failure, not a synthetic wrapper.
    let Err(err) = result else {
        panic!("no server: must exhaust and error");
    };
    assert_eq!(
        err.kind(),
        std::io::ErrorKind::ConnectionRefused,
        "expected the underlying ConnectionRefused, got {err:?}"
    );
}

#[tokio::test]
async fn connect_with_retry_recovers_after_transient_failure() {
    // Reserve an address, free it (first attempt will refuse), then bind a
    // real server on the SAME address shortly after so a retry succeeds.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let addr_for_server = addr;
    let server = tokio::spawn(async move {
        // Delay so the client's first connect attempt refuses before the
        // server is listening again.
        tokio::time::sleep(Duration::from_millis(40)).await;
        let l = loop {
            match TcpListener::bind(addr_for_server).await {
                Ok(l) => break l,
                // Port may momentarily linger; keep trying briefly.
                Err(_) => tokio::time::sleep(Duration::from_millis(5)).await,
            }
        };
        let (mut sock, _) = l.accept().await.unwrap();
        let valid = br#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"+15551230001","sourceDevice":1,"dataMessage":{"message":"hello","groupInfo":{"groupId":"grp-abc123=="}}}}}"#;
        sock.write_all(valid).await.unwrap();
        sock.write_all(b"\n").await.unwrap();
        sock.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
    });

    // Generous attempt budget with short backoff comfortably covers the ~40ms
    // server startup delay without being flaky.
    let mut transport = SignalTransport::connect_with_retry(
        &addr.to_string(),
        50,
        Duration::from_millis(10),
        Duration::from_millis(30),
    )
    .await
    .expect("must connect once the server comes up");

    // Sanity: the recovered connection is fully usable.
    let env = transport
        .receive()
        .await
        .expect("receive ok")
        .expect("an envelope");
    assert_eq!(env.source.as_deref(), Some("+15551230001"));
    assert_eq!(env.group_id.as_deref(), Some("grp-abc123=="));
    assert_eq!(env.body.as_deref(), Some("hello"));

    server.await.unwrap();
}

#[tokio::test]
async fn connect_with_retry_zero_attempts_still_tries_once() {
    let addr = reserved_dead_addr().await;

    // `max_attempts == 0` is normalized to a single attempt (no backoff sleep).
    let result = SignalTransport::connect_with_retry(
        &addr,
        0,
        Duration::from_millis(1),
        Duration::from_millis(1),
    )
    .await;
    let Err(err) = result else {
        panic!("dead endpoint must error");
    };
    assert_eq!(err.kind(), std::io::ErrorKind::ConnectionRefused);
}
