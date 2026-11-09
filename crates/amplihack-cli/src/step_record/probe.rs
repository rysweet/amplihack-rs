//! Issue #2018: whether the processes a record names are still running, and
//! the state that follows (D8).
//!
//! A process is alive when its pid exists, it is not a zombie, and its start
//! time equals the recorded one where that is known. A reused pid has a
//! different start time, so it never stands in for the recorded process.

use super::{ProcId, StepRecord};

/// Reads process state. [`SystemProbe`] is the real one; tests supply a
/// fake.
pub trait Probe {
    /// The pid names a running process that is not a zombie.
    fn exists(&self, pid: i32) -> bool;
    /// The process's start time in clock ticks after boot, while it runs.
    fn start_ticks(&self, pid: i32) -> Option<u64>;
}

/// The running system: `/proc` on Linux, `kill(pid, 0)` elsewhere.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemProbe;

impl Probe for SystemProbe {
    #[cfg(target_os = "linux")]
    fn exists(&self, pid: i32) -> bool {
        // pid 0 and below name groups or every process, never one process.
        pid > 0 && crate::commands::recipe::process_tree::start_time(pid).is_some()
    }

    #[cfg(not(target_os = "linux"))]
    fn exists(&self, pid: i32) -> bool {
        if pid <= 0 {
            return false;
        }
        // SAFETY: signal 0 checks only that the pid exists; nothing is sent.
        if unsafe { libc::kill(pid, 0) } == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    #[cfg(target_os = "linux")]
    fn start_ticks(&self, pid: i32) -> Option<u64> {
        if pid <= 0 {
            return None;
        }
        crate::commands::recipe::process_tree::start_time(pid)
    }

    #[cfg(not(target_os = "linux"))]
    fn start_ticks(&self, _pid: i32) -> Option<u64> {
        None
    }
}

/// `pid` with its current start time, for a record about to be written.
pub fn proc_id_of(pid: i32) -> ProcId {
    ProcId {
        pid,
        start_ticks: SystemProbe.start_ticks(pid),
    }
}

/// The state of a readable record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    /// The step is running under the process that started it.
    Live,
    /// The step, or its agent, still runs but its parent or launcher is gone.
    Orphan,
    /// Nothing recorded is running.
    Stale,
}

impl StepState {
    pub fn as_str(self) -> &'static str {
        match self {
            StepState::Live => "live",
            StepState::Orphan => "orphan",
            StepState::Stale => "stale",
        }
    }
}

/// D8: launcher and parent alive is live; launcher alive without its parent,
/// or agent alive without its launcher, is an orphan; otherwise stale. A
/// `null` parent (outside the launcher's PID namespace) counts as alive.
pub fn classify(record: &StepRecord, probe: &impl Probe) -> StepState {
    let launcher = alive(probe, &record.launcher);
    let parent = record.parent.as_ref().is_none_or(|p| alive(probe, p));
    match (launcher, parent) {
        (true, true) => StepState::Live,
        (true, false) => StepState::Orphan,
        (false, _) if alive(probe, &record.agent) => StepState::Orphan,
        (false, _) => StepState::Stale,
    }
}

fn alive(probe: &impl Probe, proc_id: &ProcId) -> bool {
    if !probe.exists(proc_id.pid) {
        return false;
    }
    match proc_id.start_ticks {
        None => true,
        Some(ticks) => probe.start_ticks(proc_id.pid) == Some(ticks),
    }
}
