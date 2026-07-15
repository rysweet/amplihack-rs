//! Integration test for the `amplihack-hooks signal-subscriber` subcommand.
//!
//! Spawns the real multicall binary (resolved via `CARGO_BIN_EXE_amplihack-hooks`)
//! pointed at a mock signal-cli JSON-RPC daemon, and asserts that a gate-accepted
//! operator message (device 1, allowlisted sender, matching group id) is appended
//! to the per-session file inbox. Only compiled with `--features signal`.
#![cfg(feature = "signal")]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use amplihack_signal::session_channel::Inbox;

const OPERATOR: &str = "+15551239999";
const ACCOUNT: &str = "+15551230000";
const GROUP_ID: &str = "SESSION_GROUP_ID_AAA==";

/// A blocking, single-connection mock signal-cli daemon.
///
/// On connect it immediately pushes one inbound group message (device 1), then
/// replies `{}` to any client request until the client disconnects. Returns the
/// bound `host:port`.
fn spawn_mock_daemon() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            let mut writer = stream.try_clone().unwrap();
            // Push a realistic inbound group message from the operator's phone.
            let notification = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "receive",
                "params": {
                    "envelope": {
                        "source": OPERATOR,
                        "sourceNumber": OPERATOR,
                        "sourceDevice": 1,
                        "timestamp": 1720000000000i64,
                        "dataMessage": {
                            "message": "deploy the staging branch",
                            "groupInfo": { "groupId": GROUP_ID }
                        }
                    }
                }
            });
            let mut line = serde_json::to_vec(&notification).unwrap();
            line.push(b'\n');
            let _ = writer.write_all(&line);
            let _ = writer.flush();

            // Drain and ack any requests the subscriber sends (e.g. subscribe).
            let reader = BufReader::new(stream);
            for req_line in reader.lines() {
                let Ok(req_line) = req_line else { break };
                if req_line.trim().is_empty() {
                    continue;
                }
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&req_line) {
                    let id = v.get("id").cloned().unwrap_or(serde_json::Value::Null);
                    let resp = serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": {} });
                    let mut bytes = serde_json::to_vec(&resp).unwrap();
                    bytes.push(b'\n');
                    let _ = writer.write_all(&bytes);
                    let _ = writer.flush();
                }
            }
        }
    });
    format!("{}:{}", addr.ip(), addr.port())
}

#[test]
fn accepted_operator_message_lands_in_inbox() {
    let endpoint = spawn_mock_daemon();
    let tmp = tempfile::tempdir().unwrap();
    let inbox_path = tmp.path().join("inbox.json");

    let mut child = Command::new(env!("CARGO_BIN_EXE_amplihack-hooks"))
        .arg("signal-subscriber")
        .arg("--session-id")
        .arg("it-session")
        .arg("--group-id")
        .arg(GROUP_ID)
        .arg("--inbox")
        .arg(&inbox_path)
        .env("AMPLIHACK_SIGNAL_ENDPOINT", &endpoint)
        .env("AMPLIHACK_SIGNAL_ACCOUNT", ACCOUNT)
        .env("AMPLIHACK_SIGNAL_ALLOWLIST", OPERATOR)
        .spawn()
        .expect("spawn signal-subscriber subcommand");

    // Poll the inbox until the message is delivered or we time out.
    let inbox = Inbox::new(&inbox_path);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut delivered = Vec::new();
    while Instant::now() < deadline {
        if let Ok(entries) = inbox.peek() {
            if !entries.is_empty() {
                delivered = entries;
                break;
            }
        }
        thread::sleep(Duration::from_millis(100));
    }

    let _ = child.kill();
    let _ = child.wait();

    assert_eq!(delivered.len(), 1, "exactly one accepted message expected");
    assert_eq!(delivered[0].source, OPERATOR);
    assert_eq!(delivered[0].body, "deploy the staging branch");
}
