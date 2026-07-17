//! Fixture-driven envelope-shape tests over realistic signal-cli JSON.
//!
//! These are **pure wire tests**: no network, no filesystem beyond loading the
//! static fixture strings. They pin the contract of
//! [`amplihack_signal::transport::parse_incoming`] across both group envelope
//! shapes (`dataMessage.groupInfo.groupId` and the `syncMessage.sentMessage`
//! shape) as well as non-group frames.
#![cfg(feature = "signal")]

use amplihack_signal::transport::parse_incoming;

const GID: &str = "grp-abc123==";

const DATA_GROUP: &str = include_str!("fixtures/data_message_group.json");
const SYNC_GROUP: &str = include_str!("fixtures/sync_message_group.json");
const LINKED_DEVICE: &str = include_str!("fixtures/linked_device_group.json");
const DIRECT: &str = include_str!("fixtures/direct_message.json");
const RECEIPT: &str = include_str!("fixtures/receipt_message.json");

#[test]
fn data_message_group_envelope() {
    let env = parse_incoming(DATA_GROUP).expect("parses");
    assert_eq!(env.source.as_deref(), Some("+15551230001"));
    assert_eq!(env.source_device, Some(1));
    assert_eq!(env.group_id.as_deref(), Some(GID));
    assert_eq!(env.body.as_deref(), Some("focus on the failing test first"));
    assert!(!env.is_sync);
    assert!(env.is_group());
}

#[test]
fn sync_message_group_envelope_is_marked_sync() {
    let env = parse_incoming(SYNC_GROUP).expect("parses");
    assert_eq!(env.group_id.as_deref(), Some(GID));
    assert_eq!(env.body.as_deref(), Some("session started"));
    assert!(env.is_sync, "syncMessage envelopes must set is_sync=true");
}

#[test]
fn linked_device_group_envelope_preserves_device_id() {
    // Parsing does not gate; it just surfaces the device id (2 = linked). The
    // gate is responsible for rejecting non-primary devices.
    let env = parse_incoming(LINKED_DEVICE).expect("parses");
    assert_eq!(env.group_id.as_deref(), Some(GID));
    assert_eq!(env.source_device, Some(2));
}

#[test]
fn direct_message_has_no_group() {
    let env = parse_incoming(DIRECT).expect("parses");
    assert_eq!(env.group_id, None);
    assert!(!env.is_group());
}

#[test]
fn receipt_message_has_no_group_and_no_body() {
    let env = parse_incoming(RECEIPT).expect("parses");
    assert_eq!(env.group_id, None);
    assert_eq!(env.body, None);
}

#[test]
fn non_json_line_is_a_wire_error() {
    assert!(parse_incoming("definitely not json").is_err());
}

#[test]
fn empty_line_is_a_wire_error() {
    assert!(parse_incoming("").is_err());
}
