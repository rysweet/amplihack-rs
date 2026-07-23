//! signal-cli JSON-RPC 2.0 transport: newline-delimited JSON over `tokio` TCP.
//!
//! This module has two layers:
//!
//! - **Pure wire helpers** ([`build_send_request`], [`parse_incoming`]) that do
//!   **no I/O** and are unit-tested in isolation over realistic fixture JSON.
//! - The **`SignalTransport`** client that owns the TCP socket and performs the
//!   `create_group` / `send_group` / `quit_group` / `receive` RPCs.

use serde_json::Value;

/// Maximum size, in bytes, of a single newline-delimited JSON-RPC frame.
///
/// Fail-safe input bound: a peer that never emits a newline (hostile or broken)
/// must not be able to drive unbounded memory growth. Bytes for a single frame
/// are accumulated only up to this cap; a frame that exceeds it is drained (to
/// resynchronize the stream) and skipped. Signal messages are ~2 KiB, so this
/// generous cap never truncates a legitimate frame.
const MAX_FRAME_BYTES: usize = 256 * 1024;

/// Opaque signal-cli group identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupId(pub String);

impl GroupId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for GroupId {
    fn from(s: String) -> Self {
        GroupId(s)
    }
}

/// A parsed inbound envelope, normalized across signal-cli message shapes.
///
/// `parse_incoming` populates this from either a group `dataMessage` (an
/// operator message) or a `syncMessage.sentMessage` (the account's own message
/// synced back from another device). Non-group frames (receipts, typing, direct
/// messages) parse successfully with `group_id == None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Envelope {
    /// E.164 sender number (`sourceNumber` / `source`), if present.
    pub source: Option<String>,
    /// Sending device id (`sourceDevice`), if present.
    pub source_device: Option<u32>,
    /// Group id if this is a group message, else `None`.
    pub group_id: Option<String>,
    /// Message text body, if any.
    pub body: Option<String>,
    /// `true` when derived from a `syncMessage` (the account's own message).
    pub is_sync: bool,
}

impl Envelope {
    /// Whether this envelope carries a group id.
    #[must_use]
    pub fn is_group(&self) -> bool {
        self.group_id.is_some()
    }
}

/// Errors from the pure wire helpers.
#[derive(Debug, thiserror::Error)]
pub enum WireError {
    /// The input line was not valid JSON.
    #[error("invalid JSON frame: {0}")]
    Json(String),
}

/// Build a JSON-RPC 2.0 `send` request frame for an outbound group message.
///
/// Returns the request object (the transport assigns the `id` and appends the
/// trailing newline when writing to the socket). Shape:
///
/// ```json
/// {"jsonrpc":"2.0","method":"send","params":{"groupId":"...","message":"..."}}
/// ```
#[must_use]
pub fn build_send_request(group_id: &str, body: &str) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "send",
        "params": {
            "groupId": group_id,
            "message": body,
        }
    })
}

/// Parse one newline-delimited JSON-RPC line into a normalized [`Envelope`].
///
/// Tolerant / fail-safe: any structurally-valid JSON parses to an `Envelope`
/// (with best-effort field extraction); only non-JSON input returns
/// [`WireError::Json`]. Handles both `dataMessage.groupInfo.groupId` and the
/// `syncMessage.sentMessage` group shape, and accepts the frame either wrapped
/// as `{"method":"receive","params":{"envelope":{...}}}` or as a bare envelope.
pub fn parse_incoming(line: &str) -> Result<Envelope, WireError> {
    let root: Value = serde_json::from_str(line).map_err(|e| WireError::Json(e.to_string()))?;

    // Unwrap `{"params":{"envelope":{...}}}` if present, else treat the value
    // itself as the envelope.
    let env = root
        .get("params")
        .and_then(|p| p.get("envelope"))
        .unwrap_or(&root);

    let source = env
        .get("source")
        .and_then(Value::as_str)
        .or_else(|| env.get("sourceNumber").and_then(Value::as_str))
        .map(str::to_string);
    let source_device = env
        .get("sourceDevice")
        .and_then(Value::as_u64)
        .map(|n| n as u32);

    let group_id_of = |msg: &Value| -> Option<String> {
        msg.get("groupInfo")
            .filter(|g| !g.is_null())
            .and_then(|g| g.get("groupId"))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let body_of = |msg: &Value| -> Option<String> {
        msg.get("message")
            .and_then(Value::as_str)
            .map(str::to_string)
    };

    let (group_id, body, is_sync) =
        if let Some(dm) = env.get("dataMessage").filter(|d| !d.is_null()) {
            (group_id_of(dm), body_of(dm), false)
        } else if let Some(sm) = env.get("syncMessage").filter(|d| !d.is_null()) {
            match sm.get("sentMessage").filter(|d| !d.is_null()) {
                Some(sent) => (group_id_of(sent), body_of(sent), true),
                None => (None, None, true),
            }
        } else {
            (None, None, false)
        };

    Ok(Envelope {
        source,
        source_device,
        group_id,
        body,
        is_sync,
    })
}

/// Newline-delimited JSON-RPC 2.0 client over a `tokio` TCP connection.
///
/// Owns the socket; all methods perform network I/O. The pure helpers above
/// are used internally and are what the unit tests exercise.
pub struct SignalTransport {
    reader: tokio::io::BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: tokio::net::tcp::OwnedWriteHalf,
    next_id: u64,
    /// Reusable line buffer for `read_line`, so the receive hot loop does not
    /// heap-allocate a fresh `String` for every inbound frame.
    line_buf: String,
    /// Reusable raw-byte accumulator for one frame, bounded by
    /// [`MAX_FRAME_BYTES`]; decoded once into `line_buf` per frame.
    raw_buf: Vec<u8>,
}

impl SignalTransport {
    /// Connect to the signal-cli JSON-RPC daemon at `endpoint` (`host:port`).
    pub async fn connect(endpoint: &str) -> std::io::Result<Self> {
        use tokio::io::BufReader;
        use tokio::net::TcpStream;

        let stream = TcpStream::connect(endpoint).await?;
        let (read_half, write_half) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(read_half),
            writer: write_half,
            next_id: 1,
            line_buf: String::new(),
            raw_buf: Vec::new(),
        })
    }

    /// Connect to the signal-cli JSON-RPC daemon with **bounded retry**.
    ///
    /// External-service resilience: the signal-cli daemon is a separate process
    /// and may not be accepting connections the instant amplihack starts (a
    /// startup race), or may briefly refuse while restarting. Rather than let a
    /// single transient `connect` failure silently disable the whole channel,
    /// this retries [`connect`](Self::connect) up to `max_attempts` times using
    /// **capped exponential backoff**: the delay starts at `base_delay`, doubles
    /// after each failed attempt, and is clamped to `max_delay`.
    ///
    /// The first successful connection short-circuits and is returned
    /// immediately. If every attempt fails, the **last** underlying I/O error is
    /// returned (so the caller sees the real cause, e.g. `ConnectionRefused`).
    /// `max_attempts` is treated as at least `1`, so at least one connect is
    /// always attempted and no backoff sleep occurs on the final attempt.
    ///
    /// This is additive: [`connect`](Self::connect) keeps its exact
    /// single-attempt semantics for callers that want fail-fast behavior.
    pub async fn connect_with_retry(
        endpoint: &str,
        max_attempts: u32,
        base_delay: std::time::Duration,
        max_delay: std::time::Duration,
    ) -> std::io::Result<Self> {
        let attempts = max_attempts.max(1);
        let mut delay = base_delay;
        let mut last_err: Option<std::io::Error> = None;

        for attempt in 1..=attempts {
            match Self::connect(endpoint).await {
                Ok(transport) => return Ok(transport),
                Err(e) => {
                    // Only sleep/back off when another attempt remains; never
                    // pause after the final attempt.
                    if attempt < attempts {
                        tracing::warn!(
                            attempt,
                            max_attempts = attempts,
                            error = %e,
                            "signal transport connect failed; retrying after backoff"
                        );
                        tokio::time::sleep(delay).await;
                        delay = delay.saturating_mul(2).min(max_delay);
                    }
                    last_err = Some(e);
                }
            }
        }

        // `attempts >= 1` guarantees at least one failed attempt populated
        // `last_err` before we reach here; the fallback is defensive only.
        Err(last_err.unwrap_or_else(|| {
            std::io::Error::other("connect_with_retry: no connection attempts were made")
        }))
    }

    /// Write one JSON-RPC request frame (newline-terminated) to the socket.
    async fn write_frame(&mut self, frame: &Value) -> std::io::Result<()> {
        use tokio::io::AsyncWriteExt;
        let mut line = serde_json::to_string(frame)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        line.push('\n');
        self.writer.write_all(line.as_bytes()).await?;
        self.writer.flush().await
    }

    /// Read one newline-delimited line from the socket (`None` on EOF).
    ///
    /// Reads into a reusable internal buffer and returns a borrow of it, so the
    /// receive loop avoids a per-frame allocation. The returned slice is valid
    /// until the next `read_line` call.
    ///
    /// The read is **bounded** by [`MAX_FRAME_BYTES`]: a frame larger than the
    /// cap is drained to the next newline (to resynchronize the stream) and
    /// reported as an empty line, which callers skip. This prevents a peer that
    /// never sends a newline from driving unbounded memory growth.
    async fn read_line(&mut self) -> std::io::Result<Option<&str>> {
        use tokio::io::AsyncBufReadExt;
        self.line_buf.clear();
        self.raw_buf.clear();

        let mut read_any = false;
        let mut oversized = false;
        loop {
            let available = self.reader.fill_buf().await?;
            if available.is_empty() {
                break; // EOF
            }
            read_any = true;

            let (consumed, done) = match available.iter().position(|&b| b == b'\n') {
                Some(pos) => {
                    let end = pos + 1;
                    if !oversized && self.raw_buf.len() + end <= MAX_FRAME_BYTES {
                        self.raw_buf.extend_from_slice(&available[..end]);
                    } else {
                        oversized = true;
                    }
                    (end, true)
                }
                None => {
                    let len = available.len();
                    if !oversized && self.raw_buf.len() + len <= MAX_FRAME_BYTES {
                        self.raw_buf.extend_from_slice(available);
                    } else {
                        oversized = true;
                    }
                    (len, false)
                }
            };
            self.reader.consume(consumed);
            if done {
                break;
            }
        }

        if !read_any {
            return Ok(None);
        }
        if oversized {
            // Frame exceeded the cap; the stream has been drained to the next
            // newline. Report an empty line so callers skip it (fail-safe).
            return Ok(Some(""));
        }
        // Lossy UTF-8 decode directly into `line_buf`, avoiding the
        // intermediate owned String that `String::from_utf8_lossy` allocates on
        // the invalid-byte path. Semantics are identical: one U+FFFD per
        // maximal invalid subsequence (this is exactly how `from_utf8_lossy` is
        // implemented internally).
        for chunk in self.raw_buf.utf8_chunks() {
            self.line_buf.push_str(chunk.valid());
            if !chunk.invalid().is_empty() {
                self.line_buf.push('\u{FFFD}');
            }
        }
        Ok(Some(self.line_buf.as_str()))
    }

    /// Send a request and read frames until the matching `id` response arrives,
    /// returning its `result` value. Interleaved `receive` notifications are
    /// skipped.
    async fn request(&mut self, method: &str, params: Value) -> std::io::Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.write_frame(&frame).await?;

        loop {
            let Some(line) = self.read_line().await? else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "connection closed before response",
                ));
            };
            let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(err) = value.get("error").filter(|e| !e.is_null()) {
                    return Err(std::io::Error::other(format!("JSON-RPC error: {err}")));
                }
                return Ok(value.get("result").cloned().unwrap_or(Value::Null));
            }
        }
    }

    /// Create a group by name (wraps the signal-cli `updateGroup` create-by-name
    /// RPC) and return its [`GroupId`].
    pub async fn create_group(&mut self, name: &str) -> std::io::Result<GroupId> {
        let result = self
            .request("updateGroup", serde_json::json!({ "name": name }))
            .await?;
        // signal-cli returns the new/updated group id under `groupId`.
        let gid = result
            .get("groupId")
            .and_then(Value::as_str)
            .or_else(|| {
                result
                    .get("results")
                    .and_then(|r| r.get("groupId"))
                    .and_then(Value::as_str)
            })
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "updateGroup response missing groupId",
                )
            })?;
        Ok(GroupId(gid.to_string()))
    }

    /// Post `body` to `group_id` (wraps the `send` RPC).
    pub async fn send_group(&mut self, group_id: &GroupId, body: &str) -> std::io::Result<()> {
        let params = build_send_request(group_id.as_str(), body)
            .get("params")
            .cloned()
            .unwrap_or(Value::Null);
        self.request("send", params).await.map(|_| ())
    }

    /// Leave / close a group (`quitGroup`).
    pub async fn quit_group(&mut self, group_id: &GroupId) -> std::io::Result<()> {
        self.request(
            "quitGroup",
            serde_json::json!({ "groupId": group_id.as_str() }),
        )
        .await
        .map(|_| ())
    }

    /// Read and parse the next inbound envelope from the receive stream.
    ///
    /// Returns `Ok(None)` at end-of-stream. Lines that are not valid JSON are
    /// skipped (fail-safe) rather than aborting the stream.
    pub async fn receive(&mut self) -> std::io::Result<Option<Envelope>> {
        loop {
            let Some(line) = self.read_line().await? else {
                return Ok(None);
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match parse_incoming(trimmed) {
                Ok(env) => return Ok(Some(env)),
                Err(_) => continue,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_send_request_is_jsonrpc_send() {
        let frame = build_send_request("grp-abc123==", "hello world");
        assert_eq!(frame["jsonrpc"], "2.0");
        assert_eq!(frame["method"], "send");
        assert_eq!(frame["params"]["groupId"], "grp-abc123==");
        assert_eq!(frame["params"]["message"], "hello world");
    }

    #[test]
    fn parse_incoming_rejects_non_json() {
        let err = parse_incoming("<not json>").unwrap_err();
        assert!(matches!(err, WireError::Json(_)));
    }

    #[test]
    fn parse_incoming_data_message_group() {
        let line = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{
            "source":"+15551230001","sourceNumber":"+15551230001","sourceDevice":1,
            "dataMessage":{"message":"do the thing","groupInfo":{"groupId":"grp-abc123=="}}
        },"account":"+15551230000"}}"#;
        let env = parse_incoming(line).expect("parses");
        assert_eq!(env.source.as_deref(), Some("+15551230001"));
        assert_eq!(env.source_device, Some(1));
        assert_eq!(env.group_id.as_deref(), Some("grp-abc123=="));
        assert_eq!(env.body.as_deref(), Some("do the thing"));
        assert!(!env.is_sync);
        assert!(env.is_group());
    }

    #[test]
    fn parse_incoming_sync_message_group_marks_is_sync() {
        let line = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{
            "source":"+15551230000","sourceDevice":1,
            "syncMessage":{"sentMessage":{"message":"session started",
                "groupInfo":{"groupId":"grp-abc123=="}}}
        },"account":"+15551230000"}}"#;
        let env = parse_incoming(line).expect("parses");
        assert_eq!(env.group_id.as_deref(), Some("grp-abc123=="));
        assert_eq!(env.body.as_deref(), Some("session started"));
        assert!(env.is_sync, "syncMessage must set is_sync=true");
    }

    #[test]
    fn parse_incoming_direct_message_has_no_group() {
        let line = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{
            "source":"+15551230001","sourceDevice":1,
            "dataMessage":{"message":"hi","groupInfo":null}
        },"account":"+15551230000"}}"#;
        let env = parse_incoming(line).expect("parses");
        assert_eq!(env.group_id, None);
        assert!(!env.is_group());
    }

    #[test]
    fn parse_incoming_receipt_has_no_group_no_body() {
        let line = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{
            "source":"+15551230001","sourceDevice":1,
            "receiptMessage":{"when":123,"isDelivery":true,"timestamps":[1]}
        },"account":"+15551230000"}}"#;
        let env = parse_incoming(line).expect("parses");
        assert_eq!(env.group_id, None);
        assert_eq!(env.body, None);
    }
}
