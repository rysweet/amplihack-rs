//! Integration test for the frame reader's UTF-8 sanitization (S1 mitigation).
//!
//! `read_line` decodes each raw frame with `String::from_utf8_lossy`, which
//! replaces every invalid byte sequence with the Unicode replacement character
//! U+FFFD rather than erroring or panicking. This is a security control: a
//! malicious or corrupt peer cannot crash the receive loop or smuggle
//! un-decodable bytes past parsing.
//!
//! The Step 9b perf refactor removes a redundant intermediate `String` copy at
//! the decode site. This test locks the observable behavior — invalid bytes
//! become U+FFFD and the frame still parses — so the refactor is provably
//! sanitization-preserving.
#![cfg(feature = "signal")]

use amplihack_signal::transport::SignalTransport;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

/// A frame carrying an invalid UTF-8 byte inside the message string must be
/// lossily decoded (byte → U+FFFD) and still parse into an envelope, with the
/// replacement character preserved in the body.
#[tokio::test]
async fn invalid_utf8_in_frame_is_lossily_sanitized() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();

        // Build a valid JSON envelope but splice a lone 0xFF byte into the
        // message value. 0xFF is not valid UTF-8; lossy decoding turns it into
        // U+FFFD, after which the surrounding JSON parses normally.
        let mut frame = Vec::new();
        frame.extend_from_slice(
            br#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"+15551230001","sourceDevice":1,"dataMessage":{"message":"hi"#,
        );
        frame.push(0xFF);
        frame.extend_from_slice(br#"there","groupInfo":{"groupId":"grp-abc123=="}}}}}"#);
        frame.push(b'\n');

        sock.write_all(&frame).await.unwrap();
        sock.flush().await.unwrap();
        // Hold the socket open briefly so the client reads before EOF.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    });

    let mut transport = SignalTransport::connect(&addr.to_string()).await.unwrap();

    let env = transport
        .receive()
        .await
        .expect("receive ok")
        .expect("an envelope");

    assert_eq!(env.source.as_deref(), Some("+15551230001"));
    assert_eq!(env.group_id.as_deref(), Some("grp-abc123=="));
    // The invalid 0xFF byte is sanitized to exactly one U+FFFD; the rest of the
    // body is untouched.
    assert_eq!(env.body.as_deref(), Some("hi\u{FFFD}there"));

    server.await.unwrap();
}

/// After a frame containing invalid UTF-8, the stream must resynchronize on the
/// next newline so a subsequent clean frame parses correctly. This guards the
/// per-frame buffer reset (S4) alongside the lossy-decode path.
#[tokio::test]
async fn stream_resyncs_after_invalid_utf8_frame() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();

        // First frame: contains an invalid byte in the body.
        let mut dirty = Vec::new();
        dirty.extend_from_slice(
            br#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"+15551230001","sourceDevice":1,"dataMessage":{"message":"first"#,
        );
        dirty.push(0xFE);
        dirty.extend_from_slice(br#"","groupInfo":{"groupId":"grp-abc123=="}}}}}"#);
        dirty.push(b'\n');
        sock.write_all(&dirty).await.unwrap();

        // Second frame: entirely clean.
        let clean = br#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"+15551230002","sourceDevice":1,"dataMessage":{"message":"second","groupInfo":{"groupId":"grp-abc123=="}}}}}"#;
        sock.write_all(clean).await.unwrap();
        sock.write_all(b"\n").await.unwrap();
        sock.flush().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    });

    let mut transport = SignalTransport::connect(&addr.to_string()).await.unwrap();

    let first = transport
        .receive()
        .await
        .expect("receive ok")
        .expect("first envelope");
    assert_eq!(first.body.as_deref(), Some("first\u{FFFD}"));

    let second = transport
        .receive()
        .await
        .expect("receive ok")
        .expect("second envelope");
    assert_eq!(second.source.as_deref(), Some("+15551230002"));
    assert_eq!(second.body.as_deref(), Some("second"));

    server.await.unwrap();
}
