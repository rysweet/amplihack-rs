//! Whether a conversation is running (R4, design 4.8), and how the
//! supervisor classifies an agent from that and its tmux session. Doubt
//! counts as running (R4.2, D21): any positive signal wins, and otherwise an
//! unreadable process table, sessions index or Claude command line makes
//! the answer unknown, which starts nothing.

use super::procfs::{ClaudeNames, Snapshot, is_claude_with};
use super::registry::Registry;
use super::sessions::{Index, for_process};
use super::tmux::SessionState;
use super::validate;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum Liveness {
    /// Which signal saw the conversation running.
    Running(&'static str),
    NotRunning,
    /// The reason liveness could not be decided.
    Unknown(String),
}

/// The three signals of R4.1, in order: (a) a sessions file for `id` whose
/// process is alive with the recorded start time; (b) a Claude process
/// whose command line holds `id`; (c) the recorded process, still alive and
/// still Claude (C5). Without an id only (c) applies.
pub fn liveness(
    snap: Result<&Snapshot, &str>,
    index: Result<&Index, &str>,
    names: &ClaudeNames,
    id: Option<&str>,
    recorded: Option<(u32, u64)>,
) -> Liveness {
    if let Ok(s) = snap {
        if let (Some(id), Ok(idx)) = (id, index) {
            let pairs = idx.by_id.get(id);
            if pairs.is_some_and(|v| v.iter().any(|&(pid, start)| s.alive(pid, start))) {
                return Liveness::Running("sessions file");
            }
        }
        if let Some(id) = id {
            let on_argv = s.procs.values().any(|p| match &p.argv {
                Ok(argv) => {
                    is_claude_with(names, &p.comm, argv) && argv.iter().any(|a| a.contains(id))
                }
                Err(_) => false,
            });
            if on_argv {
                return Liveness::Running("command line");
            }
        }
        if let Some((pid, start)) = recorded
            && s.alive(pid, start)
            && let Some(p) = s.procs.get(&pid)
            && is_claude_with(names, &p.comm, p.argv.as_deref().unwrap_or(&[]))
        {
            return Liveness::Running("recorded process");
        }
    }
    match (snap, index, id) {
        (Err(why), _, _) => Liveness::Unknown(why.to_string()),
        (Ok(_), Err(why), Some(_)) => Liveness::Unknown(why.to_string()),
        (Ok(s), _, Some(_)) if unreadable_claude(s, names) => {
            Liveness::Unknown("the command line of a claude process cannot be read".into())
        }
        _ => Liveness::NotRunning,
    }
}

/// Whether a process that looks like Claude by its comm alone has a
/// command line that could not be read, so signal (b) cannot be ruled out.
fn unreadable_claude(s: &Snapshot, names: &ClaudeNames) -> bool {
    s.procs
        .values()
        .any(|p| p.argv.is_err() && is_claude_with(names, &p.comm, &[]))
}

/// How the supervisor treats an agent this pass.
#[derive(Debug, Clone, PartialEq)]
pub enum Observation {
    /// Running with its tmux session: nothing to do, no line.
    Healthy,
    /// Running without its session; holds the liveness reason when unknown.
    SkipRunning(Option<String>),
    /// Not running but its session exists; holds the tmux reason when its
    /// state is unknown (B5).
    SkipSession(Option<String>),
    /// Neither: a candidate for a resume.
    Lost,
}

/// Design 4.8: unknown liveness counts as running, and an unknown tmux
/// state counts as a present session.
pub fn observe(live: &Liveness, session: &SessionState) -> Observation {
    let not_running = matches!(live, Liveness::NotRunning);
    match (session, not_running) {
        (SessionState::Present | SessionState::Unknown(_), false) => Observation::Healthy,
        (SessionState::Absent, false) => Observation::SkipRunning(match live {
            Liveness::Unknown(why) => Some(why.clone()),
            _ => None,
        }),
        (SessionState::Present, true) => Observation::SkipSession(None),
        (SessionState::Unknown(why), true) => Observation::SkipSession(Some(why.clone())),
        (SessionState::Absent, true) => Observation::Lost,
    }
}

/// D5: the conversation the recorded process now holds, when the process is
/// alive with its recorded start time, its sessions file names a valid id
/// that differs from `current`, and no other `active` agent holds that id
/// (C3).
pub fn refreshed_id(
    snap: &Snapshot,
    dirs: &[PathBuf],
    recorded: (u32, u64),
    current: Option<&str>,
    reg: &Registry,
    name: &str,
) -> Option<String> {
    let (pid, start) = recorded;
    if !snap.alive(pid, start) {
        return None;
    }
    let id = for_process(dirs, pid, start)?.session_id;
    let fresh = validate::conversation_id(&id).is_ok()
        && current != Some(id.as_str())
        && reg.active_with_conversation(&id, name).is_none();
    fresh.then_some(id)
}
