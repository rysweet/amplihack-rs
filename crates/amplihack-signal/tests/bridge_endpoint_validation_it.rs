//! TDD contract — F1: consolidated loopback endpoint validation.
//!
//! Written **first** (Step 7 TDD). These assertions specify the behavior of
//! the *single* canonical loopback+port validator after the two divergent
//! implementations (`bridge::validate_endpoint` and the CLI's
//! `validate::validate_loopback_endpoint`) are consolidated.
//!
//! The public runtime entry point is `bridge::validate_endpoint`; after F1 it
//! delegates to the stricter canonical validator, so it must:
//!   * ACCEPT bracket-less (bare) IPv6 loopback `::1` with a port — the bug
//!     this hardening pass fixes (the runtime path previously mis-split the
//!     bracket-less form and false-REJECTED it);
//!   * ACCEPT the other loopback forms (`127.0.0.0/8`, bracketed `[::1]`,
//!     literal `localhost`) with a valid port;
//!   * REJECT wildcard binds (`0.0.0.0`, `::`), routable addresses, and DNS
//!     names — a routable/wildcard daemon bind exposes the port off-host;
//!   * REJECT malformed / zero / out-of-range ports (fail closed);
//!   * still honor the explicit `unsafe_remote` opt-in as the *only* way to
//!     reach a non-loopback endpoint.
//!
//! Run: `cargo test -p amplihack-signal --features signal --test
//! bridge_endpoint_validation_it`.
#![cfg(feature = "signal")]

use amplihack_signal::bridge::{BridgeError, validate_endpoint};

/// Fail-closed default (no unsafe opt-in): accept only loopback host + valid
/// port.
fn accepts(endpoint: &str) -> bool {
    validate_endpoint(endpoint, false).is_ok()
}

#[test]
fn bare_ipv6_loopback_is_accepted() {
    // The core bug fix: bracket-less `::1` with a port must be ACCEPTED.
    assert!(
        accepts("::1:7583"),
        "bare (bracket-less) IPv6 loopback ::1 with a port must be accepted"
    );
}

#[test]
fn bracketed_ipv6_loopback_is_accepted() {
    assert!(accepts("[::1]:7583"), "[::1]:port must be accepted");
}

#[test]
fn ipv4_loopback_forms_are_accepted() {
    assert!(accepts("127.0.0.1:7583"));
    assert!(
        accepts("127.0.0.5:9000"),
        "the whole 127.0.0.0/8 block is loopback"
    );
}

#[test]
fn localhost_literal_is_accepted() {
    assert!(accepts("localhost:7583"));
}

#[test]
fn ipv4_wildcard_is_rejected() {
    assert!(
        !accepts("0.0.0.0:7583"),
        "0.0.0.0 is a wildcard bind, not loopback"
    );
}

#[test]
fn ipv6_wildcard_is_rejected() {
    assert!(
        !accepts("[::]:7583"),
        "[::] is the IPv6 wildcard, not loopback"
    );
    assert!(
        !accepts("::"),
        "the bare IPv6 unspecified address must never be accepted"
    );
}

#[test]
fn routable_addresses_are_rejected() {
    assert!(!accepts("10.0.0.5:7583"), "private-routable, not loopback");
    assert!(
        !accepts("93.184.216.34:7583"),
        "public-routable, not loopback"
    );
    assert!(!accepts("[2606:4700:4700::1111]:7583"), "routable IPv6");
}

#[test]
fn dns_names_are_rejected() {
    assert!(
        !accepts("example.com:7583"),
        "DNS names must be rejected (no resolution, fail closed)"
    );
    assert!(!accepts("signal.local:7583"));
    // `localhost.evil.example` must NOT be treated as the literal `localhost`.
    assert!(!accepts("localhost.evil.example:7583"));
}

#[test]
fn malformed_and_out_of_range_ports_are_rejected() {
    assert!(!accepts("127.0.0.1:0"), "port 0 is invalid");
    assert!(!accepts("127.0.0.1:70000"), "port > 65535 is invalid");
    assert!(!accepts("127.0.0.1:abc"), "non-numeric port");
    assert!(!accepts("127.0.0.1:"), "empty port");
    assert!(!accepts("127.0.0.1"), "missing port");
    assert!(!accepts(""), "empty endpoint");
}

#[test]
fn unsafe_remote_opt_in_bypasses_loopback_check() {
    // The explicit, documented opt-in is the ONLY way to reach a non-loopback
    // endpoint. It must still work after consolidation.
    assert!(validate_endpoint("10.0.0.5:7583", true).is_ok());
    assert!(validate_endpoint("example.com:7583", true).is_ok());
}

#[test]
fn rejections_use_the_stable_exit_code_2_taxonomy() {
    // Every loopback-safety rejection must remain `RemoteEndpointRejected`
    // (exit code 2) — no new error variants, no taxonomy drift.
    let err = validate_endpoint("0.0.0.0:7583", false).unwrap_err();
    assert!(matches!(err, BridgeError::RemoteEndpointRejected));
    assert_eq!(err.exit_code(), 2);

    let err = validate_endpoint("127.0.0.1:0", false).unwrap_err();
    assert!(matches!(err, BridgeError::RemoteEndpointRejected));
    assert_eq!(err.exit_code(), 2);
}
