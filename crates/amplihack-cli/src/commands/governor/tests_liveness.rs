//! Issue #1811 (TDD): dead-PID reclaim (D3).
//!
//! Wire with `#[cfg(all(test, unix))] mod tests_liveness;` in
//! `governor/mod.rs`.
//!
//! Expected API in `liveness`:
//! - `parse_stat_start_time(&str) -> Option<u64>`: field 22 of
//!   `/proc/<pid>/stat`, counted after the last `)`; input over 4 KiB is
//!   refused.
//! - `start_time(pid: u32) -> Option<u64>` (Linux; `None` elsewhere).
//! - `alive(pid: u32, start_time: Option<u64>) -> bool`: `false` for PIDs
//!   outside `2..=i32::MAX`, for `EPERM`, and when the start time differs.
//! - `reclaim(&mut Vec<Slot>, &mut Vec<QueueEntry>) -> usize`: removes the
//!   records of dead processes; never signals anything but 0.

use super::liveness::{alive, parse_stat_start_time, reclaim, same_process, start_time};
use super::model::{Class, QueueEntry, Slot};

fn dead_pid() -> u32 {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

fn me() -> (u32, Option<u64>) {
    let pid = std::process::id();
    (pid, start_time(pid))
}

fn slot(pid: u32, start: Option<u64>) -> Slot {
    Slot {
        pid,
        start_time: start,
        workstream: "ws".into(),
        class: Class::Ordinary,
        tree_id: format!("t-{pid}"),
        admitted_at: 1_800_000_000,
        nested_children: 0,
    }
}

fn queued(seq: u64, pid: u32, start: Option<u64>) -> QueueEntry {
    QueueEntry {
        seq,
        workstream: "ws".into(),
        class: Class::Ordinary,
        pid,
        start_time: start,
        enqueued_at: 1_800_000_000,
    }
}

#[test]
fn stat_parsing_uses_the_text_after_the_last_paren() {
    // comm may contain ") (" and spaces; after the last ")" come field 3
    // (state), fields 4 to 21, then field 22 (start time).
    let tail = (4..=21)
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    let stat = format!("123 (evil) (name x) S {tail} 987654 23 24");
    assert_eq!(parse_stat_start_time(&format!("{stat}\n")), Some(987_654));
}

#[test]
fn stat_parsing_refuses_malformed_or_oversized_input() {
    assert_eq!(parse_stat_start_time(""), None);
    assert_eq!(parse_stat_start_time("123 (no close paren S 1 2"), None);
    assert_eq!(parse_stat_start_time("123 (x) S 1 2 3"), None);
    let huge = format!("1 (x) S {}", "1 ".repeat(4096));
    assert_eq!(parse_stat_start_time(&huge), None);
    let tail = (4..=21)
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(
        parse_stat_start_time(&format!("1 (x) S {tail} notanumber")),
        None
    );
}

#[cfg(target_os = "linux")]
#[test]
fn own_start_time_is_readable() {
    assert!(me().1.is_some());
}

#[test]
fn this_process_is_alive() {
    let (pid, start) = me();
    assert!(alive(pid, start));
}

#[test]
fn exited_process_is_dead() {
    assert!(!alive(dead_pid(), None));
}

#[cfg(target_os = "linux")]
#[test]
fn pid_reuse_is_caught_by_start_time() {
    let (pid, start) = me();
    assert!(!alive(pid, start.map(|s| s + 1)));
}

#[test]
fn out_of_range_pids_are_never_signalled_and_count_as_dead() {
    for pid in [0, 1, u32::MAX, i32::MAX as u32 + 1] {
        assert!(!alive(pid, None), "{pid}");
    }
}

#[test]
fn reclaim_removes_dead_slots_and_queue_entries_only() {
    let (pid, start) = me();
    let dead = dead_pid();
    let mut slots = vec![slot(pid, start), slot(dead, None), slot(1, None)];
    let mut queue = vec![queued(1, dead, None), queued(2, pid, start)];
    let removed = reclaim(&mut slots, &mut queue);
    assert_eq!(removed, 3);
    assert_eq!(slots.iter().map(|s| s.pid).collect::<Vec<_>>(), [pid]);
    assert_eq!(queue.iter().map(|q| q.seq).collect::<Vec<_>>(), [2]);
}

#[test]
fn same_process_covers_every_combination() {
    assert!(same_process(Some(7), Some(7)));
    assert!(!same_process(Some(7), Some(8)));
    // A recorded start time that cannot be read again counts as dead (S2.3).
    assert!(!same_process(Some(7), None));
    assert!(same_process(None, Some(7)));
    assert!(same_process(None, None));
}
