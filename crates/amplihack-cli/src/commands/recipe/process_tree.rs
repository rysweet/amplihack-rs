//! Issue #1606: this user's processes, read from `/proc`, for `recipe stop`
//! and for `recipe run`'s own teardown on a stop signal.
//!
//! Linux only. Only processes owned by the effective uid are listed, since
//! they are the only ones that could be signalled anyway.

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::MetadataExt;

/// One process: its id, parent, process group, and start time (clock ticks
/// after boot), which tells it apart from a later process given the same pid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Proc {
    pub pid: i32,
    pub ppid: i32,
    pub pgid: i32,
    pub start: u64,
}

/// Every live (non-zombie) process this user owns.
pub(super) fn snapshot() -> Vec<Proc> {
    // SAFETY: geteuid has no preconditions and cannot fail.
    let uid = unsafe { libc::geteuid() };
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<i32>().ok())
        .filter(|pid| fs::metadata(format!("/proc/{pid}")).is_ok_and(|meta| meta.uid() == uid))
        .filter_map(read_stat)
        .collect()
}

/// `/proc/<pid>/stat`, or `None` for a process that is gone or a zombie.
fn read_stat(pid: i32) -> Option<Proc> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_stat(pid, &stat)
}

/// Parse a stat line. The command name is in parentheses and may itself
/// contain spaces and `)`, so the fields are read after the last `)`: state
/// (field 3), ppid (4), pgrp (5), ..., starttime (22).
fn parse_stat(pid: i32, stat: &str) -> Option<Proc> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let state = *fields.first()?;
    if state == "Z" || state == "X" {
        return None;
    }
    Some(Proc {
        pid,
        ppid: fields.get(1)?.parse().ok()?,
        pgid: fields.get(2)?.parse().ok()?,
        start: fields.get(19)?.parse().ok()?,
    })
}

/// The start time of `pid` while it exists and is not a zombie.
pub(super) fn start_time(pid: i32) -> Option<u64> {
    read_stat(pid).map(|p| p.start)
}

/// `roots`, every process descended from one of them, and every member of
/// those processes' groups. A root's own pid counts as a group too: a runner
/// is its group's leader, and its group outlives it once it has exited.
pub(super) fn tree(procs: &[Proc], roots: &BTreeSet<i32>) -> BTreeSet<i32> {
    let mut found = roots.clone();
    loop {
        let groups: BTreeSet<i32> = procs
            .iter()
            .filter(|p| found.contains(&p.pid))
            .map(|p| p.pgid)
            .chain(roots.iter().copied())
            .collect();
        let before = found.len();
        for p in procs {
            if found.contains(&p.ppid) || groups.contains(&p.pgid) {
                found.insert(p.pid);
            }
        }
        if found.len() == before {
            return found;
        }
    }
}

/// The members of the process group `pgid` other than a process whose pid is
/// `pgid` itself, and everything under them. For a group whose leader has
/// exited and been reaped: Linux does not reuse a pid while a group with that
/// id still has members, so as long as there are members, nothing here can be
/// a stranger; and once there are none, a new process given the leader's old
/// pid is not taken for the group.
pub(super) fn group_tree(procs: &[Proc], pgid: i32) -> BTreeSet<i32> {
    let members: BTreeSet<i32> = procs
        .iter()
        .filter(|p| p.pgid == pgid && p.pid != pgid)
        .map(|p| p.pid)
        .collect();
    if members.is_empty() {
        return members;
    }
    tree(procs, &members)
}

/// The processes whose initial environment holds `name=value`.
pub(super) fn with_env(procs: &[Proc], name: &str, value: &str) -> BTreeSet<i32> {
    let wanted = format!("{name}={value}").into_bytes();
    procs
        .iter()
        .filter(|p| {
            fs::read(format!("/proc/{}/environ", p.pid))
                .is_ok_and(|env| env.split(|b| *b == 0).any(|entry| entry == wanted))
        })
        .map(|p| p.pid)
        .collect()
}

/// This process and its ancestors, which must never be signalled.
pub(super) fn self_and_ancestors(procs: &[Proc]) -> BTreeSet<i32> {
    let mut found = BTreeSet::new();
    let mut pid = std::process::id() as i32;
    while pid > 1 && found.insert(pid) {
        match procs.iter().find(|p| p.pid == pid) {
            Some(p) => pid = p.ppid,
            None => break,
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: i32, ppid: i32, pgid: i32) -> Proc {
        Proc {
            pid,
            ppid,
            pgid,
            start: 0,
        }
    }

    #[test]
    fn stat_names_with_spaces_and_parens_parse() {
        let tail = "0 -1 4194304 1 0 0 0 0 0 0 0 20 0 1 0 123456 4096";
        let parsed = parse_stat(42, &format!("42 (a b) c)) S 7 42 42 {tail}")).unwrap();
        assert_eq!(
            parsed,
            Proc {
                pid: 42,
                ppid: 7,
                pgid: 42,
                start: 123456
            }
        );
        assert_eq!(parse_stat(9, &format!("9 (z) Z 1 9 9 {tail}")), None);
    }

    #[test]
    fn tree_follows_parents_and_groups() {
        let procs = [
            p(10, 1, 10),
            p(11, 10, 10),
            // Reparented to init but still in the runner's group.
            p(12, 1, 10),
            // Started its own group (setsid) under a member of the tree.
            p(13, 12, 13),
            p(20, 1, 20),
        ];
        let found = tree(&procs, &BTreeSet::from([10]));
        assert_eq!(found, BTreeSet::from([10, 11, 12, 13]));
    }

    #[test]
    fn a_reaped_leaders_group_is_found_through_its_members_only() {
        // Runner 10 exited and was reaped; 11 stays in its group and started
        // 12 in a session of its own.
        let procs = [p(11, 1, 10), p(12, 11, 12), p(20, 1, 20)];
        assert_eq!(group_tree(&procs, 10), BTreeSet::from([11, 12]));
        // The group is empty and pid 10 now belongs to a stranger that leads
        // a group of its own: nothing is found, the stranger included.
        let procs = [p(10, 1, 10), p(13, 10, 10), p(20, 1, 20)];
        assert_eq!(group_tree(&procs[..1], 10), BTreeSet::new());
        // (With a member left, pid 10 cannot have been reused.)
        assert_eq!(group_tree(&procs, 10), BTreeSet::from([10, 13]));
    }

    #[test]
    fn an_exited_leaders_group_is_still_found() {
        // Runner 10 has exited and been reaped; its group lives on.
        let procs = [p(11, 1, 10), p(12, 11, 12), p(20, 1, 20)];
        let found = tree(&procs, &BTreeSet::from([10]));
        assert_eq!(found, BTreeSet::from([10, 11, 12]));
    }

    #[test]
    fn this_process_is_found_with_its_start_time() {
        let procs = snapshot();
        let me = std::process::id() as i32;
        let mine = procs.iter().find(|p| p.pid == me).unwrap();
        assert_eq!(start_time(me), Some(mine.start));
        assert!(self_and_ancestors(&procs).contains(&me));
    }
}
