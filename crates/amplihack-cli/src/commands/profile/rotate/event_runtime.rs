//! Executes coordinator effects through the existing measurement and locked
//! swap transaction. Worker results are published before completion readiness.
#[path = "probe_eligibility.rs"]
mod eligibility;
use super::coordinator::{Coordinator, Effect, ProbeJob, ProbeResult};
use super::cycle::{self, Ctx, Mode};
use super::watch_loop::{self, Deps, ReadPass};
use super::watch_source::Events;
use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::usage_source::SourceEnv;
use crate::commands::profile::{ProfileError, name::ProfileName};
use amplihack_utils::usage_observation::{Binding, ObservationStore, PublishOutcome};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};

pub(super) fn run(ctx: Ctx, mut events: Box<dyn Events>) -> Result<(), ProfileError> {
    let directories = super::watch_source::watch_dirs(ctx.store.root(), &ctx.home, &[]);
    let wake = events.wake_handle();
    let mut signal_ids = Vec::new();
    for signal in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        let handle = wake.clone();
        // SAFETY: callback only performs atomic operations and write(2).
        signal_ids.push(unsafe {
            signal_hook::low_level::register(signal, move || {
                crate::commands::profile::usage_cmd::PROBE_CANCEL
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                let _ = handle.stop();
            })
        }?);
    }
    let mut coordinator = Coordinator::new(ctx.settings);
    let runtime = Rc::new(RefCell::new(ctx));
    let mut inputs = super::inputs::Inputs::default();
    type Work = (
        ProbeJob,
        amplihack_utils::usage_observation::Capture,
        std::path::PathBuf,
        ProfileName,
        bool,
        amplihack_utils::usage_observation::LoginLocation,
        Arc<crate::commands::profile::usage_cmd::RowReader>,
    );
    let (tx, rx) = channel::<Work>();
    let completed = Arc::new(Mutex::new(Vec::<(ProbeJob, ProbeResult)>::new()));
    // Gate bounds Claude subprocesses. The same reader serves all jobs;
    // each has its original generation-bound capture before it is queued.
    let mode = super::measure::mode();
    let timeout = super::measure::timeout();
    let (bound, warning) = crate::commands::profile::probe_limits::parallel_from_host(4096);
    if let Some(w) = warning {
        eprintln!("{w}");
    }

    let (direct_bound, warning) =
        crate::commands::profile::probe_limits::direct_parallel_from_env(4096);
    if let Some(warning) = warning {
        eprintln!("{warning}");
    }
    let bound = bound.min(direct_bound);

    let snapshot_ctx = runtime.clone();
    let mut last_off_alert = None;
    let mut was_enabled = None;
    let mut snapshot = |pass: ReadPass| {
        let ctx = snapshot_ctx.borrow();
        let value = inputs.read(&ctx.store, &ctx.home, pass)?;
        let alert = if value.enabled {
            None
        } else {
            let threshold =
                crate::commands::profile::auto_switch::config::effective(ctx.store.root())
                    .config
                    .threshold;
            super::exhausted::observation_line(&value, threshold)
        };
        if was_enabled == Some(true) && !value.enabled {
            super::daemon::log_line(&ctx.rs, "idle: auto-switch is off");
        }
        was_enabled = Some(value.enabled);
        if alert != last_off_alert {
            if let Some(line) = &alert {
                super::daemon::log_line(&ctx.rs, line);
            }
            last_off_alert = alert;
        }
        Ok(value)
    };
    let execute_ctx = runtime.clone();
    let execute_wake = wake.clone();
    let execute_completed = completed.clone();
    let mut execute = |effect: &Effect| -> Result<(), ProfileError> {
        match effect {
            Effect::Decision { id } => {
                let ctx = execute_ctx.borrow();
                super::daemon::log_line(&ctx.rs, &format!("watch: decision {id}"));
            }
            Effect::Probe(job) => {
                let ctx = execute_ctx.borrow();
                let store = ObservationStore::open(ctx.store.root())?;
                let capture = store.lock()?.capture(
                    &Binding {
                        profile: job.profile.clone(),
                        account_id: job.account_id.clone(),
                        generation: job.generation.clone(),
                    },
                    &job.observation_id(),
                    now(),
                )?;
                let mut env = SourceEnv::from_env(mode, timeout);
                env.passive_max_age = 0;
                let reader = Arc::new(super::measure::reader(&ctx, env, bound, now()));
                let root = ctx.store.root().to_path_buf();
                let profile = ProfileName::parse(&job.profile)?;
                let live =
                    crate::commands::profile::default_live::live_name(&ctx.store, Some(&ctx.home))
                        .is_some_and(|name| name == profile);
                let location_path = if live {
                    ctx.home.join(".claude/.credentials.json")
                } else {
                    root.join(profile.as_str()).join(".credentials.json")
                };
                let Some(location) = store
                    .lock()?
                    .probe_location(&capture.binding, &location_path)?
                else {
                    execute_completed
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push((job.clone(), ProbeResult::RetiredGeneration));
                    execute_wake.completion()?;
                    return Ok(());
                };
                tx.send((job.clone(), capture, root, profile, live, location, reader))
                    .map_err(|_| {
                        ProfileError::RotateDaemon("observation work queue closed".into())
                    })?;
            }
            Effect::Revalidate { id, inputs } => {
                let mut ctx = execute_ctx.borrow_mut();
                ctx.event_inputs = Some(inputs.clone());
                match cycle::run_cycle(&mut ctx, Mode::Watch) {
                    Ok(outcome) => {
                        super::hook_heal::after_cycle(outcome, &ctx);
                        super::daemon::log_line(
                            &ctx.rs,
                            &format!("watch: decision {id} finished: {}", outcome.as_str()),
                        );
                    }
                    Err(e) => {
                        super::daemon::log_line(&ctx.rs, &format!("skip: {e}"));
                        super::daemon::log_line(
                            &ctx.rs,
                            &format!("watch: decision {id} finished: failed"),
                        );
                    }
                }
            }
        }
        Ok(())
    };
    // A fixed, bounded worker set, blocked on channel readiness between
    // jobs. No retry clock or scheduling deadline exists here.
    let rx = Arc::new(Mutex::new(rx));
    let mut workers = Vec::new();
    for _ in 0..bound.max(1) {
        let rx = rx.clone();
        let wake = wake.clone();
        let completed = execute_completed.clone();
        workers.push(std::thread::spawn(move || {
            loop {
                let next = rx
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .recv();
                let Ok((job, capture, root, profile, live, location, reader)) = next else {
                    break;
                };
                let capture_time = capture.captured_at;
                let captured_job = job.clone();
                let measured_evidence = RefCell::new(None);
                let result = super::inflight::run_observed_job(
                    job,
                    || {
                        let current = (|| -> Result<_, ProfileError> {
                            let store = ObservationStore::open(&root)?;
                            let mut guard = store.lock()?;
                            eligibility::before_collection(&root, &profile, &mut guard, &capture.binding, &location)
                        })();
                        match current {
                            Ok(None) => (),
                            Ok(Some(result)) => return Err(result),
                            Err(e) => {
                                tracing::warn!("probe ownership check failed: {e}");
                                return Err(ProbeResult::Failed);
                            }
                        }
                        let probe = reader.profile_unobserved(&profile, live);
                        if probe.error.is_some() {
                            return Err(ProbeResult::Failed);
                        }
                        let report = probe.report;
                        if !matches!(report.state, crate::commands::profile::usage_parse::UsageState::Ok | crate::commands::profile::usage_parse::UsageState::SessionExhausted | crate::commands::profile::usage_parse::UsageState::WeeklyExhausted) { return Err(ProbeResult::Failed); }
                        let evidence = report.observation;
                        *measured_evidence.borrow_mut() = Some(evidence.clone());
                        Ok(super::coordinator::EvidenceInput {
                            profile: captured_job.profile.clone(), account_id: captured_job.account_id.clone(), generation: captured_job.generation.clone(), observation_id: capture.observation_id.clone(), ticket: capture.ticket, captured_at: capture_time,
                            session_window: evidence.session_window,
                            session_reset_at: evidence.session_reset_at,
                            week_reset_at: evidence.week_reset_at,
                            session_used: evidence.session_used, week_used: evidence.week_used,
                            limit_reached: evidence.limit_reached,
                        })
                    },
                    |_, _e| {
                        let published = (|| -> std::io::Result<_> {
                            let store = ObservationStore::open(&root)?;
                            let evidence = measured_evidence.borrow().clone().ok_or_else(|| {
                                std::io::Error::other("measurement evidence is missing")
                            })?;
                            let mut guard = store.lock()?;
                            if !eligibility::allowed(&root, &profile).map_err(std::io::Error::other)? {
                                return Ok(PublishOutcome::Superseded);
                            }
                            if !guard.probe_location_unchanged(&capture.binding, &location)? {
                                return Ok(PublishOutcome::RetiredGeneration);
                            }
                            let outcome = guard.publish(&capture, &evidence)?;
                            Ok(outcome)
                        })();
                        match published {
                            Ok(PublishOutcome::Accepted { .. } | PublishOutcome::Duplicate) => {
                                ProbeResult::Published
                            }
                            Ok(PublishOutcome::Superseded) => ProbeResult::Superseded,
                            Ok(PublishOutcome::RetiredGeneration) => ProbeResult::RetiredGeneration,
                            Ok(outcome) => {
                                tracing::warn!(?outcome, "probe observation refused");
                                ProbeResult::PublicationFailed
                            }
                            Err(e) => {
                                tracing::warn!("probe observation publication failed: {e}");
                                ProbeResult::PublicationFailed
                            }
                        }
                    },
                    |job, result| {
                        completed
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push((job, result))
                    },
                    || wake.completion(),
                );
                if let Err(e) = result {
                    tracing::error!("probe completion wake failed: {e}");
                    break;
                }
            }
        }));
    }
    let mut completions = || {
        std::mem::take(
            &mut *completed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    };
    let ready_ctx = runtime.clone();
    let mut ready = || {
        let ctx = ready_ctx.borrow();
        super::daemon::log_line(&ctx.rs, "watch: ready; waiting for usage and login changes");
        Ok(())
    };
    let result = watch_loop::run(Deps {
        events: events.as_mut(),
        directories: &directories,
        snapshot: &mut snapshot,
        coordinator: &mut coordinator,
        execute: &mut execute,
        completions: &mut completions,
        ready: &mut ready,
        now: &now,
    });
    // Closing the queue and cancelling reads releases every worker before
    // removing signal registrations or dropping the event descriptors.
    crate::commands::profile::usage_cmd::PROBE_CANCEL
        .store(true, std::sync::atomic::Ordering::SeqCst);
    drop(tx);
    for worker in workers {
        if worker.join().is_err() {
            tracing::error!("usage probe worker panicked during shutdown");
        }
    }
    for id in signal_ids {
        signal_hook::low_level::unregister(id);
    }
    result
}

/// Called under the existing state lock immediately before swap revalidation.
/// No credential is opened for change detection and no observation is reread.
pub(super) fn revalidate_inputs(
    ctx: &Ctx,
    state: &crate::commands::profile::auto_switch::kit::State,
) -> Result<bool, ProfileError> {
    let Some(inputs) = &ctx.event_inputs else {
        return Ok(true);
    };
    let effective = crate::commands::profile::auto_switch::config::effective(ctx.store.root());
    let control = super::inputs::policy_control(&effective, &super::pin::read(&ctx.rs));
    if !effective.on
        || control != inputs.control
        || inputs
            .logins
            .iter()
            .filter(|l| l.live || l.eligible)
            .any(|l| l.organisation.is_none())
    {
        return Ok(false);
    }
    let store = ObservationStore::open(ctx.store.root())?;
    let mut guard = store.under_state_lock();
    let profiles = ctx.store.list()?;
    for login in &inputs.logins {
        let binding = Binding {
            profile: login.profile.clone(),
            account_id: login.account_id.clone(),
            generation: login.generation.clone(),
        };
        if !profiles.contains(&login.profile)
            || guard.binding(&login.profile)?.as_ref() != Some(&binding)
            || guard.retire_if_changed(&binding)?
        {
            return Ok(false);
        }
        if let Some(evidence) = inputs
            .observations
            .iter()
            .find(|e| e.profile == login.profile)
            && guard.accepted_ticket(&binding)? != Some(evidence.ticket)
        {
            let original = inputs
                .records
                .iter()
                .find(|r| r.capture.binding == binding)
                .map(|r| &r.evidence);
            if guard.accepted_evidence(&binding)?.as_ref() != original || original.is_none() {
                return Ok(false);
            }
        }
        let profile = ProfileName::parse(&login.profile)?;
        let org = crate::commands::profile::expected::lookup(state, &profile)
            .ok()
            .flatten()
            .and_then(|r| r.org);
        if org != login.organisation {
            return Ok(false);
        }
    }
    Ok(true)
}
