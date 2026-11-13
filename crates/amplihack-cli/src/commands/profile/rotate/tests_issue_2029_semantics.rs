//! Exercise reset semantics through the actual record-to-input conversion.
//! The assertions concern effects, so silently dropping a reset cannot pass.
use super::*;
use amplihack_utils::usage_observation::{Binding, Capture, Evidence, Record};

fn reading(ticket: u64, session_reset_at: Option<i64>, week_reset_at: Option<i64>) -> Record {
    Record {
        schema_version: 1,
        capture: Capture {
            binding: Binding {
                profile: "work".into(),
                account_id: "account-work".into(),
                generation: "g1".into(),
            },
            observation_id: format!("work-g1-{ticket}"),
            ticket,
            captured_at: NOW,
        },
        evidence: Evidence {
            session_window: amplihack_utils::usage_observation::SessionWindow::Present,
            session_used: Some(85.0),
            week_used: Some(30.0),
            session_reset_at,
            week_reset_at,
            limit_reached: false,
        },
        revision: ticket,
    }
}

fn replace_reading(state: &mut InputState, record: Record) {
    state.observations[0] = super::super::inputs::evidence_input(&record);
    state.records = vec![record];
}

fn reset_changes(session: bool) {
    for (old, new) in [
        (None, Some(NOW + 1)),
        (Some(NOW + 1), None),
        (Some(NOW + 1), Some(NOW + 2)),
    ] {
        let mut state = inputs();
        let resets = |value| {
            if session {
                (value, None)
            } else {
                (None, value)
            }
        };
        let (s, w) = resets(old);
        replace_reading(&mut state, reading(1, s, w));
        let mut c = machine(&state, "60");
        let (s, w) = resets(new);
        replace_reading(&mut state, reading(2, s, w));
        let effects = c.reconcile(&state, NOW).unwrap();
        assert_eq!(
            decisions(&effects),
            1,
            "reset-only update {old:?} -> {new:?} was discarded"
        );
        assert!(probes(&effects).is_empty(), "valid evidence already exists");
        assert!(
            c.reconcile(&state, NOW).unwrap().is_empty(),
            "duplicate reset notification repeated a decision"
        );
    }
}

#[test]
fn session_reset_only_publications_reconcile_exactly_once() {
    reset_changes(true);
}

#[test]
fn weekly_reset_only_publications_reconcile_exactly_once() {
    reset_changes(false);
}

#[test]
fn crossing_a_known_reset_time_without_a_publication_never_schedules_work() {
    let mut state = inputs();
    replace_reading(&mut state, reading(1, Some(NOW + 1), Some(NOW + 2)));
    let mut c = machine(&state, "1");
    for later in [NOW + 1, NOW + 2, NOW + 31_536_000] {
        assert!(c.ready(&state, later).unwrap().is_empty());
        assert!(c.reconcile(&state, later).unwrap().is_empty());
    }
}

#[test]
fn reset_preserving_capture_and_revision_changes_do_not_create_a_decision() {
    let mut state = inputs();
    replace_reading(&mut state, reading(1, Some(NOW + 1), Some(NOW + 2)));
    let mut c = machine(&state, "60");
    let mut next = reading(2, Some(NOW + 1), Some(NOW + 2));
    next.capture.captured_at += 1;
    replace_reading(&mut state, next);
    assert!(c.reconcile(&state, NOW + 1).unwrap().is_empty());
}

#[test]
fn paused_limit_alert_uses_the_observation_instead_of_clearing_an_elapsed_reset() {
    let mut state = inputs();
    state.enabled = false;
    replace_reading(&mut state, reading(1, Some(NOW - 1), Some(NOW - 1)));
    state.observations[0].limit_reached = true;
    let line = super::super::exhausted::observation_line(&state, 85).unwrap();
    assert!(line.contains("usage limit reached"));
    assert_eq!(
        super::super::exhausted::observation_line(&state, 85),
        Some(line)
    );
    state.logins[0].generation = "replacement".into();
    assert!(super::super::exhausted::observation_line(&state, 85).is_none());
}
