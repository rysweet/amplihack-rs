//! Proposed pure policy contract. Inputs have already passed existing identity,
//! lease, exclusion, pin, spend, freshness and authorization checks. Returned
//! indices preserve input order so the existing ranking remains authoritative.
//! Intentionally red until the shared organisation_affinity module exists.
use amplihack_utils::organisation_affinity::{Affinity, Candidate, Purpose, allowed_indices};

#[test]
fn affinity_requires_present_valid_exactly_equal_ids() {
    assert_eq!(
        Affinity::between(Some("org-a"), Some("org-a")),
        Affinity::Same
    );
    assert_eq!(
        Affinity::between(Some("org-a"), Some("org-b")),
        Affinity::Crossing
    );
    assert_eq!(
        Affinity::between(Some("Org-A"), Some("org-a")),
        Affinity::Crossing
    );
    for (a, b) in [
        (None, None),
        (Some("org-a"), None),
        (None, Some("org-a")),
        (Some(""), Some("")),
        (Some("org\u{1b}[2J"), Some("org\u{1b}[2J")),
    ] {
        assert_eq!(Affinity::between(a, b), Affinity::Unresolved);
    }
}
fn c(org: Option<&str>, eligible: bool, capacity: bool) -> Candidate<'_> {
    Candidate {
        org,
        eligible,
        capacity,
    }
}
#[test]
fn same_org_capacity_restricts_the_pool_before_ranking() {
    let candidates = [
        c(Some("org-b"), true, true),
        c(Some("org-a"), true, true),
        c(None, true, true),
        c(Some("org-a"), true, true),
    ];
    assert_eq!(
        allowed_indices(Some("org-a"), Purpose::CapacityRelief, true, &candidates),
        vec![1, 3]
    );
}
#[test]
fn crossing_requires_necessary_capacity_relief_and_no_eligible_same_org_room() {
    let candidates = [
        c(Some("org-a"), false, true),
        c(Some("org-a"), true, false),
        c(Some("org-b"), true, true),
        c(None, true, true),
    ];
    assert_eq!(
        allowed_indices(Some("org-a"), Purpose::CapacityRelief, true, &candidates),
        vec![2, 3]
    );
    assert!(allowed_indices(Some("org-a"), Purpose::CapacityRelief, false, &candidates).is_empty());
}
#[test]
fn preference_only_purposes_require_confirmed_affinity_even_in_fallbacks() {
    let candidates = [
        c(Some("org-b"), true, true),
        c(None, true, true),
        c(Some("org-a"), true, true),
    ];
    for purpose in [Purpose::Upgrade, Purpose::PlanRoom] {
        for necessary in [false, true] {
            assert_eq!(
                allowed_indices(Some("org-a"), purpose, necessary, &candidates),
                vec![2]
            );
            assert!(allowed_indices(None, purpose, necessary, &candidates).is_empty());
        }
    }
}
#[test]
fn warnings_distinguish_evidence_from_risk_without_claiming_the_session_is_broken() {
    assert!(Affinity::Same.warning().is_none());
    let confirmed = Affinity::Crossing.warning().unwrap();
    assert!(confirmed.contains("earlier thinking") && confirmed.contains("next turn"));
    assert!(confirmed.contains("unavailable"));
    let unresolved = Affinity::Unresolved.warning().unwrap();
    assert!(unresolved.contains("unresolved") && unresolved.contains("earlier thinking"));
    assert!(unresolved.contains("may") || unresolved.contains("possible"));
    assert_ne!(confirmed, unresolved);
    for warning in [confirmed, unresolved] {
        assert!(!warning.contains("conversation disappears"));
        assert!(!warning.contains("cannot continue"));
    }
}
