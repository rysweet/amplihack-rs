//! Issue #1811 (TDD): the fair queue (D8).
//!
//! Wire with `#[cfg(test)] mod tests_queue;` in `governor/mod.rs`.
//!
//! Expected API in `queue`:
//! - `order_key(&QueueEntry, &[Admission]) -> (u8, usize, u64)`: class rank,
//!   admissions to the entry's workstream in the window, seq.
//! - `head(&[QueueEntry], &[Admission]) -> Option<u64>`: the seq of the
//!   entry with the lowest key.
//! - `prune_window(&mut Vec<Admission>, now)`: drops admissions older than
//!   24 hours and keeps at most the newest 1024.
//! - `Class::rank()`: coordination 0, recovery 1, review 2, ordinary 3.

use super::model::{Admission, Class, QueueEntry};
use super::queue::{head, order_key, prune_window};

const NOW: i64 = 1_800_000_000;
const DAY: i64 = 86_400;

fn q(seq: u64, workstream: &str, class: Class) -> QueueEntry {
    QueueEntry {
        seq,
        workstream: workstream.to_string(),
        class,
        pid: 100 + seq as u32,
        start_time: Some(1),
        enqueued_at: NOW,
    }
}

fn adm(workstream: &str, at: i64) -> Admission {
    Admission {
        workstream: workstream.to_string(),
        at,
    }
}

#[test]
fn class_ranks_are_fixed() {
    assert_eq!(Class::Coordination.rank(), 0);
    assert_eq!(Class::Recovery.rank(), 1);
    assert_eq!(Class::Review.rank(), 2);
    assert_eq!(Class::Ordinary.rank(), 3);
}

#[test]
fn order_key_counts_admissions_to_the_workstream() {
    let admissions = [adm("a", NOW - 10), adm("a", NOW - 5), adm("b", NOW - 1)];
    assert_eq!(order_key(&q(7, "a", Class::Review), &admissions), (2, 2, 7));
    assert_eq!(
        order_key(&q(8, "c", Class::Ordinary), &admissions),
        (3, 0, 8)
    );
}

#[test]
fn head_of_an_empty_queue_is_none() {
    assert_eq!(head(&[], &[]), None);
}

#[test]
fn class_rank_comes_before_fairness_and_seq() {
    let entries = [q(1, "a", Class::Ordinary), q(9, "a", Class::Coordination)];
    let admissions = vec![adm("a", NOW - 1); 50];
    assert_eq!(head(&entries, &admissions), Some(9));
}

#[test]
fn fifo_within_one_class_and_workstream() {
    let entries = [q(3, "a", Class::Ordinary), q(2, "a", Class::Ordinary)];
    assert_eq!(head(&entries, &[]), Some(2));
}

#[test]
fn two_workstreams_alternate_with_limit_one() {
    let mut entries = vec![
        q(1, "A", Class::Ordinary),
        q(2, "A", Class::Ordinary),
        q(3, "A", Class::Ordinary),
        q(4, "B", Class::Ordinary),
        q(5, "B", Class::Ordinary),
    ];
    let mut admissions = Vec::new();
    let mut order = Vec::new();
    while let Some(seq) = head(&entries, &admissions) {
        let i = entries.iter().position(|e| e.seq == seq).unwrap();
        let e = entries.remove(i);
        admissions.push(adm(&e.workstream, NOW));
        order.push(e.workstream);
    }
    assert_eq!(order, ["A", "B", "A", "B", "A"]);
}

#[test]
fn prune_window_drops_admissions_older_than_a_day() {
    let mut a = vec![
        adm("a", NOW - DAY - 1),
        adm("b", NOW - DAY + 1),
        adm("c", NOW),
    ];
    prune_window(&mut a, NOW);
    let names: Vec<&str> = a.iter().map(|x| x.workstream.as_str()).collect();
    assert_eq!(names, ["b", "c"]);
}

#[test]
fn prune_window_keeps_at_most_the_newest_1024() {
    let mut a: Vec<Admission> = (0..1500)
        .map(|i| adm(&format!("w{i}"), NOW - 1500 + i))
        .collect();
    prune_window(&mut a, NOW);
    assert_eq!(a.len(), 1024);
    assert!(a.iter().all(|x| x.at >= NOW - 1024));
}
