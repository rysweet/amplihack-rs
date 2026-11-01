//! The admission loop (D1, D2, D3, D11). Each round takes the lock, loads
//! the state, reclaims dead processes, decides, records the outcome and
//! releases the lock; a waiting start sleeps outside the lock and asks
//! again every 15 seconds until its deadline. Every fault admits.

use super::admit_state::{
    LOCK_WAIT, check_ticket, drop_ticket, fail_open, load_all, record, save_logged, take_ticket,
};
use super::config::{GOVERNOR_POLL_SECS, Settings};
use super::files::{GOVERNOR_QUEUE_CAP, tree_id_ok};
use super::guard::{Denied, SlotGuard};
use super::inputs;
use super::liveness::{reclaim, start_time};
use super::model::{
    Admission, Class, DecisionKind, FailOpen, FailOpenCode, Inputs, QueueEntry, ReasonCode,
    Request, Slot, Start,
};
use super::nesting::{marked_parent, nested_parent, parent_start};
use super::policy::decide;
use super::queue::prune_window;
use super::store::{Store, now, repair};
use std::path::Path;
use std::time::Duration;

/// Everything one admission needs; built by [`super::admit`] from the
/// command line, the environment and the profile root.
pub struct AdmitContext<'a> {
    pub root: Option<&'a Path>,
    pub settings: Settings,
    pub class: Class,
    /// What this start is; recorded on its slot so that starts nested
    /// under it know whether they are part of a running recipe.
    pub start: Start,
    pub workstream: String,
    pub wait_secs: u64,
    pub session_depth: Option<String>,
    pub tree_id: Option<String>,
    /// `AMPLIHACK_GOVERNOR_PARENT`: the slot tree id of the launch that
    /// started this process.
    pub parent: Option<String>,
    /// `AMPLIHACK_GOVERNOR_RECIPE`: the `recipe run` this process runs
    /// inside, when that run holds no slot of its own.
    pub recipe: Option<String>,
}

enum Round {
    Done(Result<SlotGuard, Denied>),
    Wait {
        line: String,
        retry_after: Option<i64>,
    },
}

fn round(
    store: &Store,
    root: &Path,
    ctx: &AdmitContext<'_>,
    tree_id: &str,
    deadline: i64,
    ticket: &mut Option<u64>,
) -> Round {
    let lock = match store.lock_exclusive(LOCK_WAIT) {
        Ok(lock) => lock,
        Err(f) => return Round::Done(Ok(fail_open(&f))),
    };
    let now = now();
    let mut l = match load_all(store, &lock) {
        Ok(l) => l,
        Err(f) => return Round::Done(Ok(fail_open(&f))),
    };
    let max = ctx.settings.max_concurrency.max(1);
    if l.state.limit == 0 {
        l.state.limit = max;
    }
    repair(&mut l.state, &l.queue, max);
    reclaim(&mut l.state.slots, &mut l.queue.entries);
    prune_window(&mut l.state.admissions, now);
    check_ticket(&l.queue, ticket);
    if let Some(f) = l.fault.take() {
        take_ticket(&mut l.queue, ticket);
        l.state.fail_open = Some(record(&f, now));
        let _ = save_logged(store, &lock, &l);
        return Round::Done(Ok(fail_open(&f)));
    }
    let parent = nested_parent(
        ctx.session_depth.as_deref(),
        ctx.tree_id.as_deref(),
        &l.state.slots,
    )
    .or_else(|| marked_parent(ctx.parent.as_deref(), &l.state.slots));
    if let Some(i) = parent {
        tracing::debug!(
            parent_pid = l.state.slots[i].pid,
            "governor: nested start, not counted against the limit"
        );
    }
    let snaps = inputs::snapshots(root);
    inputs::record(&mut l.history, &snaps);
    let budgets = inputs::budgets(&snaps, now);
    let (_, signal) = inputs::rates(&budgets, &l.history, now, ctx.settings.band_pct);
    if parent.is_none() && ticket.is_none() && l.queue.entries.len() >= GOVERNOR_QUEUE_CAP {
        let f = FailOpen::new(
            FailOpenCode::QueueFull,
            format!("{} requests are already waiting", l.queue.entries.len()),
        );
        l.state.fail_open = Some(record(&f, now));
        let _ = save_logged(store, &lock, &l);
        return Round::Done(Ok(fail_open(&f)));
    }
    let inputs = Inputs {
        budgets,
        signal,
        limit: l.state.limit,
        last_evaluated_at: l.state.last_evaluated_at,
        slots: l.state.slots.clone(),
        queue: l.queue.entries.clone(),
        admissions: l.state.admissions.clone(),
        settings: ctx.settings,
        request: Request {
            class: ctx.class,
            workstream: ctx.workstream.clone(),
            start: ctx.start,
            parent: parent.map(|i| parent_start(&l.state.slots[i], ctx.recipe.as_deref())),
            seq: *ticket,
            deadline,
        },
    };
    let d = decide(&inputs, now);
    if d.evaluated {
        l.state.limit = d.limit_after;
        l.state.last_evaluated_at = Some(now);
    }
    let code = d.reason.code.as_str();
    match d.kind {
        DecisionKind::Admit => {
            take_ticket(&mut l.queue, ticket);
            let no_readings = d.reason.code == ReasonCode::NoReadings;
            let fault = no_readings
                .then(|| FailOpen::new(FailOpenCode::NoReadings, d.reason.message.clone()));
            l.state.fail_open = fault.as_ref().map(|f| record(f, now));
            let made = match parent {
                Some(i) => {
                    let p = &mut l.state.slots[i];
                    p.nested_children = p.nested_children.saturating_add(1);
                    Err((p.pid, p.tree_id.clone()))
                }
                None => {
                    let pid = std::process::id();
                    // Without /proc a reused PID can leave a record with this
                    // key; a duplicate would make the file corrupt.
                    l.state
                        .slots
                        .retain(|s| !(s.pid == pid && s.tree_id == tree_id));
                    l.state.slots.push(Slot {
                        pid,
                        start_time: start_time(pid),
                        workstream: ctx.workstream.clone(),
                        class: ctx.class,
                        tree_id: tree_id.to_string(),
                        admitted_at: now,
                        nested_children: 0,
                        start: ctx.start,
                    });
                    l.state.admissions.push(Admission {
                        workstream: ctx.workstream.clone(),
                        at: now,
                    });
                    Ok(pid)
                }
            };
            // A failed save recorded nothing, so the guard holds nothing.
            if let Err(f) = save_logged(store, &lock, &l) {
                return Round::Done(Ok(fail_open(&f)));
            }
            let guard = match made {
                Ok(pid) => SlotGuard::slot(
                    root.to_path_buf(),
                    pid,
                    tree_id.to_string(),
                    fault.map(|f| f.code),
                ),
                Err((parent_pid, parent_tree)) => {
                    SlotGuard::nested_under(root.to_path_buf(), parent_pid, parent_tree)
                }
            };
            if d.announce {
                eprintln!(
                    "amplihack: governor: admitted ({code}): {}",
                    d.reason.message
                );
            }
            Round::Done(Ok(guard))
        }
        DecisionKind::Deny => {
            take_ticket(&mut l.queue, ticket);
            let _ = save_logged(store, &lock, &l);
            Round::Done(Err(Denied::new(
                ReasonCode::WeeklyExhausted.as_str(),
                d.reason.message,
            )))
        }
        DecisionKind::Wait if now >= deadline => {
            take_ticket(&mut l.queue, ticket);
            let _ = save_logged(store, &lock, &l);
            let message = format!("the wait ended while held by {code}: {}", d.reason.message);
            Round::Done(Err(Denied::new(ReasonCode::WaitExpired.as_str(), message)))
        }
        DecisionKind::Wait => {
            if parent.is_none() && ticket.is_none() {
                let pid = std::process::id();
                let seq = l.state.next_seq;
                l.state.next_seq = seq.saturating_add(1);
                l.queue.entries.push(QueueEntry {
                    seq,
                    workstream: ctx.workstream.clone(),
                    class: ctx.class,
                    pid,
                    start_time: start_time(pid),
                    enqueued_at: now,
                });
                // Kept even when the save fails: the entry may be on disk
                // already, and the next round's check forgets it if not.
                let _ = save_logged(store, &lock, &l);
                *ticket = Some(seq);
            } else {
                let _ = save_logged(store, &lock, &l);
            }
            Round::Wait {
                line: format!(
                    "amplihack: governor: waiting ({code}): {}; giving up at {}",
                    d.reason.message,
                    chrono::DateTime::from_timestamp(deadline, 0).map_or_else(
                        || deadline.to_string(),
                        |t| t.format("%H:%M:%S UTC").to_string()
                    )
                ),
                retry_after: d.retry_after,
            }
        }
    }
}

/// Decides one start; see the module documentation.
pub fn admit_with(ctx: &AdmitContext<'_>) -> Result<SlotGuard, Denied> {
    tracing::debug!(class = ctx.class.as_str(), "governor: deciding a start");
    let Some(root) = ctx.root else {
        return Ok(fail_open(&FailOpen::new(
            FailOpenCode::NoRoot,
            "no profile root",
        )));
    };
    let store = match Store::open(root) {
        Ok(store) => store,
        Err(f) => return Ok(fail_open(&f)),
    };
    let tree_id = ctx
        .tree_id
        .clone()
        .filter(|t| tree_id_ok(t))
        .unwrap_or_else(crate::commands::recipe::run::execute::new_tree_id);
    let wait = i64::try_from(ctx.wait_secs).unwrap_or(i64::MAX);
    let deadline = now().saturating_add(wait);
    let mut ticket = None;
    let mut last_line = String::new();
    loop {
        match round(&store, root, ctx, &tree_id, deadline, &mut ticket) {
            Round::Done(result) => {
                drop_ticket(&store, &mut ticket);
                return result;
            }
            Round::Wait { line, retry_after } => {
                if line != last_line {
                    eprintln!("{line}");
                    last_line = line;
                }
                let now = now();
                let mut secs = GOVERNOR_POLL_SECS
                    .min(u64::try_from(deadline.saturating_sub(now)).unwrap_or(0));
                if let Some(at) = retry_after.filter(|at| *at > now) {
                    secs = secs.min(u64::try_from(at - now).unwrap_or(secs));
                }
                std::thread::sleep(Duration::from_secs(secs.max(1)));
            }
        }
    }
}
