//! TDD contract — F3: fail-closed group-membership parsing.
//!
//! Written **first** (Step 7 TDD). Relaying agent output leaks it to every
//! group member, so membership verification must be *positive-only*: any
//! member the bridge cannot fully account for withholds the relay.
//!
//! The current `parse_group_members` uses a `filter_map` that **silently
//! drops** a member lacking an E.164 `number` and returns the surviving subset
//! as `Ok`. That is fail-*open*: an unaccounted-for member (e.g. one whose
//! `number` field is absent) vanishes from the verified set, so the mismatch
//! check can spuriously pass. After F3, a member missing the `number` field is
//! a parse failure (`Err(WireError::Membership)`), which the caller maps to
//! `group_members == None` → `classify(_, None)` → `Membership::Unverified`,
//! and the relay is withheld.
//!
//! Run: `cargo test -p amplihack-signal --features signal --test
//! bridge_membership_failclosed_it`.
#![cfg(feature = "signal")]

use amplihack_signal::bridge::membership::{Membership, classify};
use amplihack_signal::transport::{WireError, parse_group_members};
use serde_json::json;

const GROUP: &str = "group.abcdef==";

/// A well-formed `listGroups` result where every member carries a `number`.
fn all_numbered() -> serde_json::Value {
    json!([{
        "id": GROUP,
        "members": [
            { "number": "+12065551234" },
            { "number": "+12065559999" },
        ]
    }])
}

/// The same group, but one member is missing the `number` field entirely
/// (e.g. a member known only by ACI/UUID). Under fail-closed parsing this must
/// NOT be silently dropped.
fn one_member_missing_number() -> serde_json::Value {
    json!([{
        "id": GROUP,
        "members": [
            { "number": "+12065551234" },
            { "uuid": "8d9f0e2a-0000-4000-8000-000000000000" },
        ]
    }])
}

#[test]
fn well_formed_members_parse_to_their_numbers() {
    // Sanity: the happy path is unchanged.
    let members =
        parse_group_members(&all_numbered(), GROUP).expect("fully-numbered members must parse");
    assert_eq!(members, vec!["+12065551234", "+12065559999"]);
}

#[test]
fn member_missing_number_is_a_parse_failure() {
    // Fail-closed: a member without a string `number` must make the whole
    // parse fail — never silently drop the member and return the subset.
    let err = parse_group_members(&one_member_missing_number(), GROUP)
        .expect_err("a member missing `number` must fail closed, not be dropped");
    assert!(
        matches!(err, WireError::Membership(_)),
        "expected WireError::Membership, got {err:?}"
    );
}

#[test]
fn member_missing_number_classifies_as_unverified() {
    // End-to-end fail-closed authorization: the caller turns the parse failure
    // into `None` (no positively-known member set), which classify() treats as
    // Unverified, so the relay is withheld.
    let expected = vec!["+12065551234".to_string(), "+12065559999".to_string()];
    let actual: Option<Vec<String>> = parse_group_members(&one_member_missing_number(), GROUP).ok();
    let membership = classify(&expected, actual.as_deref());

    assert!(
        matches!(membership, Membership::Unverified(_)),
        "a member missing its E.164 number must yield Unverified, got {membership:?}"
    );
    assert!(
        !membership.may_relay(),
        "relay must be withheld when membership is unverified"
    );
}

#[test]
fn member_with_non_string_number_is_a_parse_failure() {
    // A `number` present but not a string is equally unaccountable → fail closed.
    let value = json!([{
        "id": GROUP,
        "members": [
            { "number": "+12065551234" },
            { "number": 12065559999_i64 },
        ]
    }]);
    let err =
        parse_group_members(&value, GROUP).expect_err("a non-string `number` must fail closed");
    assert!(matches!(err, WireError::Membership(_)), "got {err:?}");
}

#[test]
fn parse_failure_message_does_not_leak_member_numbers() {
    // PII discipline: the error surfaced to logs/audit must reference the
    // defect, not embed any member phone number.
    let err = parse_group_members(&one_member_missing_number(), GROUP).unwrap_err();
    let WireError::Membership(msg) = err else {
        panic!("expected WireError::Membership");
    };
    assert!(
        !msg.contains("+12065551234"),
        "membership parse error must not leak a member number: {msg:?}"
    );
}
