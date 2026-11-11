//! One supervisor pass (design 5.1): load the registry, refresh drifted
//! conversation ids (D5), apply SessionEnd markers, classify every `active`
//! agent, run the pre-checks on lost ones (R5.4, D14), then hand the rest to
//! `resume.rs`. Every write goes through `store::update` and applies only
//! while the agent's `registered_at` is unchanged (A2). Each line is printed
//! and appended to the log after the write it belongs to (A15, B8).

use super::markers::{self, Disposition};
use super::procfs::{self, Snapshot};
use super::registry::{EndedBy, Entry, Registry, State, fmt_ts, restarts_within};
use super::report::{self, Outcome};
use super::resume::{self, Candidate, emit, outcome_of, precheck, record_line};
use super::scan::{self, Liveness, Observation};
use super::sessions::{self, Index};
use super::store::{self, AgentsDir};
use super::tmux::{SessionState, Tmux};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

const HOUR: i64 = 3600;

pub struct PassCfg {
    pub batch: usize,
    pub gap: Duration,
    pub max_restarts: usize,
    /// The amplihack executable the pane runs, checked once at start (S16).
    pub exe: PathBuf,
}

/// `/proc` and the Claude config directories read once (A11). Errors are
/// kept as reasons: they make liveness unknown, never end the pass.
pub(super) struct View {
    pub snap: Result<Snapshot, String>,
    pub dirs: Result<Vec<PathBuf>, String>,
    pub index: Result<Index, String>,
}

impl View {
    pub(super) fn read(home: &Path) -> Self {
        let uid = amplihack_utils::secure_files::euid();
        let snap = procfs::snapshot(&procfs::proc_root(), uid);
        let dirs = sessions::config_dirs(home);
        let index = dirs.clone().and_then(|d| sessions::index(&d));
        Self { snap, dirs, index }
    }

    pub(super) fn liveness(&self, e: &Entry) -> Liveness {
        scan::liveness(
            self.snap.as_ref().map_err(String::as_str),
            self.index.as_ref().map_err(String::as_str),
            &procfs::CLAUDE,
            e.conversation_id.as_deref(),
            e.recorded(),
        )
    }
}

pub fn run_pass(
    dir: &AgentsDir,
    home: &Path,
    cfg: &PassCfg,
    stop: &AtomicBool,
) -> anyhow::Result<()> {
    let reg = load(dir)?;
    let now = super::registry::now();
    let view = View::read(home);
    refresh_ids(dir, &reg, &view, now)?;
    apply_markers(dir, &view, now)?;
    let reg = load(dir)?;
    let candidates = classify(dir, home, &reg, &view, cfg, now)?;
    resume::run(dir, home, cfg, candidates, stop)
}

pub(super) fn load(dir: &AgentsDir) -> anyhow::Result<Registry> {
    store::load(dir).map_err(|e| e.into_error(dir))
}

/// D5: an agent whose recorded process now holds another conversation (after
/// `/clear`, say) follows it, unless another active agent has it (C3).
fn refresh_ids(dir: &AgentsDir, reg: &Registry, view: &View, now: i64) -> anyhow::Result<()> {
    let (Ok(snap), Ok(dirs)) = (&view.snap, &view.dirs) else {
        return Ok(());
    };
    let changes: Vec<(String, String, String, String)> = active(reg)
        .filter_map(|(name, e)| {
            let current = e.conversation_id.as_deref();
            let id = scan::refreshed_id(snap, dirs, e.recorded()?, current, reg, name)?;
            Some((
                name.clone(),
                e.registered_at.clone(),
                e.launch_id.clone(),
                id,
            ))
        })
        .collect();
    if changes.is_empty() {
        return Ok(());
    }
    store::update(dir, now, |r| {
        for (name, registered_at, launch_id, id) in &changes {
            if r.active_with_conversation(id, name).is_some() {
                continue;
            }
            if let Some(e) = r.agents.get_mut(name)
                && e.state == State::Active
                && e.registered_at == *registered_at
                && e.launch_id == *launch_id
            {
                e.conversation_id = Some(id.clone());
            }
        }
        Ok(())
    })
}

/// Ends the agents that SessionEnd markers name (R6.3, A6), then deletes
/// the markers that were applied or are stale. Ending writes no line. An
/// `ended/` that is not a private directory gives one warning and no
/// markers.
fn apply_markers(dir: &AgentsDir, view: &View, now: i64) -> anyhow::Result<()> {
    let found = match markers::read(&dir.ended(), now) {
        Ok(found) => found,
        Err(e) => {
            eprintln!("warning: ~/.amplihack/agents/ended: {e}; no markers were read");
            return Ok(());
        }
    };
    if found.is_empty() {
        return Ok(());
    }
    let reg = load(dir)?;
    // Without a snapshot the recorded process may be alive: keep the marker.
    let alive = |pid: u32, start: u64| match &view.snap {
        Ok(s) => s.alive(pid, start),
        Err(_) => true,
    };
    let mut ends = Vec::new();
    let mut delete = Vec::new();
    for m in &found {
        match markers::decide(m, &reg, &alive, now) {
            Disposition::End(name) => {
                if let Some(e) = reg.agents.get(&name) {
                    ends.push((name, e.registered_at.clone(), m));
                }
            }
            Disposition::Delete => delete.push(m.path.clone()),
            Disposition::Keep => {}
        }
    }
    if !ends.is_empty() {
        let ended = store::update(dir, now, |r| {
            let mut done = Vec::new();
            for (name, registered_at, m) in &ends {
                if let Some(e) = r.agents.get_mut(name)
                    && e.state == State::Active
                    && e.registered_at == *registered_at
                    && e.conversation_id.as_deref() == Some(m.session_id.as_str())
                {
                    e.end(now, EndedBy::SessionEnd);
                    done.push(m.path.clone());
                }
            }
            Ok(done)
        })?;
        delete.extend(ended);
    }
    for path in delete {
        if let Err(e) = std::fs::remove_file(&path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!(
                "warning: {}: {e}; the marker was not removed",
                path.display()
            );
        }
    }
    Ok(())
}

/// What one active agent looked like before the write.
struct Plan {
    name: String,
    registered_at: String,
    launch_id: String,
    conv: Option<String>,
    obs: Observation,
    /// For a lost agent: the first failed pre-check made outside the lock.
    pre: Option<Outcome>,
    launch_dir: String,
}

/// Steps 5 to 7: classification (one `has-session` per socket and session),
/// then one write that clears or sets `lost_at`, records each line and
/// applies the restart limit. A healthy agent also loses a stored skip or
/// failure, so its recurrence is logged again. Returns the agents to resume.
fn classify(
    dir: &AgentsDir,
    home: &Path,
    reg: &Registry,
    view: &View,
    cfg: &PassCfg,
    now: i64,
) -> anyhow::Result<Vec<Candidate>> {
    let mut sessions: HashMap<(Option<String>, String), SessionState> = HashMap::new();
    let mut plans = Vec::new();
    // Skips even the lock on a healthy pass; `store::update` only skips an unchanged write.
    let mut write = false;
    for (name, e) in active(reg) {
        let live = view.liveness(e);
        let key = (e.tmux_socket.clone(), e.tmux_session.clone());
        let state = sessions.entry(key).or_insert_with(|| {
            Tmux::new(e.tmux_socket.as_deref().map(Path::new)).has_session(&e.tmux_session)
        });
        let obs = scan::observe(&live, state);
        let last = e.last_recovery.as_ref().map(|l| l.result.as_str());
        write |= obs != Observation::Healthy
            || e.lost_at.is_some()
            || last.is_some_and(report::clears_when_healthy);
        let pre = (obs == Observation::Lost)
            .then(|| precheck(home, e))
            .flatten();
        plans.push(Plan {
            name: name.clone(),
            registered_at: e.registered_at.clone(),
            launch_id: e.launch_id.clone(),
            conv: e.conversation_id.clone(),
            obs,
            pre,
            launch_dir: e.launch_dir.clone(),
        });
    }
    if !write {
        return Ok(Vec::new());
    }
    let (lines, candidates) = store::update(dir, now, |r| {
        let mut lines = Vec::new();
        let mut candidates = Vec::new();
        for p in &plans {
            let Some(e) = r.agents.get_mut(&p.name) else {
                continue;
            };
            let same = e.registered_at == p.registered_at
                && e.launch_id == p.launch_id
                && e.conversation_id == p.conv;
            if e.state != State::Active || !same {
                continue;
            }
            if p.obs == Observation::Healthy {
                e.lost_at = None;
                let last = e.last_recovery.as_ref().map(|l| l.result.as_str());
                if last.is_some_and(report::clears_when_healthy) {
                    e.last_recovery = None;
                }
                continue;
            }
            let n = restarts_within(&e.restarts, now, HOUR);
            let outcome = match outcome_of(&p.obs) {
                Some(skipped) => skipped,
                None => {
                    if e.lost_at.is_none() {
                        e.lost_at = Some(fmt_ts(now));
                    }
                    match &p.pre {
                        Some(failed) => failed.clone(),
                        None if n >= cfg.max_restarts => Outcome::RestartLimit,
                        None => {
                            candidates.push(Candidate {
                                name: p.name.clone(),
                                registered_at: p.registered_at.clone(),
                                launch_dir: p.launch_dir.clone(),
                            });
                            continue;
                        }
                    }
                }
            };
            lines.extend(record_line(e, &p.name, now, &outcome));
            if outcome == Outcome::RestartLimit {
                e.stop(&report::alert_text(n));
                lines.push(report::alert_line(now, &p.name, n));
            }
        }
        Ok((lines, candidates))
    })?;
    for line in &lines {
        emit(&dir.log(), line);
    }
    Ok(candidates)
}

fn active(reg: &Registry) -> impl Iterator<Item = (&String, &Entry)> {
    reg.agents.iter().filter(|(_, e)| e.state == State::Active)
}
