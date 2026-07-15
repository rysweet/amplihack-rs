//! signal-cli JSON-RPC 2.0 wire format: pure request builders and an inbound
//! envelope parser (no I/O), plus the gated async TCP client.
//!
//! signal-cli speaks newline-delimited JSON-RPC 2.0. Only the pure helpers are
//! compiled on default builds and unit-tested with no sockets, exactly like the
//! Simard reference `transport.rs`.

use serde_json::Value;

/// Default maximum inbound frame size (1 MiB). Mirrors [`crate::config`].
pub const DEFAULT_MAX_FRAME_BYTES: usize = 1024 * 1024;

/// A parsed, relevant inbound group message. Only messages carrying a group id
/// and body are surfaced; everything else parses to `Ok(None)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingMessage {
    /// Sender E.164 (from `envelope.source` / `sourceNumber`).
    pub source_number: String,
    /// Sending device id. Operator's primary phone is device 1.
    pub source_device: u32,
    /// Group id the message was sent to, if any.
    pub group_id: Option<String>,
    /// The message body text.
    pub body: String,
}

/// Inbound parse failure. A *malformed* frame is an error; an *irrelevant* but
/// well-formed frame is `Ok(None)` — the two are deliberately distinct so the
/// receive loop can log-and-continue on noise but surface real corruption.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    /// The frame exceeded the configured maximum size and was not buffered.
    #[error("inbound frame too large: {size} > {max} bytes")]
    FrameTooLarge {
        /// Observed size.
        size: usize,
        /// Configured maximum.
        max: usize,
    },
    /// The frame was not valid JSON.
    #[error("malformed inbound JSON: {0}")]
    Json(String),
}

/// Build a `send` request that posts `body` to `group_id`.
///
/// Shape: `{"jsonrpc":"2.0","id":<id>,"method":"send","params":{"groupId":..,"message":..}}`.
pub fn build_send_request(_group_id: &str, _body: &str, _id: u64) -> Value {
    todo!("build send request (P2)")
}

/// Build an `updateGroup` request that creates a group named `name` containing
/// exactly `members` (for the self-only group, the single operator account).
///
/// Shape: `{"jsonrpc":"2.0","id":<id>,"method":"updateGroup","params":{"name":..,"members":[..]}}`.
pub fn build_create_group_request(_name: &str, _members: &[String], _id: u64) -> Value {
    todo!("build updateGroup request (P2)")
}

/// Build a `quitGroup` request that leaves `group_id`.
pub fn build_quit_group_request(_group_id: &str, _id: u64) -> Value {
    todo!("build quitGroup request (P2)")
}

/// Parse one NDJSON line using the default max frame size.
pub fn parse_incoming(line: &str) -> Result<Option<IncomingMessage>, ParseError> {
    parse_incoming_bounded(line, DEFAULT_MAX_FRAME_BYTES)
}

/// Parse one NDJSON line, rejecting frames larger than `max_frame_bytes`.
///
/// Tolerant by contract: unknown/irrelevant well-formed envelopes return
/// `Ok(None)`; only oversized or non-JSON input returns `Err`. Never panics.
/// Handles both inbound group shapes:
/// - `params.envelope.dataMessage.groupInfo.groupId`
/// - `params.envelope.syncMessage.sentMessage.groupInfo.groupId`
pub fn parse_incoming_bounded(
    _line: &str,
    _max_frame_bytes: usize,
) -> Result<Option<IncomingMessage>, ParseError> {
    todo!("parse inbound envelope, dual-shape, tolerant (P2)")
}

#[cfg(test)]
mod tests {
    use super::*;

    const F_DATA: &str = include_str!("../tests/fixtures/inbound_data_message.json");
    const F_SYNC: &str = include_str!("../tests/fixtures/inbound_sync_sent_message.json");
    const F_RECEIPT: &str = include_str!("../tests/fixtures/inbound_irrelevant_receipt.json");
    const F_BOT: &str = include_str!("../tests/fixtures/inbound_sync_from_bot_device.json");
    const F_MALFORMED: &str = include_str!("../tests/fixtures/inbound_malformed.json");

    #[test]
    fn send_request_has_expected_shape() {
        let req = build_send_request("GID==", "hello", 7);
        assert_eq!(req["jsonrpc"], "2.0");
        assert_eq!(req["id"], 7);
        assert_eq!(req["method"], "send");
        assert_eq!(req["params"]["groupId"], "GID==");
        assert_eq!(req["params"]["message"], "hello");
    }

    #[test]
    fn create_group_request_uses_update_group_method() {
        let req = build_create_group_request("amplihack-sess-1", &["+15551230000".into()], 1);
        assert_eq!(req["jsonrpc"], "2.0");
        assert_eq!(req["id"], 1);
        assert_eq!(req["method"], "updateGroup");
        assert_eq!(req["params"]["name"], "amplihack-sess-1");
        assert_eq!(req["params"]["members"][0], "+15551230000");
    }

    #[test]
    fn quit_group_request_uses_quit_group_method() {
        let req = build_quit_group_request("GID==", 5);
        assert_eq!(req["method"], "quitGroup");
        assert_eq!(req["params"]["groupId"], "GID==");
        assert_eq!(req["id"], 5);
    }

    #[test]
    fn parses_data_message_shape() {
        let msg = parse_incoming(F_DATA).unwrap().expect("relevant message");
        assert_eq!(msg.source_number, "+15551239999");
        assert_eq!(msg.source_device, 1);
        assert_eq!(msg.group_id.as_deref(), Some("SESSION_GROUP_ID_AAA=="));
        assert_eq!(msg.body, "deploy the staging branch");
    }

    #[test]
    fn parses_sync_sent_message_shape() {
        let msg = parse_incoming(F_SYNC).unwrap().expect("relevant message");
        assert_eq!(msg.source_number, "+15551239999");
        assert_eq!(msg.source_device, 1);
        assert_eq!(msg.group_id.as_deref(), Some("SESSION_GROUP_ID_AAA=="));
        assert_eq!(msg.body, "run the tests again");
    }

    #[test]
    fn sync_from_bot_device_still_parses_with_its_device_id() {
        // Parsing must faithfully report sourceDevice >= 2; the *gate* (not the
        // parser) is responsible for rejecting the bot's own synced-back echo.
        let msg = parse_incoming(F_BOT).unwrap().expect("relevant message");
        assert_eq!(msg.source_device, 2);
        assert_eq!(msg.body, "session started for task #903");
    }

    #[test]
    fn irrelevant_envelope_returns_none() {
        assert_eq!(parse_incoming(F_RECEIPT).unwrap(), None);
    }

    #[test]
    fn non_group_data_message_returns_none() {
        // A direct (non-group) message has no groupInfo — not for us.
        let line = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"+15551239999","sourceDevice":1,"dataMessage":{"message":"hi"}}}}"#;
        assert_eq!(parse_incoming(line).unwrap(), None);
    }

    #[test]
    fn malformed_json_is_error() {
        let err = parse_incoming(F_MALFORMED).unwrap_err();
        assert!(matches!(err, ParseError::Json(_)), "got {err:?}");
    }

    #[test]
    fn oversized_frame_is_rejected_without_buffering() {
        let big = "x".repeat(64);
        let err = parse_incoming_bounded(&big, 16).unwrap_err();
        assert!(
            matches!(err, ParseError::FrameTooLarge { size: 64, max: 16 }),
            "got {err:?}"
        );
    }

    #[test]
    fn empty_line_is_none() {
        assert_eq!(parse_incoming("").unwrap(), None);
    }
}
