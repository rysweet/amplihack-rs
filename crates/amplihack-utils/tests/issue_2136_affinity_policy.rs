//! Proposed pure policy contract. Inputs have already passed existing identity,
//! lease, exclusion, pin, spend, freshness and authorization checks. Returned
//! indices preserve input order so the existing ranking remains authoritative.
//! Only a confirmed crossing is restricted: unknown is not crossing.
use amplihack_utils::organisation_affinity::{
    Affinity, Candidate, Purpose, allowed_indices, pools, preference_permitted,
};

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
    // Not necessary: the crossing is refused, the unknown target is not.
    assert_eq!(
        allowed_indices(Some("org-a"), Purpose::CapacityRelief, false, &candidates),
        vec![3]
    );
}
#[test]
fn preference_only_purposes_never_cross_but_unknown_is_not_crossing() {
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
            // Unknown source: nothing is a confirmed crossing.
            assert_eq!(
                allowed_indices(None, purpose, necessary, &candidates),
                vec![0, 1, 2]
            );
        }
        // Known source, no same-organisation target: only the crossing goes.
        assert_eq!(
            allowed_indices(Some("org-a"), purpose, false, &candidates[..2]),
            vec![1]
        );
    }
}
#[test]
fn unknown_source_still_returns_from_overflow_to_a_recovered_plan_login() {
    // A session on a paid overflow login whose organisation was never
    // recorded: the recovered plan login must stay reachable (an upgrade).
    let candidates = [c(None, true, true), c(Some("org-a"), true, true)];
    assert_eq!(
        pools(None, false, &candidates).allowed,
        vec![0, 1],
        "missing metadata must not keep a session on overflow"
    );
    assert!(preference_permitted(None, Some("org-a")));
    assert!(preference_permitted(Some("org-a"), None));
    assert!(!preference_permitted(Some("org-a"), Some("org-b")));
}
#[test]
fn pools_give_the_plan_room_fallback_its_own_non_crossing_pool() {
    let candidates = [
        c(Some("org-b"), true, true),
        c(Some("org-a"), true, false),
        c(Some("org-a"), false, true),
    ];
    let p = pools(Some("org-a"), true, &candidates);
    assert_eq!(p.allowed, vec![0], "necessary relief may cross");
    assert_eq!(p.plan_room, vec![1], "room fallback never crosses");
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
