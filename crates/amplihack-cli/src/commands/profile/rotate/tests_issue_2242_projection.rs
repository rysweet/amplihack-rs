use super::super::super::decide::Decision;
use super::super::super::{fresh, observe};
use super::*;

fn verdict(c: &mut Ctx) -> Decision {
    let state = State::new(c.store.root()).unwrap();
    let snap = observe::snapshot(c, &state).unwrap();
    let m = measure(c, &snap).unwrap();
    fresh::live::step(c, &snap, m).unwrap().decision
}

fn is_swap(d: Decision) -> bool {
    matches!(d, Decision::Swap { .. })
}

#[test]
fn weekly_consumption_predicts_exhaustion_below_the_normal_threshold() {
    let e = fixture();
    let mut c = context(&e);
    c.settings.week_at = 99;
    sample(10.0, 45.0, 0, T + 10_000);
    assert!(!is_swap(verdict(&mut c)));
    advance(120);
    sample(10.0, 75.0, 0, T + 10_000);
    assert!(
        is_swap(verdict(&mut c)),
        "weekly usage reaches 100 before the next cycle"
    );
}

#[test]
fn projection_includes_observation_age_before_the_next_interval() {
    let e = fixture();
    let mut c = context(&e);
    c.settings.session_at = 99;
    sample(40.0, 20.0, 20, T + 10_000);
    assert!(!is_swap(verdict(&mut c)));
    advance(120);
    sample(68.0, 20.0, 20, T + 10_000);
    assert!(
        is_swap(verdict(&mut c)),
        "28/120 * (20 + 120) predicts 100.67% use"
    );
}

#[test]
fn reset_before_predicted_exhaustion_prevents_an_early_swap() {
    let e = fixture();
    let mut c = context(&e);
    c.settings.session_at = 99;
    let reset = T + 150;
    sample(50.0, 20.0, 0, reset);
    assert!(!is_swap(verdict(&mut c)));
    advance(120);
    sample(75.0, 20.0, 0, reset);
    assert!(
        !is_swap(verdict(&mut c)),
        "reset occurs 90 seconds before predicted exhaustion"
    );
}

#[test]
fn missing_history_still_permits_an_ordinary_threshold_swap() {
    let e = fixture();
    let mut c = context(&e);
    sample(86.0, 20.0, 0, T + 10_000);
    assert!(is_swap(verdict(&mut c)));
}

#[test]
fn a_short_or_reset_crossing_history_cannot_manufacture_a_rate() {
    for reset_changed in [false, true] {
        let e = fixture();
        let mut c = context(&e);
        c.settings.session_at = 99;
        sample(20.0, 20.0, 0, T + 150);
        assert!(!is_swap(verdict(&mut c)));
        advance(if reset_changed { 120 } else { 1 });
        sample(
            70.0,
            20.0,
            0,
            if reset_changed { T + 20_000 } else { T + 150 },
        );
        assert!(!is_swap(verdict(&mut c)));
    }
}

#[test]
fn absent_reset_evidence_cannot_authorise_a_consumption_projection() {
    for weekly in [false, true] {
        let e = fixture();
        let mut c = context(&e);
        c.settings.session_at = 99;
        c.settings.week_at = 99;
        for (elapsed, used) in [(0, 40.0), (120, 75.0)] {
            advance(elapsed);
            sample(
                if weekly { 10.0 } else { used },
                if weekly { used } else { 20.0 },
                0,
                T + 10_000,
            );
            SAMPLE.with(|s| {
                let mut s = s.borrow_mut();
                if weekly {
                    s.windows.week.as_mut().unwrap().resets_at = None;
                } else {
                    s.windows.session.as_mut().unwrap().resets_at = None;
                }
            });
            assert!(
                !is_swap(verdict(&mut c)),
                "without reset evidence, the samples cannot establish a common usage window (weekly={weekly})"
            );
        }
    }
}

#[test]
fn insufficient_history_is_explained_on_an_otherwise_successful_cycle() {
    let e = fixture();
    let mut c = context(&e);
    sample(20.0, 20.0, 0, T + 10_000);
    assert_ne!(drive(&e, &mut c).unwrap(), CycleOutcome::Swapped);
    let lines = log(&e);
    assert!(
        lines.lines().any(|l| l.contains("projection")
            && (l.contains("insufficient") || l.contains("history") || l.contains("samples"))),
        "explain why no projection is available: {lines}"
    );
    assert_eq!(ino_at(&e.live()), Some(e.l));
    assert!(!e.rs.dir.join("swap.json").exists());
}

#[test]
fn predictive_swap_keeps_measured_usage_and_records_projected_reason() {
    let e = fixture();
    let mut c = context(&e);
    c.settings.session_at = 99;
    sample(40.0, 20.0, 0, T + 10_000);
    assert_ne!(drive(&e, &mut c).unwrap(), CycleOutcome::Swapped);
    advance(120);
    sample(75.0, 20.0, 0, T + 10_000);
    assert_eq!(
        drive(&e, &mut c).unwrap(),
        CycleOutcome::Swapped,
        "{}",
        log(&e)
    );
    assert!(
        log(&e).contains("reason=projected"),
        "explain the predictive decision: {}",
        log(&e)
    );
    assert!(
        log(&e).contains("75%"),
        "logs preserve the actual observation: {}",
        log(&e)
    );
    let event = json(&e.rs.dir.join("swap.json"));
    assert_eq!(
        event["reading"]["session"], 5,
        "event describes measured incoming capacity"
    );
    assert_eq!(event["reading"]["measured_at"], T + 120);
}

#[test]
fn simultaneous_forecasts_preserve_weekly_exhaustion_through_publication() {
    let e = fixture();
    let mut c = context(&e);
    c.measure = Some(|ctx, snap| {
        decide_measured(
            ctx,
            snap,
            SAMPLE.with(|s| s.borrow().clone()),
            probes_for(snap, &[("spare", ok(5.0, 88.0)), ("extra", ok(5.0, 90.0))]),
            clock(),
        )
    });
    sample(40.0, 40.0, 0, T + 10_000);
    assert_ne!(drive(&e, &mut c).unwrap(), CycleOutcome::Swapped);
    advance(120);
    sample(65.0, 75.0, 0, T + 10_000);
    assert_eq!(
        drive(&e, &mut c).unwrap(),
        CycleOutcome::Swapped,
        "{}",
        log(&e)
    );
    assert_eq!(ino_at(&e.live()), Some(e.t));
    assert_eq!(ino_at(&e.slot("work")), Some(e.l));
    let event = json(&e.rs.dir.join("swap.json"));
    assert_eq!(event["live_login"], "spare");
    assert_eq!(event["reading"]["session"], 5);
    assert_eq!(event["reading"]["week"], 88);
    assert_eq!(event["reading"]["measured_at"], T + 120);
    assert!(log(&e).contains("reason=projected"), "{}", log(&e));
    assert!(log(&e).contains("75%"), "{}", log(&e));
}

#[test]
fn independent_forecasts_use_measured_values_only_for_unavailable_windows() {
    for unavailable in [None, Some("session"), Some("week")] {
        let e = fixture();
        let mut c = context(&e);
        for (elapsed, session, week) in [(0, 40.0, 40.0), (120, 65.0, 75.0)] {
            advance(elapsed);
            sample(session, week, 0, T + 10_000);
            SAMPLE.with(|s| {
                let mut p = s.borrow_mut();
                match unavailable {
                    Some("session") => p.windows.session.as_mut().unwrap().resets_at = None,
                    Some("week") => p.windows.week.as_mut().unwrap().resets_at = None,
                    _ => {}
                }
            });
            let state = State::new(c.store.root()).unwrap();
            let snap = observe::snapshot(&c, &state).unwrap();
            let measured = measure(&c, &snap).unwrap();
            let decided = fresh::live::step(&mut c, &snap, measured).unwrap();
            assert_eq!(
                decided.live,
                super::super::super::decide::Reading::Measured { session, week },
                "decision outlook must not replace observed history"
            );
            if elapsed == 0 {
                assert!(c.live_outlook.is_none());
                continue;
            }
            assert!(is_swap(decided.decision), "unavailable={unavailable:?}");
            let outlook = c.live_outlook.as_ref().unwrap();
            assert_eq!(
                (outlook.session, outlook.week),
                (
                    if unavailable == Some("session") {
                        65.0
                    } else {
                        90.0
                    },
                    if unavailable == Some("week") {
                        75.0
                    } else {
                        100.0
                    },
                ),
                "unavailable={unavailable:?}"
            );
        }
    }
}
