//! #2029 integration contract for register/read/reconcile/readiness ordering.
//! Fake I/O supplies events and snapshots; policy stays in the production
//! Coordinator. An exhausted event script fails, exposing busy waiting.
//!
//! Deps deliberately has no pending deadline or interval callback. ReadPass
//! distinguishes baseline, targeted change and single-pass overflow reads.
use super::TuneFlags;
use super::coordinator::{Coordinator, Effect, InputState, ProbeJob, ProbeResult};
use super::settings::resolve;
use super::watch_loop::{self, Deps, ReadPass};
use super::watch_source::{Event, Events};
use crate::commands::profile::ProfileError;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;

#[path = "tests_issue_2029_startup.rs"]
mod startup;

type Trace = Rc<RefCell<Vec<String>>>;
type Step = Box<dyn FnOnce() -> Vec<Event>>;

struct Script {
    trace: Trace,
    steps: VecDeque<Step>,
    queued: Rc<RefCell<Vec<Event>>>,
}

impl Events for Script {
    fn watch(&mut self, _dirs: &[PathBuf]) -> Result<(), ProfileError> {
        self.trace.borrow_mut().push("registered".into());
        Ok(())
    }
    fn watched(&self) -> Vec<PathBuf> {
        Vec::new()
    }
    fn wait(&mut self) -> Result<Vec<Event>, ProfileError> {
        self.trace.borrow_mut().push("blocked".into());
        Ok(self.steps.pop_front().expect(
            "loop waited without a scripted event; possible recurring work",
        )())
    }
    fn drain(&mut self) -> Result<Vec<Event>, ProfileError> {
        Ok(std::mem::take(&mut *self.queued.borrow_mut()))
    }
}

#[derive(Default)]
struct Run {
    trace: Trace,
    effects: Vec<Effect>,
    reads: Vec<ReadPass>,
    completion_drains: usize,
}

fn drive(steps: Vec<Step>, during_baseline: bool) -> Run {
    drive_at(steps, during_baseline, Rc::new(Cell::new(1_790_000_000)))
}

fn drive_at(steps: Vec<Step>, during_baseline: bool, clock: Rc<Cell<i64>>) -> Run {
    let mut run = Run::default();
    let queued = Rc::new(RefCell::new(Vec::new()));
    let mut events = Script {
        trace: run.trace.clone(),
        steps: steps.into(),
        queued: queued.clone(),
    };
    let mut coordinator =
        Coordinator::new(resolve(&TuneFlags::default(), Some("1"), &|_| None).unwrap());
    let mut initial = true;
    let mut snapshot = |pass| {
        run.trace.borrow_mut().push(format!("read:{pass:?}"));
        run.reads.push(pass);
        if initial && during_baseline {
            queued.borrow_mut().push(Event::Changed);
        }
        initial = false;
        Ok(InputState {
            enabled: true,
            ..InputState::default()
        })
    };
    let mut execute = |effect: &Effect| {
        run.effects.push(effect.clone());
        Ok(())
    };
    let mut completions = || {
        run.completion_drains += 1;
        Vec::<(ProbeJob, ProbeResult)>::new()
    };
    let ready_trace = run.trace.clone();
    let mut ready = || {
        ready_trace.borrow_mut().push("ready".into());
        Ok(())
    };
    watch_loop::run(Deps {
        events: &mut events,
        directories: &[],
        snapshot: &mut snapshot,
        coordinator: &mut coordinator,
        execute: &mut execute,
        completions: &mut completions,
        ready: &mut ready,
        now: &|| clock.get(),
    })
    .unwrap();
    run
}

#[test]
fn watches_are_registered_before_baseline_and_readiness_follows_reconciliation() {
    let run = drive(vec![Box::new(|| vec![Event::Stop])], true);
    let trace = run.trace.borrow();
    assert_eq!(trace.first().map(String::as_str), Some("registered"));
    let ready = trace.iter().position(|s| s == "ready").unwrap();
    assert!(trace[..ready].iter().any(|s| s == "read:Startup"));
    assert!(
        trace[..ready].iter().any(|s| s == "read:Changed"),
        "setup event was lost: {trace:?}"
    );
    assert!(
        run.effects.is_empty(),
        "startup reconciliation must not cause a probe sweep"
    );
}

#[test]
fn overflow_reads_every_observation_once_then_returns_to_blocking() {
    let run = drive(
        vec![
            Box::new(|| vec![Event::Overflow, Event::Overflow]),
            Box::new(|| vec![Event::Stop]),
        ],
        false,
    );
    assert_eq!(
        run.reads
            .iter()
            .filter(|p| **p == ReadPass::Startup)
            .count(),
        1
    );
    assert_eq!(
        run.reads
            .iter()
            .filter(|p| **p == ReadPass::Overflow)
            .count(),
        1
    );
    assert_eq!(
        run.trace
            .borrow()
            .iter()
            .filter(|s| *s == "blocked")
            .count(),
        2
    );
    assert!(
        run.effects.is_empty(),
        "overflow is reconciliation, not speculative probing"
    );
}

#[test]
fn completion_wakes_drain_the_authoritative_queue_without_an_input_scan() {
    let run = drive(
        vec![
            Box::new(|| vec![Event::Completion]),
            Box::new(|| vec![Event::Stop]),
        ],
        false,
    );
    assert!(run.completion_drains >= 1);
    assert_eq!(run.reads, vec![ReadPass::Startup]);
    assert!(run.effects.is_empty());
}

#[test]
fn advancing_a_fake_clock_while_the_source_blocks_cannot_run_a_decision() {
    let clock = Rc::new(Cell::new(1_790_000_000_i64));
    let moved = clock.clone();
    let run = drive_at(
        vec![Box::new(move || {
            moved.set(moved.get() + 31_536_000);
            vec![Event::Stop]
        })],
        false,
        clock.clone(),
    );
    assert_eq!(clock.get(), 1_821_536_000);
    assert!(run.effects.is_empty());
    assert_eq!(run.reads, vec![ReadPass::Startup]);
}
