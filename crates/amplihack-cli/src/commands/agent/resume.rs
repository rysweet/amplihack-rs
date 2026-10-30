//! Resuming the lost agents of one pass (R5.5 to R5.8): dirtiest first, in
//! batches with a gap between them, each one checked again immediately
//! before its `tmux new-session` (D15), and each conversation at most once
//! per pass (C3).

use super::dirty;
use super::launch;
use super::pass::{PassCfg, View, load};
use super::registry::{Entry, State, fmt_ts};
use super::report::{self, Outcome};
use super::scan::{self, Observation};
use super::server::{Launched, Placement, Real};
use super::sessions;
use super::store::{self, AgentsDir};
use super::supervise::sleep_unless_stopped;
use super::tmux::Tmux;
use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// A lost agent that passed its pre-checks.
pub(super) struct Candidate {
    pub name: String,
    pub registered_at: String,
    pub launch_dir: String,
}

/// Runs `each` over `items`, `batch` at a time, sleeping `gap` between
/// batches and never after the last. The stop flag is checked before each
/// item; a sleeper that returns `false` (stopped) or an item that returns
/// `Break` ends the schedule, without another sleep.
pub fn schedule<T>(
    items: Vec<T>,
    batch: usize,
    gap: Duration,
    sleep: &dyn Fn(Duration, &AtomicBool) -> bool,
    stop: &AtomicBool,
    mut each: impl FnMut(T) -> ControlFlow<()>,
) {
    let batch = batch.max(1);
    for (i, item) in items.into_iter().enumerate() {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        if i > 0 && i % batch == 0 && !sleep(gap, stop) {
            return;
        }
        if each(item).is_break() {
            return;
        }
    }
}

/// Orders the candidates by `git status` line count, then name, and resumes
/// them in batches. The first error ends the pass at once, without waiting
/// for the next batch, and is returned.
pub(super) fn run(
    dir: &AgentsDir,
    home: &Path,
    cfg: &PassCfg,
    candidates: Vec<Candidate>,
    stop: &AtomicBool,
) -> anyhow::Result<()> {
    let counts = candidates
        .iter()
        .map(|c| (c.name.clone(), dirty::dirty_count(Path::new(&c.launch_dir))))
        .collect();
    let order = dirty::order(counts);
    let registered: HashMap<String, String> = candidates
        .into_iter()
        .map(|c| (c.name, c.registered_at))
        .collect();
    let mut resumed = HashSet::new();
    let mut placement = Placement::default();
    let mut failure = None;
    schedule(
        order,
        cfg.batch,
        cfg.gap,
        &sleep_unless_stopped,
        stop,
        |name| {
            let Some(registered_at) = registered.get(&name) else {
                return ControlFlow::Continue(());
            };
            let held = (&mut resumed, &mut placement);
            match resume_one(dir, home, cfg, &name, registered_at, held) {
                Ok(()) => ControlFlow::Continue(()),
                Err(e) => {
                    failure = Some(e);
                    ControlFlow::Break(())
                }
            }
        },
    );
    failure.map_or(Ok(()), Err)
}

/// D15: the record is read again and dropped without a line if it was
/// removed, registered again or is no longer active; liveness, tmux and the
/// pre-checks are repeated; a conversation already resumed in this pass is
/// skipped. Otherwise the pane command is started, placed by `server.rs`.
fn resume_one(
    dir: &AgentsDir,
    home: &Path,
    cfg: &PassCfg,
    name: &str,
    registered_at: &str,
    (resumed, placement): (&mut HashSet<String>, &mut Placement),
) -> anyhow::Result<()> {
    let reg = load(dir)?;
    let Some(e) = reg.agents.get(name) else {
        return Ok(());
    };
    if e.registered_at != registered_at || e.state != State::Active {
        return Ok(());
    }
    let view = View::read(home);
    let tmux = Tmux::new(e.tmux_socket.as_deref().map(Path::new));
    let obs = scan::observe(&view.liveness(e), &tmux.has_session(&e.tmux_session));
    if obs == Observation::Healthy {
        if e.lost_at.is_none() {
            return Ok(());
        }
        return settle(dir, name, registered_at, None, false);
    }
    let held = |id: &str| resumed.contains(id).then_some(Outcome::AlreadyRunning);
    let changed = outcome_of(&obs)
        .or_else(|| precheck(home, e))
        .or_else(|| e.conversation_id.as_deref().and_then(held));
    if let Some(outcome) = changed {
        return settle(dir, name, registered_at, Some(outcome), false);
    }
    let Some(id) = e.conversation_id.clone() else {
        return Ok(());
    };
    let now = super::registry::now();
    let now_ts = fmt_ts(now);
    let lost = e.lost_at.clone();
    let message = launch::message(lost.as_deref().unwrap_or(&now_ts), &now_ts);
    let argv = launch::pane_command(name, e, &cfg.exe, &message);
    let real = Real {
        tmux: &tmux,
        session: &e.tmux_session,
        window: &e.tmux_window,
        dir: Path::new(&e.launch_dir),
        command: &argv,
    };
    let socket = e.tmux_socket.as_deref().map(Path::new);
    let warning = match placement.launch(socket, now, &real) {
        Launched::Started { warning } => warning,
        // P6: a failed launch counts as a restart, so the restart limit
        // ends an agent whose launch keeps failing.
        Launched::Failed(line) => {
            return settle(dir, name, registered_at, Some(Outcome::Tmux(line)), true);
        }
        Launched::Unknown { reason, attempted } => {
            let skipped = Some(Outcome::TmuxUnknown(reason));
            return settle(dir, name, registered_at, skipped, attempted);
        }
    };
    resumed.insert(id.clone());
    let result = Outcome::Resumed.to_string();
    let (conv, lost, at) = (Some(id.as_str()), lost.as_deref(), Some(now_ts.as_str()));
    let line = report::recovery_line(now, name, conv, lost, at, &result);
    let written = store::update(dir, now, |r| {
        if let Some(e) = r.agents.get_mut(name)
            && e.registered_at == registered_at
            && e.state == State::Active
        {
            let rec = report::last_recovery(now, conv, lost, at, &result);
            e.record_resume(now, rec);
        }
        Ok(())
    });
    // The session was started either way, so the line is always written.
    emit(&dir.log(), &line);
    if let Some(reason) = warning {
        emit(
            &dir.log(),
            &report::server_unscoped_line(now, name, &reason),
        );
    }
    written
}

/// Records what the check before a resume found instead: a line for
/// `outcome` (subject to D13), or, for an agent found healthy, a cleared
/// `lost_at` and no line. `restart` also counts a restart: a launch was
/// attempted, even if the line repeats and is not written.
fn settle(
    dir: &AgentsDir,
    name: &str,
    registered_at: &str,
    outcome: Option<Outcome>,
    restart: bool,
) -> anyhow::Result<()> {
    let now = super::registry::now();
    let line = store::update(dir, now, |r| {
        let Some(e) = r.agents.get_mut(name) else {
            return Ok(None);
        };
        if e.registered_at != registered_at || e.state != State::Active {
            return Ok(None);
        }
        if restart {
            e.restarts.push(fmt_ts(now));
        }
        Ok(match &outcome {
            Some(o) => record_line(e, name, now, o),
            None => {
                e.lost_at = None;
                None
            }
        })
    })?;
    if let Some(line) = line {
        emit(&dir.log(), &line);
    }
    Ok(())
}

/// The pre-checks of R5.4 that need the filesystem, in order: conversation
/// id, launch directory, then the transcript in the config directory the
/// resume will use (D12, A4). `None` when all pass.
pub(super) fn precheck(home: &Path, e: &Entry) -> Option<Outcome> {
    let Some(id) = e.conversation_id.as_deref() else {
        return Some(Outcome::NoConversation);
    };
    let launch_dir = Path::new(&e.launch_dir);
    if !launch_dir.is_dir() {
        return Some(Outcome::NoLaunchDir);
    }
    let config = sessions::profile_config_dir(home, e.amplihack_flags.profile.as_deref());
    match config {
        Ok(c) if sessions::transcript_exists(&c, launch_dir, id) => None,
        _ => Some(Outcome::NoTranscript),
    }
}

/// The result of a skipped agent; `None` for a healthy or a lost one.
pub(super) fn outcome_of(obs: &Observation) -> Option<Outcome> {
    match obs {
        Observation::SkipRunning(None) => Some(Outcome::AlreadyRunning),
        Observation::SkipRunning(Some(why)) => Some(Outcome::LivenessUnknown(why.clone())),
        Observation::SkipSession(None) => Some(Outcome::SessionExists),
        Observation::SkipSession(Some(why)) => Some(Outcome::TmuxUnknown(why.clone())),
        Observation::Healthy | Observation::Lost => None,
    }
}

/// Sets `last_recovery` and returns the line of a result other than a
/// resume, unless the line repeats the agent's previous one (D13). A stop
/// is a change of state and is always written.
pub(super) fn record_line(
    e: &mut Entry,
    name: &str,
    now: i64,
    outcome: &Outcome,
) -> Option<String> {
    let result = outcome.to_string();
    let conv = e.conversation_id.clone();
    let last = e.last_recovery.as_ref();
    if *outcome != Outcome::RestartLimit && report::suppressed(last, conv.as_deref(), &result) {
        return None;
    }
    let lost = e.lost_at.clone();
    let (conv, lost) = (conv.as_deref(), lost.as_deref());
    e.last_recovery = Some(report::last_recovery(now, conv, lost, None, &result));
    Some(report::recovery_line(now, name, conv, lost, None, &result))
}

/// Prints `line` (for the service journal) and appends it to the log. A
/// failed append is a warning: the line has still been printed.
pub(super) fn emit(log: &Path, line: &str) {
    println!("{line}");
    if let Err(e) = report::append(log, line) {
        eprintln!(
            "warning: {}: {e}; the line was printed but not logged",
            log.display()
        );
    }
}
