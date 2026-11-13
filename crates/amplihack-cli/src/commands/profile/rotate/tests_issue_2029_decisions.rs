//! #2029 executable contract for the production decision coordinator.
//!
//! baseline establishes semantic inputs without starting a decision.
//! reconcile accepts an externally triggered snapshot; ready only drains
//! already-ready work. complete continues the job's owning decision. All
//! return Effects; the daemon executes Probe and Revalidate effects through
//! its existing unlocked measurement and locked swap transactions.
//!
//! Input types below are non-secret values, not paths or credential data.
//! Reporting metadata is deliberately present to prove it is ignored.

use super::TuneFlags;
use super::coordinator::{
    Coordinator, Effect, EvidenceInput, InputState, LoginInput, ProbeJob, ProbeResult,
};
use super::settings::resolve;
use std::cell::Cell;

const NOW: i64 = 1_790_000_000;

#[path = "tests_issue_2029_completion.rs"]
mod completion;

#[path = "tests_issue_2029_semantics.rs"]
mod semantics;

fn login(name: &str, generation: &str, live: bool) -> LoginInput {
    LoginInput {
        profile: name.into(),
        account_id: format!("account-{name}"),
        generation: generation.into(),
        eligible: true,
        organisation: Some("org-test".into()),
        live,
    }
}

fn evidence(name: &str, generation: &str, ticket: u64, session: f64) -> EvidenceInput {
    EvidenceInput {
        profile: name.into(),
        account_id: format!("account-{name}"),
        generation: generation.into(),
        observation_id: format!("{name}-{generation}-{ticket}"),
        ticket,
        captured_at: NOW,
        session_window: amplihack_utils::usage_observation::SessionWindow::Present,
        session_reset_at: None,
        week_reset_at: None,
        session_used: Some(session),
        week_used: Some(30.0),
        limit_reached: false,
    }
}

fn inputs() -> InputState {
    InputState {
        logins: vec![login("work", "g1", true), login("spare", "g2", false)],
        observations: vec![
            evidence("work", "g1", 1, 20.0),
            evidence("spare", "g2", 1, 10.0),
        ],
        enabled: true,
        report_revision: 0,
        ..InputState::default()
    }
}

fn machine(state: &InputState, interval: &str) -> Coordinator {
    let settings = resolve(&TuneFlags::default(), Some(interval), &|_| None).unwrap();
    let mut c = Coordinator::new(settings);
    assert!(
        c.baseline(state, NOW).unwrap().is_empty(),
        "startup must not start decisions or probes"
    );
    c
}

fn probes(effects: &[Effect]) -> Vec<ProbeJob> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Probe(job) => Some(job.clone()),
            _ => None,
        })
        .collect()
}

fn decisions(effects: &[Effect]) -> usize {
    effects
        .iter()
        .filter(|e| matches!(e, Effect::Decision { .. }))
        .count()
}

fn revalidations(effects: &[Effect]) -> usize {
    effects
        .iter()
        .filter(|e| matches!(e, Effect::Revalidate { .. }))
        .count()
}

#[test]
fn startup_is_a_baseline_even_when_every_observation_is_missing() {
    let mut state = inputs();
    state.observations.clear();
    let mut c = machine(&state, "1");
    assert!(c.ready(&state, NOW).unwrap().is_empty());
}

#[test]
fn fake_clock_advancement_after_startup_never_schedules_any_work() {
    for interval in ["1", "60", "86400"] {
        let state = inputs();
        let mut c = machine(&state, interval);
        let clock = Cell::new(NOW);
        for advance in [1, 3600, 86_400, 604_800, 31_536_000] {
            clock.set(clock.get() + advance);
            assert!(
                c.ready(&state, clock.get()).unwrap().is_empty(),
                "elapsed time created work with deprecated interval {interval}"
            );
        }
        // Even a duplicate kernel notification after a year cannot turn
        // observation age into an independent decision.
        assert!(c.reconcile(&state, clock.get()).unwrap().is_empty());
    }
}

#[test]
fn crossing_either_configured_threshold_starts_exactly_one_logical_decision() {
    for (session, week) in [(85.0, 30.0), (20.0, 90.0)] {
        let mut state = inputs();
        let mut c = machine(&state, "60");
        let mut update = evidence("work", "g1", 2, session);
        update.week_used = Some(week);
        state.observations[0] = update;
        let effects = c.reconcile(&state, NOW).unwrap();
        assert_eq!(decisions(&effects), 1);
        assert!(
            probes(&effects).is_empty(),
            "current-generation evidence already exists"
        );
        assert_eq!(revalidations(&effects), 1);
        for _ in 0..3 {
            assert!(
                c.reconcile(&state, NOW).unwrap().is_empty(),
                "duplicate notification started another decision"
            );
        }
    }
}

#[test]
fn output_writes_and_ordering_metadata_do_not_trigger_policy_decisions() {
    let mut state = inputs();
    let mut c = machine(&state, "1");
    state.report_revision += 1;
    assert!(c.reconcile(&state, NOW).unwrap().is_empty());
    // A new accepted identifier/ticket containing identical measurement
    // evidence must not manufacture a threshold crossing or confirmation.
    state.observations[0] = evidence("work", "g1", 2, 20.0);
    assert!(c.reconcile(&state, NOW).unwrap().is_empty());
}

#[test]
fn a_new_eligible_login_probes_only_that_generation_once() {
    let mut state = inputs();
    let mut c = machine(&state, "60");
    state.logins.push(login("new", "g3", false));
    let effects = c.reconcile(&state, NOW).unwrap();
    let jobs = probes(&effects);
    assert_eq!(jobs.len(), 1);
    assert_eq!((&*jobs[0].profile, &*jobs[0].generation), ("new", "g3"));
    assert_eq!(decisions(&effects), 1);
    assert!(c.reconcile(&state, NOW).unwrap().is_empty());
}

#[test]
fn adding_a_login_does_not_probe_an_unrelated_unobserved_login() {
    let mut state = inputs();
    state.observations.pop();
    let mut c = machine(&state, "60");
    state.logins.push(login("new", "g3", false));
    let jobs = probes(&c.reconcile(&state, NOW).unwrap());
    assert_eq!(
        jobs.len(),
        1,
        "login addition authorised an unrelated probe"
    );
    assert_eq!(jobs[0].profile, "new");
    assert!(c.reconcile(&state, NOW).unwrap().is_empty());
}

#[test]
fn ineligible_addition_is_not_probed_until_it_becomes_eligible() {
    let mut state = inputs();
    let mut c = machine(&state, "60");
    let mut added = login("new", "g3", false);
    added.eligible = false;
    state.logins.push(added);
    assert!(probes(&c.reconcile(&state, NOW).unwrap()).is_empty());
    state.logins.last_mut().unwrap().eligible = true;
    let jobs = probes(&c.reconcile(&state, NOW).unwrap());
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].profile, "new");
}

#[test]
fn missing_candidate_evidence_is_requested_only_when_a_decision_needs_it() {
    let mut state = inputs();
    state.observations.pop();
    let mut c = machine(&state, "60");
    state.report_revision += 1;
    assert!(probes(&c.reconcile(&state, NOW).unwrap()).is_empty());
    state.observations[0] = evidence("work", "g1", 2, 85.0);
    let effects = c.reconcile(&state, NOW).unwrap();
    let jobs = probes(&effects);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].profile, "spare");
    assert_eq!(
        revalidations(&effects),
        0,
        "must wait for necessary evidence"
    );
}

#[test]
fn failed_probe_blocks_without_retries_until_a_relevant_external_change() {
    let mut state = inputs();
    let mut c = machine(&state, "1");
    state.logins.push(login("new", "g3", false));
    let job = probes(&c.reconcile(&state, NOW).unwrap()).pop().unwrap();
    let effects = c.complete(&job, ProbeResult::Failed, &state, NOW).unwrap();
    assert!(probes(&effects).is_empty());
    assert_eq!(
        decisions(&effects),
        0,
        "completion continues the owning decision"
    );
    for later in [NOW + 60, NOW + 604_800, NOW + 31_536_000] {
        assert!(c.ready(&state, later).unwrap().is_empty());
        assert!(c.reconcile(&state, later).unwrap().is_empty());
    }
    state.report_revision += 1;
    assert!(c.reconcile(&state, NOW + 31_536_000).unwrap().is_empty());
    state.logins.last_mut().unwrap().generation = "g4".into();
    let next = probes(&c.reconcile(&state, NOW + 31_536_000).unwrap());
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].generation, "g4");
}

#[test]
fn successful_completion_continues_its_decision_and_ignores_its_own_write() {
    let mut state = inputs();
    state.observations.pop();
    let mut c = machine(&state, "60");
    state.observations[0] = evidence("work", "g1", 2, 95.0);
    let started = c.reconcile(&state, NOW).unwrap();
    let job = probes(&started).pop().unwrap();
    assert_eq!(decisions(&started), 1);
    // Production publishes before putting this completion in the queue.
    let mut own = evidence("spare", "g2", 2, 10.0);
    own.observation_id = job.observation_id();
    state.observations.push(own);
    let completed = c
        .complete(&job, ProbeResult::Published, &state, NOW)
        .unwrap();
    assert_eq!(decisions(&completed), 0);
    assert_eq!(revalidations(&completed), 1);
    assert!(
        c.complete(&job, ProbeResult::Published, &state, NOW)
            .unwrap()
            .is_empty()
    );
    assert!(c.reconcile(&state, NOW).unwrap().is_empty());
}

#[test]
fn superseded_retired_or_failed_publications_are_terminal_not_replacement_jobs() {
    for outcome in [
        ProbeResult::Superseded,
        ProbeResult::RetiredGeneration,
        ProbeResult::PublicationFailed,
    ] {
        let mut state = inputs();
        let mut c = machine(&state, "1");
        state.logins.push(login("new", "g3", false));
        let job = probes(&c.reconcile(&state, NOW).unwrap()).pop().unwrap();
        let effects = c.complete(&job, outcome, &state, NOW).unwrap();
        assert!(probes(&effects).is_empty());
        assert_eq!(revalidations(&effects), 0);
        assert!(c.ready(&state, NOW + 604_800).unwrap().is_empty());
    }
}

#[test]
fn concurrent_external_changes_survive_a_probe_completion_and_self_writes() {
    let mut state = inputs();
    let mut c = machine(&state, "60");
    state.logins.push(login("new", "g3", false));
    let job = probes(&c.reconcile(&state, NOW).unwrap()).pop().unwrap();
    state.observations[0] = evidence("work", "g1", 2, 95.0);
    let mut own = evidence("new", "g3", 1, 10.0);
    own.observation_id = job.observation_id();
    state.observations.push(own);
    state.report_revision += 1;
    let effects = c
        .complete(&job, ProbeResult::Published, &state, NOW)
        .unwrap();
    assert_eq!(
        decisions(&effects),
        1,
        "external threshold crossing was discarded with probe output"
    );
    assert_eq!(revalidations(&effects), 1);
    assert!(c.reconcile(&state, NOW).unwrap().is_empty());
}

#[test]
fn generation_change_or_removal_during_measurement_refuses_commit() {
    for remove in [false, true] {
        let mut state = inputs();
        state.observations.pop();
        let mut c = machine(&state, "60");
        state.observations[0] = evidence("work", "g1", 2, 95.0);
        let job = probes(&c.reconcile(&state, NOW).unwrap()).pop().unwrap();
        if remove {
            state.logins.pop();
        } else {
            state.logins[1].generation = "replacement".into();
        }
        state.observations.push(evidence("spare", "g2", 2, 10.0));
        let effects = c
            .complete(&job, ProbeResult::Published, &state, NOW)
            .unwrap();
        assert_eq!(
            revalidations(&effects),
            0,
            "stale completion attempted a swap"
        );
    }
}

#[test]
fn missing_or_different_organisation_identity_cannot_authorise_a_move() {
    for organisation in [None, Some("other-org".to_string())] {
        let mut state = inputs();
        let mut c = machine(&state, "60");
        state.logins[1].organisation = organisation;
        state.observations[0] = evidence("work", "g1", 2, 95.0);
        assert_eq!(revalidations(&c.reconcile(&state, NOW).unwrap()), 0);
    }
}

#[test]
fn actual_limit_evidence_can_start_a_decision_without_percentages() {
    let mut state = inputs();
    let mut c = machine(&state, "60");
    let mut limit = evidence("work", "g1", 2, 0.0);
    limit.session_used = None;
    limit.week_used = None;
    limit.limit_reached = true;
    state.observations[0] = limit;
    let effects = c.reconcile(&state, NOW).unwrap();
    assert_eq!(decisions(&effects), 1);
    assert_eq!(revalidations(&effects), 1);
}

#[test]
fn login_addition_alone_does_not_probe_other_missing_logins_even_when_live_is_over_threshold() {
    let mut state = inputs();
    state.observations[0] = evidence("work", "g1", 1, 95.0);
    state.observations.pop();
    let mut c = machine(&state, "60");
    state.logins.push(login("new", "g3", false));
    let jobs = probes(&c.reconcile(&state, NOW).unwrap());
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].profile, "new");
}

#[test]
fn a_live_probe_can_continue_the_same_decision_with_targeted_candidates() {
    let mut state = inputs();
    state.observations.clear();
    let mut c = machine(&state, "1");
    state.control = "changed policy".into();
    let started = c.reconcile(&state, NOW).unwrap();
    let jobs = probes(&started);
    assert_eq!(decisions(&started), 1);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].profile, "work");
    let live = &jobs[0];
    let mut own = evidence("work", "g1", 1, 95.0);
    own.observation_id = live.observation_id();
    state.observations.push(own);
    let continued = c
        .complete(live, ProbeResult::Published, &state, NOW)
        .unwrap();
    assert_eq!(decisions(&continued), 0);
    let candidates = probes(&continued);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].profile, "spare");
    assert_eq!(candidates[0].decision_id, live.decision_id);
    let mut own = evidence("spare", "g2", 1, 10.0);
    own.observation_id = candidates[0].observation_id();
    state.observations.push(own);
    let completed = c
        .complete(&candidates[0], ProbeResult::Published, &state, NOW)
        .unwrap();
    assert_eq!(revalidations(&completed), 1);
    assert_eq!(decisions(&completed), 0);
    assert!(probes(&completed).is_empty());
    assert!(c.reconcile(&state, NOW).unwrap().is_empty());
}

#[test]
fn a_pin_blocks_probes_and_releasing_it_does_not_probe_unneeded_candidates() {
    let mut state = inputs();
    state.held = true;
    let mut c = machine(&state, "1");
    state.logins.push(login("new", "g3", false));
    assert!(c.reconcile(&state, NOW).unwrap().is_empty());
    state.held = false;
    assert!(probes(&c.reconcile(&state, NOW).unwrap()).is_empty());
}
