//! Integration test for the bounded frame reader (DoS hardening).
//!
//! A single frame larger than `MAX_FRAME_BYTES` (256 KiB) must be drained and
//! skipped rather than buffered whole, and the stream must resynchronize so a
//! subsequent valid frame still parses. This locks in the fix for the
//! unbounded-`read_line` memory-exhaustion vector.
#![cfg(feature = "signal")]

use amplihack_signal::transport::SignalTransport;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

#[tokio::test]
async fn oversized_frame_is_skipped_and_stream_resyncs() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    // Server: send one oversized (>256 KiB, no early newline) frame, then a
    // valid group data message.
    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        // 300 KiB of non-newline bytes, then a newline: exceeds the cap.
        let oversized = vec![b'x'; 300 * 1024];
        sock.write_all(&oversized).await.unwrap();
        sock.write_all(b"\n").await.unwrap();

        let valid = br#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"+15551230001","sourceDevice":1,"dataMessage":{"message":"hello","groupInfo":{"groupId":"grp-abc123=="}}}}}"#;
        sock.write_all(valid).await.unwrap();
        sock.write_all(b"\n").await.unwrap();
        sock.flush().await.unwrap();
        // Keep the socket open briefly so the client can read before EOF.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    });

    let mut transport = SignalTransport::connect(&addr.to_string()).await.unwrap();

    // The oversized frame is skipped; the next real envelope is returned.
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
