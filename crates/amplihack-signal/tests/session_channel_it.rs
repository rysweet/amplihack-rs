//! Integration test: `SignalSession` against a mock signal-cli JSON-RPC daemon.
//!
//! Only compiled with `--features signal`; otherwise this file is empty.
#![cfg(feature = "signal")]

use std::time::Duration;

use amplihack_signal::config::{GroupMode, SignalConfig};
use amplihack_signal::session_channel::SignalSession;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// Minimal NDJSON JSON-RPC mock. Replies to `updateGroup`/`send`/`quitGroup`.
/// Returns the bound `host:port` and a join handle.
async fn spawn_mock() -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        handle_conn(stream).await
    });
    (format!("{}:{}", addr.ip(), addr.port()), handle)
}

/// Reads NDJSON requests until EOF, replies to each, and records the methods.
async fn handle_conn(stream: TcpStream) -> Vec<String> {
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();
    let mut seen_methods = Vec::new();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let req: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let id = req.get("id").cloned().unwrap_or(serde_json::Value::Null);
        let method = req
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();
        let result = match method.as_str() {
            "updateGroup" => serde_json::json!({ "groupId": "MOCK_GROUP_ID==" }),
            _ => serde_json::json!({ "timestamp": 1720000000000i64 }),
        };
        let resp = serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result });
        let mut bytes = serde_json::to_vec(&resp).unwrap();
        bytes.push(b'\n');
        write_half.write_all(&bytes).await.unwrap();
        write_half.flush().await.unwrap();
        seen_methods.push(method);
    }
    seen_methods
}

fn config_for(endpoint: &str) -> SignalConfig {
    SignalConfig {
        endpoint: endpoint.to_string(),
        account: "+15551230000".into(),
        allowlist: vec!["+15551239999".into()],
        own_device_id: None,
        echo_ttl: Duration::from_secs(30),
        group_mode: GroupMode::PerSession,
        rolling_group_id: None,
        max_frame_bytes: 1024 * 1024,
    }
}

#[tokio::test]
async fn announce_post_quit_roundtrip() {
    let (endpoint, server) = spawn_mock().await;
    let mut session = SignalSession::connect(config_for(&endpoint))
        .await
        .expect("connect to mock daemon");

    let group_id = session
        .announce("amplihack-session-test")
        .await
        .expect("announce returns a group id");
    assert_eq!(group_id, "MOCK_GROUP_ID==");

    session
        .post("session started for task #903")
        .await
        .expect("post to group");

    session.quit().await.expect("quit group");

    drop(session);
    let methods = server.await.unwrap();
    assert!(methods.contains(&"updateGroup".to_string()));
    assert!(methods.contains(&"send".to_string()));
    assert!(methods.contains(&"quitGroup".to_string()));
}
