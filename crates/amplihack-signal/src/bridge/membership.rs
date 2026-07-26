//! Outbound group-membership verification — **fail closed**.
//!
//! Because relaying agent output to the group leaks whatever the agent produced
//! to every member, the bridge verifies the group's member set matches the
//! expected operator-only set **before every outbound post**. Verification is
//! positive-only: anything other than an exact match — an RPC error, a timeout,
//! an ambiguous response, an unexpected extra member, or a missing expected
//! member — is treated as [`Membership::Unverified`] and **refuses** the relay.
//! The bridge never assumes "probably fine".

use std::collections::BTreeSet;

/// The result of comparing an actual group member set to the expected set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Membership {
    /// The actual member set exactly equals the expected operator-only set.
    Verified,
    /// Membership could not be positively verified; relay must be withheld. The
    /// string explains why (for the local terminal alert / audit log).
    Unverified(String),
}

impl Membership {
    /// Whether outbound relay is permitted. Only [`Membership::Verified`] is.
    #[must_use]
    pub fn may_relay(&self) -> bool {
        matches!(self, Membership::Verified)
    }
}

/// Classify group membership by comparing `actual` to `expected`.
///
/// `actual == None` models an RPC error / timeout / ambiguous response and is
/// always [`Membership::Unverified`]. A `Some` set must equal `expected` as a
/// set (order- and duplicate-insensitive) to be [`Membership::Verified`].
#[must_use]
pub fn classify(expected: &[String], actual: Option<&[String]>) -> Membership {
    let Some(actual) = actual else {
        return Membership::Unverified(
            "group membership query failed or was ambiguous (no member set)".to_string(),
        );
    };
    let expected_set: BTreeSet<&String> = expected.iter().collect();
    let actual_set: BTreeSet<&String> = actual.iter().collect();
    if expected_set == actual_set {
        return Membership::Verified;
    }
    let unexpected: Vec<&&String> = actual_set.difference(&expected_set).collect();
    let missing: Vec<&&String> = expected_set.difference(&actual_set).collect();
    Membership::Unverified(format!(
        "group membership mismatch (unexpected: {unexpected:?}, missing: {missing:?})"
    ))
}
