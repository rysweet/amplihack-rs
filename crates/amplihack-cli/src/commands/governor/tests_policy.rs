//! Issue #1811 (TDD): the pure decision function, steps 1 to 4.
//!
//! Wire with `#[cfg(test)] mod tests_policy;` in `governor/mod.rs`.
//!
//! Expected API (see docs/reference/capacity-governor.md#model):
//! - `model::{Budget, WindowBudget, Class, Request, Inputs, Slot, QueueEntry,
//!   Admission, Decision, DecisionKind, ReasonCode}` with public fields as
//!   built in `fixture` below. `Decision` derives `Debug` and `PartialEq`.
//! - `rate::RateSignal::{Over, Under, Steady, Unknown}`.
//! - `config::Settings` with `Default` (4, 10, 1, 10, 60, 600, enabled).
//! - `policy::decide(&Inputs, now) -> Decision`. No I/O, no environment.
//!
//! `fixture` is shared with `tests_policy_flow` through `pub(super)`.

use super::config::Settings;
use super::model::{
    Budget, Class, Decision, DecisionKind, Inputs, ReasonCode, Request, WindowBudget,
};
use super::policy::decide;
use super::rate::RateSignal;

pub(super) const NOW: i64 = 1_800_000_000;
pub(super) const HOUR: i64 = 3600;

pub(super) mod fixture {
    use super::*;

    pub fn win(used_pct: f64, resets_in: i64) -> WindowBudget {
        WindowBudget {
            used_pct,
            resets_at: Some(NOW + resets_in),
        }
    }

    /// A profile with a session window.
    pub fn budget(name: &str, active: bool, session: f64, weekly: f64) -> Budget {
        Budget {
            name: name.to_string(),
            active,
            eligible: true,
            session: Some(win(session, 2 * HOUR)),
            weekly: win(weekly, 48 * HOUR),
        }
    }

    /// A weekly-only profile (D12).
    pub fn weekly_only(name: &str, active: bool, weekly: f64) -> Budget {
        Budget {
            session: None,
            ..budget(name, active, 0.0, weekly)
        }
    }

    pub fn request(class: Class) -> Request {
        Request {
            class,
            workstream: "ws-1".to_string(),
            nested: false,
            seq: None,
            deadline: NOW + 600,
        }
    }

    pub fn inputs(budgets: Vec<Budget>, class: Class) -> Inputs {
        Inputs {
            budgets,
            signal: RateSignal::Unknown,
            limit: 4,
            last_evaluated_at: Some(NOW),
            slots: Vec::new(),
            queue: Vec::new(),
            admissions: Vec::new(),
            settings: Settings::default(),
            request: request(class),
        }
    }

    pub fn check(d: &Decision, kind: DecisionKind, code: ReasonCode) {
        assert_eq!((d.kind, d.reason.code), (kind, code), "{d:?}");
    }
}

use fixture::*;

const CLASSES: [Class; 4] = [
    Class::Coordination,
    Class::Recovery,
    Class::Review,
    Class::Ordinary,
];

#[test]
fn step1_no_budgets_admits_fail_open() {
    let d = decide(&inputs(Vec::new(), Class::Ordinary), NOW);
    check(&d, DecisionKind::Admit, ReasonCode::NoReadings);
}

#[test]
fn step1_ineligible_profiles_do_not_count_as_readings() {
    let mut overflow = budget("credits", true, 100.0, 100.0);
    overflow.eligible = false;
    let d = decide(&inputs(vec![overflow], Class::Ordinary), NOW);
    check(&d, DecisionKind::Admit, ReasonCode::NoReadings);
}

#[test]
fn step2_weekly_exhausted_waits_for_the_earliest_weekly_reset() {
    let mut a = budget("alpha", true, 10.0, 100.0);
    let b = budget("beta", false, 10.0, 100.0);
    a.weekly.resets_at = Some(NOW + 300);
    let mut i = inputs(vec![a, b], Class::Ordinary);
    i.request.deadline = NOW + 3600;
    let d = decide(&i, NOW);
    check(&d, DecisionKind::Wait, ReasonCode::WeeklyExhausted);
    assert_eq!(d.retry_after, Some(NOW + 300));
}

#[test]
fn step2_denies_when_the_reset_is_after_the_deadline() {
    let i = inputs(vec![budget("alpha", true, 10.0, 100.0)], Class::Ordinary);
    let d = decide(&i, NOW);
    check(&d, DecisionKind::Deny, ReasonCode::WeeklyExhausted);
}

#[test]
fn step2_denies_at_once_when_the_wait_is_zero() {
    let mut a = budget("alpha", true, 10.0, 100.0);
    a.weekly.resets_at = Some(NOW + 1);
    let mut i = inputs(vec![a], Class::Ordinary);
    i.request.deadline = NOW;
    check(
        &decide(&i, NOW),
        DecisionKind::Deny,
        ReasonCode::WeeklyExhausted,
    );
}

#[test]
fn step2_holds_every_class_and_nested_requests() {
    for class in CLASSES {
        for nested in [false, true] {
            let mut i = inputs(vec![weekly_only("alpha", true, 100.0)], class);
            i.request.nested = nested;
            let d = decide(&i, NOW);
            assert_eq!(
                d.reason.code,
                ReasonCode::WeeklyExhausted,
                "{class:?} {d:?}"
            );
            assert_ne!(d.kind, DecisionKind::Admit);
        }
    }
}

#[test]
fn step2_one_profile_with_weekly_room_is_enough_and_names_the_rotator() {
    let i = inputs(
        vec![
            budget("alpha", true, 10.0, 100.0),
            budget("beta", false, 10.0, 40.0),
        ],
        Class::Ordinary,
    );
    let d = decide(&i, NOW);
    check(&d, DecisionKind::Admit, ReasonCode::Room);
    assert!(d.announce, "room is on a profile other than the active one");
    assert!(d.reason.message.contains("beta"), "{}", d.reason.message);
    assert!(d.reason.message.contains("rotator"), "{}", d.reason.message);
}

#[test]
fn room_on_the_active_profile_is_not_announced() {
    let i = inputs(
        vec![
            budget("alpha", true, 10.0, 20.0),
            budget("beta", false, 10.0, 40.0),
        ],
        Class::Ordinary,
    );
    let d = decide(&i, NOW);
    check(&d, DecisionKind::Admit, ReasonCode::Room);
    assert!(!d.announce);
}

#[test]
fn step3_session_exhausted_waits_for_every_class_until_the_session_reset() {
    for class in CLASSES {
        let mut a = budget("alpha", true, 100.0, 50.0);
        a.session = Some(win(100.0, 900));
        let d = decide(&inputs(vec![a], class), NOW);
        check(&d, DecisionKind::Wait, ReasonCode::SessionExhausted);
        assert_eq!(d.retry_after, Some(NOW + 900), "{class:?}");
    }
}

#[test]
fn step3_session_exhaustion_never_denies_even_with_wait_zero() {
    let mut i = inputs(vec![budget("alpha", true, 100.0, 50.0)], Class::Ordinary);
    i.request.deadline = NOW;
    let d = decide(&i, NOW);
    check(&d, DecisionKind::Wait, ReasonCode::SessionExhausted);
}

#[test]
fn weekly_only_profile_is_never_session_exhausted() {
    let i = inputs(
        vec![
            budget("alpha", true, 100.0, 50.0),
            weekly_only("beta", false, 40.0),
        ],
        Class::Ordinary,
    );
    let d = decide(&i, NOW);
    check(&d, DecisionKind::Admit, ReasonCode::Room);
    assert!(d.reason.message.contains("beta"));
}

#[test]
fn imminent_session_reset_never_adds_weekly_capacity() {
    // 99% weekly, session resetting in two minutes: 1 usable point.
    let mut a = budget("alpha", true, 10.0, 99.0);
    a.session = Some(win(10.0, 120));
    let i = inputs(vec![a.clone()], Class::Ordinary);
    check(&decide(&i, NOW), DecisionKind::Wait, ReasonCode::Reserve);
    let mut i = inputs(vec![a], Class::Ordinary);
    i.settings.reserve_points = 0;
    check(&decide(&i, NOW), DecisionKind::Admit, ReasonCode::Room);
}

#[test]
fn step4_reserve_holds_ordinary_at_the_boundary() {
    // Remainder exactly 10 with reserve 10: held.
    let i = inputs(vec![budget("alpha", true, 0.0, 90.0)], Class::Ordinary);
    check(&decide(&i, NOW), DecisionKind::Wait, ReasonCode::Reserve);
    // Just above the reserve: admitted.
    let i = inputs(vec![budget("alpha", true, 0.0, 89.5)], Class::Ordinary);
    check(&decide(&i, NOW), DecisionKind::Admit, ReasonCode::Room);
}

#[test]
fn step4_uses_the_best_usable_weekly_remainder() {
    let i = inputs(
        vec![
            budget("alpha", true, 0.0, 95.0),
            budget("beta", false, 0.0, 50.0),
        ],
        Class::Ordinary,
    );
    check(&decide(&i, NOW), DecisionKind::Admit, ReasonCode::Room);
}

#[test]
fn step4_protected_classes_may_spend_the_reserve() {
    for class in [Class::Coordination, Class::Recovery, Class::Review] {
        let i = inputs(vec![budget("alpha", true, 0.0, 95.0)], class);
        check(&decide(&i, NOW), DecisionKind::Admit, ReasonCode::Room);
    }
}

#[test]
fn step4_reserve_still_holds_nested_ordinary_work() {
    let mut i = inputs(vec![budget("alpha", true, 0.0, 95.0)], Class::Ordinary);
    i.request.nested = true;
    check(&decide(&i, NOW), DecisionKind::Wait, ReasonCode::Reserve);
}

#[test]
fn step4_reserve_never_denies() {
    let mut i = inputs(vec![budget("alpha", true, 0.0, 95.0)], Class::Ordinary);
    i.request.deadline = NOW;
    check(&decide(&i, NOW), DecisionKind::Wait, ReasonCode::Reserve);
}
