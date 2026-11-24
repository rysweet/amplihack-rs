//! Proposed pure policy contract. Inputs have already passed existing identity,
//! lease, exclusion, pin, spend, freshness and authorization checks. Returned
//! indices preserve input order so the existing ranking remains authoritative.
//! Only a confirmed crossing is restricted: unknown is not crossing.
use amplihack_utils::organisation_affinity::{
    Affinity, Candidate, Need, Purpose, allowed_indices, pools, preference_permitted,
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
        room: capacity,
    }
}
fn r(org: Option<&str>, capacity: bool, room: bool) -> Candidate<'_> {
    Candidate {
        org,
        eligible: true,
        capacity,
        room,
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
    // recorded: the recovered plan login must stay reachable.
    let candidates = [c(None, true, true), c(Some("org-a"), true, true)];
    assert_eq!(
        pools(None, Need::LeaveOverflow, &candidates).allowed,
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
        // Room without strict capacity.
        r(Some("org-a"), false, true),
        c(Some("org-a"), false, true),
    ];
    let p = pools(Some("org-a"), Need::Relief, &candidates);
    assert_eq!(p.allowed, vec![0], "necessary relief may cross");
    assert_eq!(p.plan_room, vec![1], "room fallback never crosses");
}
#[test]
fn a_same_org_login_without_room_does_not_exclude_an_unknown_org_login_with_room() {
    // Same-organisation login: no capacity, no room. Unknown-organisation
    // login: room but not strict capacity. The plan-room fallback must keep
    // the unknown one, or a session stays on paid overflow.
    let candidates = [r(Some("org-a"), false, false), r(None, false, true)];
    let p = pools(Some("org-a"), Need::Relief, &candidates);
    assert_eq!(p.plan_room, vec![1]);
    // With room on the same-organisation login it is preferred.
    let candidates = [r(Some("org-a"), false, true), r(None, false, true)];
    assert_eq!(
        pools(Some("org-a"), Need::Relief, &candidates).plan_room,
        vec![0]
    );
}
#[test]
fn need_is_worked_out_in_one_place() {
    assert_eq!(Need::of(true, false), Need::LeaveOverflow);
    assert_eq!(Need::of(true, true), Need::LeaveOverflow);
    assert_eq!(Need::of(false, true), Need::Relief);
    assert_eq!(Need::of(false, false), Need::Upgrade);
}
#[test]
fn leaving_paid_overflow_may_cross_only_without_a_same_org_plan_login() {
    // Crusty round 7 on #2295: the overflow login's organisation (org-b) is
    // recorded and the only plan login with capacity is in org-a. Leaving
    // paid spend is cost relief, so the crossing is permitted.
    let cross = [c(Some("org-a"), true, true)];
    assert_eq!(
        pools(Some("org-b"), Need::LeaveOverflow, &cross).allowed,
        vec![0]
    );
    // A same-organisation plan login with capacity excludes the crossing.
    let both = [c(Some("org-a"), true, true), c(Some("org-b"), true, true)];
    assert_eq!(
        pools(Some("org-b"), Need::LeaveOverflow, &both).allowed,
        vec![1]
    );
    // The room fallback off overflow may cross on the same terms ...
    let room = [r(Some("org-a"), false, true)];
    assert_eq!(
        pools(Some("org-b"), Need::LeaveOverflow, &room).plan_room,
        vec![0]
    );
    let room_both = [r(Some("org-a"), false, true), r(Some("org-b"), false, true)];
    assert_eq!(
        pools(Some("org-b"), Need::LeaveOverflow, &room_both).plan_room,
        vec![1]
    );
    // ... while plan room from a plan login never crosses.
    for need in [Need::Upgrade, Need::Relief] {
        assert!(pools(Some("org-b"), need, &room).plan_room.is_empty());
    }
}
#[test]
fn log_lines_carry_the_affinity_kind_and_both_profiles() {
    assert_eq!(Affinity::Same.log_line("x", "y"), None);
    let line = Affinity::Unresolved.log_line("x", "y").unwrap();
    assert!(line.starts_with("affinity: unresolved x->y: Organisation affinity is unresolved"));
    let line = Affinity::Crossing.log_line("x", "y").unwrap();
    assert!(line.starts_with("affinity: crossing x->y: Cross-organisation swap"));
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
