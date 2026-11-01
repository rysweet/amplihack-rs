//! Issue #1811 (TDD): the pure decision function, steps 5 to 7, rate
//! feedback inside `decide`, nesting, and the "deny only at step 2" rule.
//!
//! Wire with `#[cfg(test)] mod tests_policy_flow;` in `governor/mod.rs`.
//! Uses the fixture from `tests_policy`. Expected extra API:
//! - `model::Slot { pid, start_time: Option<u64>, workstream, class, tree_id,
//!   admitted_at, nested_children: u32, start: Start }`
//! - `model::QueueEntry { seq, workstream, class, pid, start_time:
//!   Option<u64>, enqueued_at }`
//! - `model::Admission { workstream, at }`
//! - `Decision { limit_after: u32, evaluated: bool, .. }`

use super::model::{Admission, Class, DecisionKind, QueueEntry, ReasonCode, Slot, Start};
use super::policy::decide;
use super::rate::RateSignal;
use super::tests_policy::fixture::{budget, check, inputs, weekly_only};
use super::tests_policy::{HOUR, NOW};

fn slot(pid: u32, workstream: &str) -> Slot {
    Slot {
        pid,
        start_time: Some(1),
        workstream: workstream.to_string(),
        class: Class::Ordinary,
        tree_id: format!("t-{pid}"),
        admitted_at: NOW - 60,
        nested_children: 0,
        start: Start::Agent,
    }
}

fn queued(seq: u64, workstream: &str, class: Class) -> QueueEntry {
    QueueEntry {
        seq,
        workstream: workstream.to_string(),
        class,
        pid: 1000 + seq as u32,
        start_time: Some(1),
        enqueued_at: NOW - 30,
    }
}

fn roomy() -> Vec<super::model::Budget> {
    vec![budget("alpha", true, 10.0, 20.0)]
}

#[test]
fn step5_waits_when_slots_are_at_the_limit() {
    let mut i = inputs(roomy(), Class::Ordinary);
    i.limit = 2;
    i.slots = vec![slot(10, "a"), slot(11, "b")];
    let d = decide(&i, NOW);
    check(&d, DecisionKind::Wait, ReasonCode::Concurrency);
    assert_eq!(d.limit_after, 2);
}

#[test]
fn step5_applies_to_protected_classes_too() {
    let mut i = inputs(roomy(), Class::Coordination);
    i.limit = 1;
    i.slots = vec![slot(10, "a")];
    check(
        &decide(&i, NOW),
        DecisionKind::Wait,
        ReasonCode::Concurrency,
    );
}

#[test]
fn rate_over_steps_the_limit_down_on_evaluation() {
    let mut i = inputs(roomy(), Class::Ordinary);
    i.limit = 2;
    i.slots = vec![slot(10, "a")];
    i.signal = RateSignal::Over;
    i.last_evaluated_at = None;
    let d = decide(&i, NOW);
    assert!(d.evaluated);
    assert_eq!(d.limit_after, 1);
    check(&d, DecisionKind::Wait, ReasonCode::Concurrency);
}

#[test]
fn rate_over_at_limit_one_keeps_one_slot_and_never_denies() {
    let mut i = inputs(roomy(), Class::Ordinary);
    i.limit = 1;
    i.signal = RateSignal::Over;
    i.last_evaluated_at = None;
    let d = decide(&i, NOW);
    assert_eq!(d.limit_after, 1);
    check(&d, DecisionKind::Admit, ReasonCode::Room);
    i.slots = vec![slot(10, "a")];
    i.request.deadline = NOW;
    let d = decide(&i, NOW);
    check(&d, DecisionKind::Wait, ReasonCode::Concurrency);
}

#[test]
fn rate_under_steps_up_and_is_clamped_to_max() {
    let mut i = inputs(roomy(), Class::Ordinary);
    i.signal = RateSignal::Under;
    i.last_evaluated_at = None;
    i.limit = 2;
    assert_eq!(decide(&i, NOW).limit_after, 3);
    i.limit = 4;
    assert_eq!(decide(&i, NOW).limit_after, 4);
    i.settings.step = 3;
    i.limit = 2;
    assert_eq!(decide(&i, NOW).limit_after, 4);
}

#[test]
fn rate_steady_and_unknown_keep_the_limit() {
    for signal in [RateSignal::Steady, RateSignal::Unknown] {
        let mut i = inputs(roomy(), Class::Ordinary);
        i.signal = signal;
        i.last_evaluated_at = None;
        i.limit = 3;
        let d = decide(&i, NOW);
        assert!(d.evaluated);
        assert_eq!(d.limit_after, 3, "{signal:?}");
    }
}

#[test]
fn evaluation_runs_at_most_once_per_interval() {
    let mut i = inputs(roomy(), Class::Ordinary);
    i.signal = RateSignal::Over;
    i.limit = 3;
    i.last_evaluated_at = Some(NOW - 59);
    let d = decide(&i, NOW);
    assert!(!d.evaluated);
    assert_eq!(d.limit_after, 3);
    i.last_evaluated_at = Some(NOW - 60);
    let d = decide(&i, NOW);
    assert!(d.evaluated);
    assert_eq!(d.limit_after, 2);
}

#[test]
fn step6_waits_behind_an_earlier_request_in_the_same_class() {
    let mut i = inputs(roomy(), Class::Ordinary);
    i.queue = vec![
        queued(1, "a", Class::Ordinary),
        queued(2, "ws-1", Class::Ordinary),
    ];
    i.request.seq = Some(2);
    check(&decide(&i, NOW), DecisionKind::Wait, ReasonCode::Queue);
}

#[test]
fn step6_head_of_the_queue_is_admitted() {
    let mut i = inputs(roomy(), Class::Ordinary);
    i.queue = vec![
        queued(1, "ws-1", Class::Ordinary),
        queued(2, "a", Class::Ordinary),
    ];
    i.request.seq = Some(1);
    check(&decide(&i, NOW), DecisionKind::Admit, ReasonCode::Room);
}

#[test]
fn step6_a_request_not_yet_queued_waits_behind_the_queue() {
    let mut i = inputs(roomy(), Class::Ordinary);
    i.queue = vec![queued(1, "a", Class::Ordinary)];
    check(&decide(&i, NOW), DecisionKind::Wait, ReasonCode::Queue);
}

#[test]
fn step6_class_rank_comes_before_seq() {
    let mut i = inputs(roomy(), Class::Review);
    i.queue = vec![
        queued(1, "a", Class::Ordinary),
        queued(9, "ws-1", Class::Review),
    ];
    i.request.seq = Some(9);
    check(&decide(&i, NOW), DecisionKind::Admit, ReasonCode::Room);
}

#[test]
fn step6_fairness_prefers_the_workstream_with_fewer_admissions() {
    let mut i = inputs(roomy(), Class::Ordinary);
    i.queue = vec![
        queued(1, "a", Class::Ordinary),
        queued(2, "ws-1", Class::Ordinary),
    ];
    i.admissions = vec![Admission {
        workstream: "a".to_string(),
        at: NOW - HOUR,
    }];
    i.request.seq = Some(2);
    check(&decide(&i, NOW), DecisionKind::Admit, ReasonCode::Room);
}

#[test]
fn nested_skips_concurrency_and_queue() {
    // Under a recipe's slot and under a session's slot alike (D2): at a
    // limit of 1 the parent would otherwise wait on itself.
    for (start, parent) in [
        (Start::Agent, Start::Recipe),
        (Start::Recipe, Start::Recipe),
        (Start::Recipe, Start::Agent),
        (Start::Agent, Start::Agent),
    ] {
        let mut i = inputs(roomy(), Class::Ordinary);
        i.limit = 1;
        i.slots = vec![slot(10, "a")];
        i.queue = vec![queued(1, "a", Class::Coordination)];
        i.request.start = start;
        i.request.parent = Some(parent);
        check(&decide(&i, NOW), DecisionKind::Admit, ReasonCode::Nested);
    }
}

#[test]
fn nested_is_still_held_by_session_exhaustion() {
    let mut i = inputs(
        vec![budget("alpha", true, 100.0, 50.0)],
        Class::Coordination,
    );
    i.request.parent = Some(Start::Recipe);
    check(
        &decide(&i, NOW),
        DecisionKind::Wait,
        ReasonCode::SessionExhausted,
    );
}

#[test]
fn deny_comes_only_from_weekly_exhaustion() {
    let pools = [
        Vec::new(),
        roomy(),
        vec![budget("alpha", true, 100.0, 50.0)],
        vec![budget("alpha", true, 0.0, 95.0)],
        vec![weekly_only("alpha", true, 100.0)],
        vec![
            budget("alpha", true, 0.0, 100.0),
            budget("beta", false, 0.0, 100.0),
        ],
    ];
    for pool in pools {
        for class in [Class::Coordination, Class::Ordinary] {
            for (parent, slots) in [
                (None, 0),
                (None, 4),
                (Some(Start::Agent), 4),
                (Some(Start::Recipe), 4),
            ] {
                for signal in [RateSignal::Over, RateSignal::Unknown] {
                    let mut i = inputs(pool.clone(), class);
                    i.request.parent = parent;
                    i.request.deadline = NOW;
                    i.slots = (0..slots).map(|p| slot(10 + p, "a")).collect();
                    i.queue = vec![queued(1, "a", Class::Coordination)];
                    i.signal = signal;
                    i.last_evaluated_at = None;
                    let d = decide(&i, NOW);
                    if d.kind == DecisionKind::Deny {
                        assert_eq!(d.reason.code, ReasonCode::WeeklyExhausted, "{d:?}");
                    }
                    assert!((1..=4).contains(&d.limit_after), "{d:?}");
                }
            }
        }
    }
}

#[test]
fn decide_does_not_depend_on_profile_order() {
    let a = budget("alpha", true, 100.0, 50.0);
    let b = budget("beta", false, 30.0, 60.0);
    let c = weekly_only("gamma", false, 70.0);
    let one = decide(
        &inputs(vec![a.clone(), b.clone(), c.clone()], Class::Ordinary),
        NOW,
    );
    let two = decide(&inputs(vec![c, b, a], Class::Ordinary), NOW);
    assert_eq!(one, two);
}
