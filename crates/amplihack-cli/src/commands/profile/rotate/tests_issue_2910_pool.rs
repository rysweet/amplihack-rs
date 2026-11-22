//! Issue #2910 (D12 as narrowed by S1, S4, R3, R4): the one organisation pool
//! of the rotator's target choice. Wired from `affinity_pool.rs`.
//!
//! The rotator no longer has a second pool for plan room. One pool, from one
//! set of candidates, says which logins may be ranked:
//!
//! 1. the logins in the live login's organisation, when any qualifies;
//! 2. otherwise the logins whose organisation is not known (today's rule: an
//!    unknown organisation is not a crossing, and the swap carries the
//!    existing "unresolved" warning);
//! 3. otherwise the logins in another organisation, a confirmed crossing, and
//!    only when `may_cross` is true.
//!
//! `may_cross` is true when the live login is an overflow login being left, or
//! when the live login has no room now (a window at 100%, or a recorded block).
//! It is false for an upgrade, for a live login that still has room, and for a
//! live login whose probe failed. With no known source organisation the pool
//! is unrestricted, as today.
//!
//! The candidate type is the shared one in `amplihack_utils`, which is not
//! changed (auto-switch uses it too). Here `capacity` means "qualifies and is
//! not held"; `room` is not read.
//!
//! API these tests assume (in `affinity_pool.rs`):
//!
//! ```text
//! pool(source: Option<&str>, may_cross: bool, candidates: &[Candidate<'_>])
//!     -> Vec<usize>
//! enum OrgCheck { Same, Cross, Refuse }       // Debug, Clone, Copy, PartialEq, Eq
//! org_check(live_org: Option<&str>, incoming_org: Option<&str>, may_cross: bool)
//!     -> OrgCheck
//! ```
//!
//! Pure functions: no file, no clock, no network.

use super::*;
use amplihack_utils::organisation_affinity::Candidate;

fn c(org: Option<&str>, usable: bool) -> Candidate<'_> {
    Candidate {
        org,
        eligible: usable,
        capacity: usable,
        room: false,
    }
}

// --- pool ---------------------------------------------------------------------

#[test]
fn a_same_organisation_login_excludes_every_other_login() {
    let cands = [
        c(Some("org-b"), true),
        c(Some("org-a"), true),
        c(None, true),
        c(Some("org-a"), true),
    ];
    assert_eq!(pool(Some("org-a"), true, &cands), [1, 3]);
    assert_eq!(pool(Some("org-a"), false, &cands), [1, 3]);
}

#[test]
fn with_no_same_organisation_login_the_unresolved_ones_come_next() {
    let cands = [
        c(Some("org-b"), true),
        c(None, true),
        c(Some("org-c"), true),
        c(None, true),
    ];
    assert_eq!(pool(Some("org-a"), true, &cands), [1, 3]);
    assert_eq!(pool(Some("org-a"), false, &cands), [1, 3]);
}

#[test]
fn a_confirmed_crossing_is_pooled_only_when_nothing_else_qualifies_and_may_cross() {
    let cands = [c(Some("org-b"), true), c(Some("org-c"), true)];
    assert_eq!(pool(Some("org-a"), true, &cands), [0, 1]);
    assert!(
        pool(Some("org-a"), false, &cands).is_empty(),
        "an upgrade, or a live login with room, never crosses"
    );
}

#[test]
fn a_same_organisation_login_that_does_not_qualify_does_not_hide_a_crossing() {
    // The same-organisation login is held or has no room: it is not usable.
    let cands = [c(Some("org-a"), false), c(Some("org-b"), true)];
    assert_eq!(pool(Some("org-a"), true, &cands), [1]);
    assert!(pool(Some("org-a"), false, &cands).is_empty());
}

#[test]
fn a_login_that_does_not_qualify_is_never_pooled() {
    let cands = [
        c(Some("org-a"), false),
        c(None, false),
        c(Some("org-b"), false),
    ];
    for may_cross in [true, false] {
        assert!(pool(Some("org-a"), may_cross, &cands).is_empty());
        assert!(pool(None, may_cross, &cands).is_empty());
    }
}

#[test]
fn an_eligible_login_that_is_not_qualifying_is_not_pooled_either() {
    // `capacity` is "qualifies and is not held": eligibility alone is not enough.
    let held = Candidate {
        org: Some("org-a"),
        eligible: true,
        capacity: false,
        room: true,
    };
    assert!(pool(Some("org-a"), true, &[held]).is_empty());
}

#[test]
fn the_plan_room_bit_is_not_read() {
    // The second pool of the old rule is gone: `room` alone adds nothing.
    let room_only = Candidate {
        org: Some("org-a"),
        eligible: true,
        capacity: false,
        room: true,
    };
    let usable = c(Some("org-b"), true);
    assert_eq!(pool(Some("org-a"), true, &[room_only, usable]), [1]);
}

#[test]
fn with_no_known_source_organisation_the_pool_is_every_usable_login() {
    let cands = [
        c(Some("org-b"), true),
        c(None, false),
        c(Some("org-a"), true),
        c(None, true),
    ];
    for may_cross in [true, false] {
        assert_eq!(pool(None, may_cross, &cands), [0, 2, 3]);
    }
}

#[test]
fn an_identifier_that_cannot_be_trusted_counts_as_unknown() {
    // The shared rule: empty, whitespace or control characters are no identifier.
    let cands = [
        c(Some("org-b"), true),
        c(Some("has space"), true),
        c(Some(""), true),
    ];
    // A source of that kind is no source: unrestricted.
    for bad in ["", "has space", "tab\there"] {
        assert_eq!(pool(Some(bad), false, &cands), [0, 1, 2], "source {bad:?}");
    }
    // A candidate of that kind is unresolved, not a crossing.
    assert_eq!(pool(Some("org-a"), false, &cands), [1, 2]);
}

#[test]
fn the_pool_keeps_the_order_of_the_candidates() {
    let cands: Vec<Candidate<'_>> = (0..10).map(|_| c(Some("org-a"), true)).collect();
    assert_eq!(
        pool(Some("org-a"), false, &cands),
        (0..10).collect::<Vec<_>>()
    );
}

#[test]
fn an_empty_candidate_list_gives_an_empty_pool() {
    assert!(pool(Some("org-a"), true, &[]).is_empty());
    assert!(pool(None, true, &[]).is_empty());
}

// --- org_check (S4): the check the swap step makes -----------------------------------

#[test]
fn the_same_organisation_goes_ahead() {
    for may_cross in [true, false] {
        assert_eq!(
            org_check(Some("org-a"), Some("org-a"), may_cross),
            OrgCheck::Same
        );
    }
}

#[test]
fn a_crossing_goes_ahead_only_when_the_decision_allowed_one() {
    assert_eq!(
        org_check(Some("org-a"), Some("org-b"), true),
        OrgCheck::Cross
    );
    assert_eq!(
        org_check(Some("org-a"), Some("org-b"), false),
        OrgCheck::Refuse
    );
}

#[test]
fn an_unknown_organisation_on_either_side_is_never_refused() {
    // As today: an unknown organisation is not a crossing; the swap goes ahead
    // and carries the "unresolved" warning.
    for may_cross in [true, false] {
        for (live, incoming) in [
            (None, Some("org-b")),
            (Some("org-a"), None),
            (None, None),
            (Some(""), Some("org-b")),
            (Some("org-a"), Some("has space")),
        ] {
            let got = org_check(live, incoming, may_cross);
            assert_ne!(got, OrgCheck::Refuse, "{live:?} -> {incoming:?}");
            assert_ne!(got, OrgCheck::Cross, "{live:?} -> {incoming:?}");
        }
    }
}
