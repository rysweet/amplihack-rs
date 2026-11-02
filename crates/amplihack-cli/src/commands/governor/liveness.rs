//! Dead-process reclaim (D3). A record is live while its PID exists and,
//! where `/proc` can say, still has the start time recorded with it, so a
//! reused PID does not keep a slot. Signal 0 is the only signal ever sent:
//! it checks existence and delivers nothing.

use super::model::{QueueEntry, Slot};

/// The longest `/proc/<pid>/stat` read.
const MAX_STAT: usize = 4096;
/// Field 22 (start time) is the 20th field after the closing `)`.
const START_TIME_INDEX: usize = 19;

/// Field 22 of `/proc/<pid>/stat`, counted after the last `)` because the
/// command name may hold spaces and parentheses.
pub fn parse_stat_start_time(stat: &str) -> Option<u64> {
    if stat.len() > MAX_STAT {
        return None;
    }
    let close = stat.rfind(')')?;
    stat[close + 1..]
        .split_whitespace()
        .nth(START_TIME_INDEX)?
        .parse()
        .ok()
}

/// The start time of `pid`, in clock ticks since boot.
#[cfg(target_os = "linux")]
pub fn start_time(pid: u32) -> Option<u64> {
    let file = std::fs::File::open(format!("/proc/{pid}/stat")).ok()?;
    let raw = amplihack_utils::secure_files::read_bounded(file, MAX_STAT as u64).ok()?;
    parse_stat_start_time(std::str::from_utf8(&raw).ok()?)
}

/// No `/proc` here: the start time is unknown and PID reuse goes undetected.
#[cfg(not(target_os = "linux"))]
pub fn start_time(_pid: u32) -> Option<u64> {
    None
}

/// Field 4 (parent PID) of `/proc/<pid>/stat`, counted after the last `)`.
pub fn parse_stat_ppid(stat: &str) -> Option<u32> {
    if stat.len() > MAX_STAT {
        return None;
    }
    let close = stat.rfind(')')?;
    stat[close + 1..].split_whitespace().nth(1)?.parse().ok()
}

/// The longest parent chain walked.
const MAX_HOPS: usize = 64;

/// Whether `pid` is this process or one of its ancestors. A marker that
/// leaked into an unrelated environment (a tmux server's, say) then names a
/// slot that is not an ancestor and nests nothing.
pub fn self_or_ancestor(pid: u32) -> bool {
    in_ancestry(std::process::id(), pid)
}

/// Whether `pid` is `from` or one of `from`'s ancestors.
#[cfg(target_os = "linux")]
pub fn in_ancestry(from: u32, pid: u32) -> bool {
    let mut current = from;
    for _ in 0..MAX_HOPS {
        if current == pid {
            return true;
        }
        let parent = std::fs::File::open(format!("/proc/{current}/stat"))
            .ok()
            .and_then(|f| amplihack_utils::secure_files::read_bounded(f, MAX_STAT as u64).ok())
            .and_then(|raw| parse_stat_ppid(std::str::from_utf8(&raw).ok()?));
        match parent {
            Some(p) if p > 1 => current = p,
            _ => return false,
        }
    }
    false
}

/// No `/proc` here: the ancestry cannot be checked and the marker is
/// trusted, as the tree id is.
#[cfg(not(target_os = "linux"))]
pub fn in_ancestry(_from: u32, _pid: u32) -> bool {
    true
}

/// Whether `pid` is a live process with the recorded start time. PIDs
/// outside `2..=i32::MAX` are never signalled and count as dead, as does a
/// process this user may not signal (`EPERM`) and one whose recorded start
/// time cannot be checked.
pub fn alive(pid: u32, recorded_start: Option<u64>) -> bool {
    let Ok(raw) = i32::try_from(pid) else {
        return false;
    };
    if raw < 2 {
        return false;
    }
    // SAFETY: kill with signal 0 sends nothing; it only checks that the
    // process exists and may be signalled. `raw` is a positive PID, so it
    // never addresses a process group or every process.
    let rc = unsafe { libc::kill(raw, 0) };
    if rc != 0 {
        return false;
    }
    same_process(recorded_start, start_time(pid))
}

/// Whether a live PID is still the process that was recorded. A slot with
/// no recorded start time is trusted (no `/proc` when it was made); a
/// recorded start time that can no longer be read counts as dead (S2.3).
pub fn same_process(recorded: Option<u64>, observed: Option<u64>) -> bool {
    match (recorded, observed) {
        (Some(want), Some(got)) => want == got,
        (Some(_), None) => false,
        (None, _) => true,
    }
}

/// Removes the slots and queue entries of dead processes; returns how many.
pub fn reclaim(slots: &mut Vec<Slot>, queue: &mut Vec<QueueEntry>) -> usize {
    let before = slots.len() + queue.len();
    slots.retain(|s| alive(s.pid, s.start_time));
    queue.retain(|q| alive(q.pid, q.start_time));
    before - slots.len() - queue.len()
}
