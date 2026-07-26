//! R4 — full-conversation mirroring: outbound truncation + echo suppression
//! (issue #1002).
//!
//! Mirroring the whole session (every user prompt + every assistant turn) to the
//! Signal group introduces two hazards these tests pin down:
//!
//! 1. **Unbounded message size.** Assistant turns can be huge; each mirrored
//!    message must be bounded to `RELAY_MAX_BYTES` (4 KiB) at a UTF-8 char
//!    boundary so we never send a partial code point or a giant frame.
//!
//! 2. **Echo loops.** The outbound mirror runs in the hook process while the
//!    inbound subscriber runs in a detached process, so the in-memory
//!    echo-suppression window in `Gate` cannot span processes. A file-shared,
//!    hashed fingerprint of each outbound body lets the subscriber recognize and
//!    drop the account's own mirrored messages instead of re-injecting them.
//!
//! Both seams take an explicit `root`, so tests are hermetic (temp dir, no
//! HOME/cwd mutation).
//!
//! RED: the `outbound` module and its items do not exist yet.
#![cfg(feature = "signal")]

use amplihack_hooks::signal_integration::outbound::{
    RELAY_MAX_BYTES, is_recent_outbound_fingerprint, record_outbound_fingerprint,
    truncate_for_relay,
};

#[test]
fn relay_max_is_four_kib() {
    assert_eq!(RELAY_MAX_BYTES, 4096);
}

#[test]
fn short_body_is_unchanged() {
    let body = "operator: please rerun the flaky test";
    assert_eq!(truncate_for_relay(body, RELAY_MAX_BYTES), body);
}

#[test]
fn oversized_body_is_truncated_and_marked() {
    let body = "x".repeat(RELAY_MAX_BYTES * 2);
    let out = truncate_for_relay(&body, RELAY_MAX_BYTES);
    assert!(out.len() < body.len(), "must shrink an oversized body");
    assert!(
        out.contains("truncated"),
        "truncation must be visibly marked, got: {:?}",
        &out[out.len().saturating_sub(40)..]
    );
}

#[test]
fn truncation_never_splits_a_multibyte_char() {
    // '€' is 3 bytes; a byte cap that lands mid-character must round down to a
    // char boundary rather than produce invalid UTF-8 (or panic).
    let body = "€".repeat(4000); // 12_000 bytes, well over 4 KiB
    let out = truncate_for_relay(&body, RELAY_MAX_BYTES);
    // The invariant we care about: the result is valid UTF-8. `String` already
    // guarantees this, so the real assertion is "did not panic slicing" plus a
    // sane bound on the mirrored prefix.
    let prefix_bytes = out.find("truncated").unwrap_or(out.len());
    assert!(
        prefix_bytes <= RELAY_MAX_BYTES,
        "mirrored prefix must not exceed the byte cap"
    );
}

#[test]
fn empty_body_is_unchanged() {
    assert_eq!(truncate_for_relay("", RELAY_MAX_BYTES), "");
}

#[test]
fn fingerprint_round_trips_for_same_body() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let sid = "sess-r4-echo";
    let body = "session started";

    assert!(
        !is_recent_outbound_fingerprint(root, sid, body),
        "unseen body is not a recent outbound"
    );
    record_outbound_fingerprint(root, sid, body).expect("record fingerprint");
    assert!(
        is_recent_outbound_fingerprint(root, sid, body),
        "after mirroring our own body, the subscriber must recognize the echo"
    );
}

#[test]
fn fingerprint_does_not_match_a_different_body() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let sid = "sess-r4-distinct";
    record_outbound_fingerprint(root, sid, "hello from the agent").unwrap();
    assert!(
        !is_recent_outbound_fingerprint(root, sid, "a genuine operator instruction"),
        "a distinct operator message must NOT be suppressed as an echo"
    );
}

#[test]
fn fingerprints_are_scoped_per_session() {
    // One session's outbound must not suppress another session's inbound.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    record_outbound_fingerprint(root, "sess-A", "shared body").unwrap();
    assert!(is_recent_outbound_fingerprint(
        root,
        "sess-A",
        "shared body"
    ));
    assert!(
        !is_recent_outbound_fingerprint(root, "sess-B", "shared body"),
        "fingerprints must be isolated per session id"
    );
}
