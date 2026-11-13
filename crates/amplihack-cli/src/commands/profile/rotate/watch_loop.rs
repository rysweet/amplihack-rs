//! Register before reading; reconcile queued changes before announcing
//! readiness; then block on filesystem, completion and shutdown readiness.
use super::coordinator::{Coordinator, Effect, InputState, ProbeJob, ProbeResult};
use super::watch_source::{Event, Events};
use crate::commands::profile::ProfileError;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReadPass {
    Startup,
    Changed,
    Overflow,
    /// Already-published records from the authoritative completion queue.
    Completed,
}
pub(super) struct Deps<'a> {
    pub events: &'a mut dyn Events,
    pub directories: &'a [PathBuf],
    pub snapshot: &'a mut dyn FnMut(ReadPass) -> Result<InputState, ProfileError>,
    pub coordinator: &'a mut Coordinator,
    pub execute: &'a mut dyn FnMut(&Effect) -> Result<(), ProfileError>,
    pub completions: &'a mut dyn FnMut() -> Vec<(ProbeJob, ProbeResult)>,
    pub ready: &'a mut dyn FnMut() -> Result<(), ProfileError>,
    pub now: &'a dyn Fn() -> i64,
}
fn apply(deps: &mut Deps<'_>, effects: Vec<Effect>) -> Result<(), ProfileError> {
    for effect in effects {
        (deps.execute)(&effect)?;
    }
    Ok(())
}
pub(super) fn run(mut deps: Deps<'_>) -> Result<(), ProfileError> {
    deps.events.watch(deps.directories)?;
    let mut state = (deps.snapshot)(ReadPass::Startup)?;
    subscribe(&mut deps, &mut state)?;
    deps.coordinator.baseline(&state, (deps.now)())?;
    // Establish all registrations and reconcile setup races as baseline.
    let mut queued = deps.events.drain()?;
    while queued
        .iter()
        .any(|e| matches!(e, Event::Changed | Event::Overflow))
    {
        let pass = if queued.contains(&Event::Overflow) {
            ReadPass::Overflow
        } else {
            ReadPass::Changed
        };
        state = (deps.snapshot)(pass)?;
        subscribe(&mut deps, &mut state)?;
        deps.coordinator.baseline(&state, (deps.now)())?;
        if queued.contains(&Event::Stop) {
            return Ok(());
        }
        queued = deps.events.drain()?;
    }
    (deps.ready)()?;
    loop {
        if queued.is_empty() {
            queued = deps.events.wait()?;
        }
        let changed = queued
            .iter()
            .any(|e| matches!(e, Event::Changed | Event::Overflow));
        if changed {
            let pass = if queued.contains(&Event::Overflow) {
                ReadPass::Overflow
            } else {
                ReadPass::Changed
            };
            state = (deps.snapshot)(pass)?;
            subscribe(&mut deps, &mut state)?;
        }
        // Completion publications are consumed before ordinary reconciliation,
        // so their own atomic replacement cannot create another decision.
        if changed || queued.contains(&Event::Completion) {
            let completions = (deps.completions)();
            if completions
                .iter()
                .any(|(_, r)| *r == ProbeResult::Published)
            {
                state = (deps.snapshot)(ReadPass::Completed)?;
            }
            for (job, result) in completions {
                let effects = deps
                    .coordinator
                    .complete(&job, result, &state, (deps.now)())?;
                apply(&mut deps, effects)?;
            }
        }
        if changed {
            let effects = deps.coordinator.reconcile(&state, (deps.now)())?;
            apply(&mut deps, effects)?;
        }
        let effects = deps.coordinator.ready(&state, (deps.now)())?;
        apply(&mut deps, effects)?;
        if queued.contains(&Event::Stop) {
            break;
        }
        queued = deps.events.drain()?;
    }
    Ok(())
}
/// Always reread after registration: a previously missing directory can
/// acquire a subscription without changing the requested pathname set.
fn subscribe(deps: &mut Deps<'_>, state: &mut InputState) -> Result<(), ProfileError> {
    loop {
        let registered = directories(deps.directories, state);
        deps.events.watch(&registered)?;
        if registered.is_empty() {
            return Ok(());
        }
        *state = (deps.snapshot)(ReadPass::Changed)?;
        if registered == directories(deps.directories, state) {
            return Ok(());
        }
    }
}
fn directories(initial: &[PathBuf], state: &InputState) -> Vec<PathBuf> {
    let mut dirs = initial.to_vec();
    dirs.extend(state.directories.iter().cloned());
    dirs.sort();
    dirs.dedup();
    dirs
}
