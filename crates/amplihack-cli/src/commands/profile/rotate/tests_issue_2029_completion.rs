//! Worker-side integration: publish, enqueue the terminal outcome, then wake.
//! The production helper catches measurement failures and always
//! resolves the owning job. Wake bytes are never the completion data.
use super::*;
use crate::commands::profile::rotate::inflight::run_observed_job;
use std::cell::RefCell;

fn new_job() -> ProbeJob {
    let mut state = inputs();
    let mut coordinator = machine(&state, "60");
    state.logins.push(login("new", "g3", false));
    probes(&coordinator.reconcile(&state, NOW).unwrap())
        .pop()
        .unwrap()
}

#[test]
fn publication_finishes_before_enqueue_and_completion_readiness() {
    let trace = RefCell::new(Vec::new());
    let queued = RefCell::new(Vec::new());
    let job = new_job();
    run_observed_job(
        job.clone(),
        || {
            trace.borrow_mut().push("measure");
            Ok(evidence("new", "g3", 1, 10.0))
        },
        |captured, _evidence| {
            assert_eq!(captured, &job, "worker relabelled the captured generation");
            trace.borrow_mut().push("published");
            ProbeResult::Published
        },
        |job, result| {
            trace.borrow_mut().push("queued");
            queued.borrow_mut().push((job, result));
        },
        || {
            assert_eq!(
                queued.borrow().len(),
                1,
                "wake preceded authoritative enqueue"
            );
            trace.borrow_mut().push("wake");
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(
        *trace.borrow(),
        vec!["measure", "published", "queued", "wake"]
    );
}

#[test]
fn failed_publication_enqueues_a_terminal_failure_instead_of_usable_evidence() {
    for result in [
        ProbeResult::PublicationFailed,
        ProbeResult::Superseded,
        ProbeResult::RetiredGeneration,
    ] {
        let queued = RefCell::new(Vec::new());
        let wakes = Cell::new(0);
        run_observed_job(
            new_job(),
            || Ok(evidence("new", "g3", 1, 10.0)),
            |_, _| result.clone(),
            |job, result| queued.borrow_mut().push((job, result)),
            || {
                wakes.set(wakes.get() + 1);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(queued.borrow()[0].1, result);
        assert_eq!(
            wakes.get(),
            1,
            "failed publication left its decision waiting"
        );
    }
}

#[test]
fn failed_measurement_never_publishes_synthetic_usage() {
    let queued = RefCell::new(Vec::new());
    run_observed_job(
        new_job(),
        || Err(ProbeResult::Failed),
        |_, _| panic!("a failed measurement reached publication"),
        |job, result| queued.borrow_mut().push((job, result)),
        || Ok(()),
    )
    .unwrap();
    assert_eq!(queued.borrow()[0].1, ProbeResult::Failed);
}

#[test]
fn a_panicking_measurement_still_resolves_the_job_once() {
    let queued = RefCell::new(Vec::new());
    run_observed_job(
        new_job(),
        || panic!("injected worker failure"),
        |_, _| panic!("panicking measurement reached publication"),
        |job, result| queued.borrow_mut().push((job, result)),
        || Ok(()),
    )
    .unwrap();
    assert_eq!(queued.borrow().len(), 1);
    assert_eq!(queued.borrow()[0].1, ProbeResult::Failed);
}

#[test]
fn coalescing_wake_signals_does_not_drop_authoritative_completions() {
    let queued = RefCell::new(Vec::new());
    let readable = Cell::new(false);
    let mut state = inputs();
    let mut c = machine(&state, "60");
    state
        .logins
        .extend([login("new", "g3", false), login("another", "g4", false)]);
    let jobs = probes(&c.reconcile(&state, NOW).unwrap());
    assert_eq!(jobs.len(), 2);
    for job in jobs {
        run_observed_job(
            job,
            || Err(ProbeResult::Failed),
            |_, _| panic!("unexpected publication"),
            |job, result| queued.borrow_mut().push((job, result)),
            || {
                readable.set(true);
                Ok(())
            },
        )
        .unwrap();
    }
    assert!(readable.get());
    assert_eq!(
        queued.borrow().len(),
        2,
        "one coalesced wake must expose both queued results"
    );
    for (job, result) in queued.into_inner() {
        let effects = c.complete(&job, result, &state, NOW).unwrap();
        assert!(probes(&effects).is_empty());
    }
    assert!(c.ready(&state, NOW + 604_800).unwrap().is_empty());
}

#[test]
fn terminal_probe_completion_cannot_consume_an_independent_publication_for_the_same_login() {
    for result in [
        ProbeResult::Superseded,
        ProbeResult::Failed,
        ProbeResult::PublicationFailed,
    ] {
        let mut state = inputs();
        state.observations.pop();
        let mut c = machine(&state, "60");
        state.observations[0] = evidence("work", "g1", 2, 95.0);
        let job = probes(&c.reconcile(&state, NOW).unwrap()).pop().unwrap();
        // A real session publishes while this probe is collecting. Its
        // identifier is independent of the terminal probe's capture.
        let external = evidence("spare", "g2", 3, 90.0);
        assert_ne!(external.observation_id, job.observation_id());
        state.observations.push(external);
        let effects = c.complete(&job, result.clone(), &state, NOW).unwrap();
        assert_eq!(
            decisions(&effects),
            1,
            "{result:?} discarded an external publication as though it were the probe's own write"
        );
        assert!(
            probes(&effects).is_empty(),
            "valid current-generation evidence exists"
        );
        assert!(c.reconcile(&state, NOW).unwrap().is_empty());
    }
}
