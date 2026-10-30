//! Unit tests for the pure parts of a supervisor pass: batch scheduling with
//! a gap only between batches and the stop flag (R5.5, R5.9), the resume
//! order (R5.5, design 4.12), and the git command that counts dirty files
//! (S15, S17), inspected without running it. The pass itself is tested end
//! to end in `tests/issue_1760_agent_supervise*.rs`.
//!
//! API under test:
//! - `resume::schedule<T>(items: Vec<T>, batch: usize, gap: Duration,
//!   sleep: &dyn Fn(Duration, &AtomicBool) -> bool, stop: &AtomicBool,
//!   each: impl FnMut(T) -> ControlFlow<()>)`: checks `stop` before each
//!   item; sleeps `gap` between batches, never after the last; a sleeper
//!   returning `false` (stopped) or an item returning `Break` ends it
//! - `dirty::order(cands: Vec<(String, usize)>) -> Vec<String>`: count
//!   descending, then name in byte order
//! - `dirty::command(dir: &Path) -> std::process::Command`: not spawned here
//! - `dirty::count(stdout: &[u8]) -> usize`: lines of porcelain output

use super::dirty::*;
use super::resume::schedule;
use std::cell::RefCell;
use std::ffi::OsStr;
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const GAP: Duration = Duration::from_secs(7);

/// Runs `schedule` over `0..n` and returns the events: item numbers and
/// `sleep <secs>`.
fn events(n: usize, batch: usize, stop_after: Option<usize>, sleeper_ok: bool) -> Vec<String> {
    let log = RefCell::new(Vec::new());
    let stop = AtomicBool::new(false);
    let sleep = |d: Duration, _: &AtomicBool| {
        log.borrow_mut().push(format!("sleep {}", d.as_secs()));
        sleeper_ok
    };
    schedule((0..n).collect(), batch, GAP, &sleep, &stop, |i: usize| {
        log.borrow_mut().push(i.to_string());
        if stop_after == Some(i) {
            stop.store(true, Ordering::SeqCst);
        }
        ControlFlow::Continue(())
    });
    log.into_inner()
}

#[test]
fn batches_are_separated_by_the_gap() {
    assert_eq!(
        events(5, 2, None, true),
        ["0", "1", "sleep 7", "2", "3", "sleep 7", "4"]
    );
}

#[test]
fn no_gap_after_the_last_batch() {
    assert_eq!(events(4, 2, None, true), ["0", "1", "sleep 7", "2", "3"]);
    assert_eq!(events(3, 6, None, true), ["0", "1", "2"]);
    assert_eq!(events(2, 1, None, true), ["0", "sleep 7", "1"]);
    assert!(events(0, 2, None, true).is_empty());
}

#[test]
fn the_stop_flag_ends_the_schedule_between_items() {
    assert_eq!(events(5, 2, Some(0), true), ["0"]);
    assert_eq!(
        events(5, 2, Some(1), true),
        ["0", "1"],
        "no sleep once stopped"
    );
    let stop = AtomicBool::new(true);
    let mut ran = 0;
    schedule(
        vec![1, 2],
        1,
        GAP,
        &|_: Duration, _: &AtomicBool| true,
        &stop,
        |_: i32| {
            ran += 1;
            ControlFlow::Continue(())
        },
    );
    assert_eq!(ran, 0, "stopped before the first item");
}

#[test]
fn a_stopped_sleep_ends_the_schedule() {
    assert_eq!(events(5, 2, None, false), ["0", "1", "sleep 7"]);
}

/// Review issue 4: a failed resume is reported without sleeping first.
#[test]
fn a_break_ends_the_schedule_before_the_next_sleep() {
    let slept = RefCell::new(0);
    let sleep = |_: Duration, _: &AtomicBool| {
        *slept.borrow_mut() += 1;
        true
    };
    let mut ran = Vec::new();
    let stop = AtomicBool::new(false);
    schedule(vec![1, 2, 3], 1, GAP, &sleep, &stop, |i: i32| {
        ran.push(i);
        ControlFlow::Break(())
    });
    assert_eq!((ran, *slept.borrow()), (vec![1], 0));
}

#[test]
fn the_sleeper_gets_the_stop_flag() {
    let stop = AtomicBool::new(false);
    let seen = RefCell::new(false);
    let sleep = |_: Duration, s: &AtomicBool| {
        *seen.borrow_mut() = std::ptr::eq(s, &stop);
        true
    };
    schedule(vec![1, 2], 1, GAP, &sleep, &stop, |_: i32| {
        ControlFlow::Continue(())
    });
    assert!(*seen.borrow());
}

fn order_of(v: &[(&str, usize)]) -> Vec<String> {
    order(v.iter().map(|(n, c)| (n.to_string(), *c)).collect())
}

#[test]
fn dirtiest_first_then_name() {
    let got = order_of(&[
        ("b", 0),
        ("a", 0),
        ("c", 3),
        ("B", 3),
        ("a-2", 0),
        ("z", 10),
    ]);
    assert_eq!(got, ["z", "B", "c", "a", "a-2", "b"]);
    assert!(order_of(&[]).is_empty());
    assert_eq!(order_of(&[("x", 1)]), ["x"]);
}

#[test]
fn porcelain_lines_are_counted() {
    assert_eq!(count(b""), 0);
    assert_eq!(count(b" M a\n"), 1);
    assert_eq!(count(b" M a\nM  b\nR  c -> d\n"), 3);
    assert_eq!(
        count(b" M a\n M b"),
        2,
        "a last line without a newline counts"
    );
}

#[test]
fn the_git_command_s17() {
    let dir = Path::new("/srv/work tree");
    let c = command(dir);
    assert_eq!(c.get_program(), OsStr::new("git"));
    let args: Vec<&OsStr> = c.get_args().collect();
    let want = [
        "-c",
        "core.fsmonitor=false",
        "-C",
        "/srv/work tree",
        "status",
        "--porcelain",
        "--untracked-files=no",
    ];
    assert_eq!(args, want.map(OsStr::new));
    let envs: Vec<(&OsStr, Option<&OsStr>)> = c.get_envs().collect();
    let has = |k: &str, v: Option<&str>| envs.contains(&(OsStr::new(k), v.map(OsStr::new)));
    assert!(has("GIT_OPTIONAL_LOCKS", Some("0")));
    assert!(has("GIT_TERMINAL_PROMPT", Some("0")));
    for removed in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
    ] {
        assert!(has(removed, None), "{removed} must be removed");
    }
}
