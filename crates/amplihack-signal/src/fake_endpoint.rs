//! A loopback, in-process fake of the signal-cli JSON-RPC daemon for
//! **hermetic offline testing**.
//!
//! The real Signal channel talks newline-delimited JSON-RPC 2.0 to signal-cli's
//! `daemon --tcp` endpoint. To exercise the real [`crate::transport::SignalTransport`]
//! (and, above it, [`crate::session_channel::SignalSession`]) end-to-end in CI
//! **without ever touching the Signal network or creating a real group**, this
//! type stands up a `127.0.0.1:0` (ephemeral loopback) TCP server that speaks
//! the same wire protocol and records what the transport sent.
//!
//! Guarantees that make it safe for CI:
//! * binds **loopback only** (`127.0.0.1`), so no test can reach an external
//!   host;
//! * never performs any real Signal operation — `updateGroup` returns a
//!   configurable fake group id, `send`/`quitGroup` just record and ack;
//! * inbound envelopes are **explicitly enqueued** by the test (nothing arrives
//!   unless a test asks for it).
//!
//! It supports both the "enqueue before connect" and "enqueue after connect"
//! orderings: pre-connection inbound lines are buffered and flushed as soon as a
//! client connects.

use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// What the fake recorded from the transport, for test assertions.
#[derive(Default)]
struct Recorded {
    /// `updateGroup` names, in order.
    created: Vec<String>,
    /// `(groupId, body)` pairs from `send`, in order.
    sent: Vec<(String, String)>,
    /// `quitGroup` group ids, in order.
    quit: Vec<String>,
}

/// Shared state between the public handle and the background server tasks.
struct Shared {
    recorded: Mutex<Recorded>,
    /// Group id returned by `updateGroup` (settable via [`FakeSignalEndpoint::with_group_id`]).
    group_id: Mutex<String>,
    /// Live sender to the connected client's writer task (if any).
    live_tx: Mutex<Option<mpsc::UnboundedSender<String>>>,
    /// Inbound lines enqueued before a client connected.
    pending: Mutex<Vec<String>>,
}

/// A loopback fake of the signal-cli JSON-RPC daemon.
pub struct FakeSignalEndpoint {
    addr: String,
    shared: Arc<Shared>,
}

impl FakeSignalEndpoint {
    /// Bind an ephemeral loopback port and start accepting connections.
    pub async fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?.to_string();
        let shared = Arc::new(Shared {
            recorded: Mutex::new(Recorded::default()),
            group_id: Mutex::new("grp-fake==".to_string()),
            live_tx: Mutex::new(None),
            pending: Mutex::new(Vec::new()),
        });

        let accept_shared = shared.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(handle_conn(stream, accept_shared.clone()));
            }
        });

        Ok(Self { addr, shared })
    }

    /// Configure the group id returned by `updateGroup` (builder style).
    #[must_use]
    pub fn with_group_id(self, id: &str) -> Self {
        *self.shared.group_id.lock().unwrap() = id.to_string();
        self
    }

    /// The `host:port` the fake is listening on (always `127.0.0.1:<port>`).
    #[must_use]
    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// Enqueue one inbound JSON-RPC `receive` frame to be delivered to the
    /// connected client. Accepts pretty-printed JSON (it is normalized to a
    /// single newline-delimited frame). Delivered immediately if a client is
    /// connected, otherwise buffered until one connects.
    pub fn enqueue_inbound(&self, raw: &str) {
        // The wire protocol is newline-delimited, so collapse any embedded
        // newlines by round-tripping through serde_json into a compact line.
        let compact = match serde_json::from_str::<Value>(raw) {
            Ok(v) => serde_json::to_string(&v).unwrap_or_else(|_| raw.replace('\n', " ")),
            Err(_) => raw.replace('\n', " "),
        };
        let line = format!("{compact}\n");

        let live = self.shared.live_tx.lock().unwrap();
        if let Some(tx) = live.as_ref() {
            let _ = tx.send(line);
        } else {
            drop(live);
            self.shared.pending.lock().unwrap().push(line);
        }
    }

    /// Group names recorded from `updateGroup`.
    #[must_use]
    pub fn created_groups(&self) -> Vec<String> {
        self.shared.recorded.lock().unwrap().created.clone()
    }

    /// `(groupId, body)` pairs recorded from `send`.
    #[must_use]
    pub fn sent(&self) -> Vec<(String, String)> {
        self.shared.recorded.lock().unwrap().sent.clone()
    }

    /// Group ids recorded from `quitGroup`.
    #[must_use]
    pub fn quit_groups(&self) -> Vec<String> {
        self.shared.recorded.lock().unwrap().quit.clone()
    }
}

/// Serve a single client connection: answer its JSON-RPC requests and push any
/// enqueued inbound frames.
async fn handle_conn(stream: tokio::net::TcpStream, shared: Arc<Shared>) {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    // Register this connection as the live sink and flush anything enqueued
    // before the client connected.
    {
        let pending: Vec<String> = std::mem::take(&mut *shared.pending.lock().unwrap());
        for line in pending {
            let _ = tx.send(line);
        }
        *shared.live_tx.lock().unwrap() = Some(tx.clone());
    }

    // Writer task: serialize all outbound frames (responses + pushed inbound)
    // through a single owner of the write half.
    let writer = tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            if write_half.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            let _ = write_half.flush().await;
        }
    });

    let mut line = String::new();
    loop {
        line.clear();
        let n = match reader.read_line(&mut line).await {
            Ok(n) => n,
            Err(_) => break,
        };
        if n == 0 {
            break; // EOF
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        let method = req.get("method").and_then(Value::as_str).unwrap_or("");
        let params = req.get("params").cloned().unwrap_or(Value::Null);

        let result = match method {
            "updateGroup" => {
                if let Some(name) = params.get("name").and_then(Value::as_str) {
                    shared
                        .recorded
                        .lock()
                        .unwrap()
                        .created
                        .push(name.to_string());
                }
                let gid = shared.group_id.lock().unwrap().clone();
                serde_json::json!({ "groupId": gid })
            }
            "send" => {
                let g = params
                    .get("groupId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let m = params
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                shared.recorded.lock().unwrap().sent.push((g, m));
                serde_json::json!({ "results": [], "timestamp": 0 })
            }
            "quitGroup" => {
                let g = params
                    .get("groupId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                shared.recorded.lock().unwrap().quit.push(g);
                serde_json::json!({})
            }
            _ => Value::Null,
        };

        // Reply only to requests that carry an id (JSON-RPC calls).
        if let Some(id) = req.get("id").cloned() {
            let resp = serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result });
            let mut s = serde_json::to_string(&resp).unwrap_or_default();
            s.push('\n');
            let _ = tx.send(s);
        }
    }

    // Client disconnected: drop the live sink so the writer task can finish.
    *shared.live_tx.lock().unwrap() = None;
    drop(tx);
    let _ = writer.await;
}
