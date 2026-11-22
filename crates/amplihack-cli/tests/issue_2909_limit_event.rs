//! Issue #2909: a limit record makes the rotator rotate at once, and a
//! recorded reset wakes it. Every rotator runs with `--interval 3600` in a
//! temporary HOME, so a swap within the test's ceiling can only have come
//! from the event, never from an interval tick. The "limit record" is
//! written by a stand-in that edits the rotate ledger directly; the step
//! that writes it for real belongs to #2911. No sleeps: `wait_until` only.

#![cfg(target_os = "linux")]

use crate::support::fake_proc::FakeProc;
use crate::support::profile_rotate::*;
use crate::support::wait::{output_within, wait_until};
use std::fs;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;

/// Kills the rotator named by the fixture's pid file if a test fails first.
struct Reaper(PathBuf, FakeProc);

impl Drop for Reaper {
    fn drop(&mut self) {
        if let Ok(s) = fs::read_to_string(&self.0)
            && let Ok(pid) = s.trim().parse::<i32>()
            && pid > 1
        {
            let cmdline = fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            if String::from_utf8_lossy(&cmdline).contains("rotate") {
                // SAFETY: plain kill(2) of a process this test started.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Starts a rotator that waits up to an hour between ticks.
fn start_long(f: &Fixture) -> (Reaper, i32) {
    start_every(f, "3600")
}

/// Starts a rotator that waits up to `interval` seconds between ticks.
fn start_every(f: &Fixture, interval: &str) -> (Reaper, i32) {
    let reaper = Reaper(f.rotate_dir().join("rotate.pid"), FakeProc::new());
    f.with_proc(&reaper.1);
    let out = output_within(
        "`profile rotate start --interval <n>`",
        &mut f.cmd(&[
            "profile",
            "rotate",
            "start",
            "--interval",
            interval,
            "--hold",
            "0",
        ]),
    );
    assert_no_secrets(&out);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let pid: i32 = fs::read_to_string(&reaper.0)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    reaper.1.link_own(pid as u32);
    (reaper, pid)
}

/// The stand-in recorder, writing as `rotate/step_block.rs` does: under the
/// rotate lock, so it never races a cycle's own read-modify-write of the
/// ledger, it merges one block and renames the file into place.
///
/// Without the lock a record written while a cycle runs can be overwritten
/// by that cycle's save and lost outright, which no reader can recover.
/// With it, a record written while a cycle measures is kept by the cycle's
/// save, and the rotator must still act on it although the file is then
/// its own write; that is the case this test met first.
fn record_block(f: &Fixture, login: &str, reason: &str, at: i64, until: i64) {
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(f.rotate_dir().parent().unwrap().join("lock"))
        .unwrap();
    wait_until("the rotate lock", || {
        // SAFETY: flock(2) on a descriptor this test owns; LOCK_NB never
        // blocks. The lock is released when `lock` is dropped below.
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
    });
    let path = f.rotate_dir().join("ledger.json");
    let mut v = f
        .ledger()
        .unwrap_or_else(|| serde_json::json!({"version": 1}));
    v["overflow_blocked"][login] = serde_json::json!({"at": at, "until": until, "reason": reason});
    let tmp = f.rotate_dir().join("ledger.json.stand-in");
    fs::write(&tmp, v.to_string()).unwrap();
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600)).unwrap();
    fs::rename(&tmp, &path).unwrap();
    drop(lock);
}

fn cycles(f: &Fixture) -> usize {
    f.log().lines().filter(|l| l.contains(" cycle ")).count()
}

/// The byte offset of the first log line containing `needle`.
fn first(log: &str, needle: &str) -> Option<usize> {
    log.find(needle)
}

#[test]
fn a_limit_record_makes_the_rotator_swap_inside_its_interval() {
    let f = Fixture::new();
    f.usage("work", 10, 10);
    let (_r, pid) = start_long(&f);
    wait_until("the first cycle", || {
        f.log().lines().any(|l| l.contains(" cycle live=work"))
    });
    assert!(!f.log().contains(" swap "), "work has room: {}", f.log());
    // The login runs out and a step records the limit.
    f.usage("work", 100, 40);
    record_block(&f, "work", "session_limit", now(), now() + 3 * 3600);
    wait_until("a swap off work caused by the record", || {
        f.log()
            .lines()
            .any(|l| l.contains(" swap ") && l.contains("work->"))
    });
    let log = f.log();
    // spare (10/20) has more room than extra (50/50), so it is the target.
    assert!(log.contains(" swap work->spare live "), "{log}");
    // With `--interval 3600` the only way to a second cycle inside the
    // test's ceiling is a wake, and the wake the log names is the record.
    let woke = first(&log, "wake: ledger changed").expect("the record woke the rotator");
    assert!(
        Some(woke) < first(&log, " swap work->"),
        "the wake comes before the swap it caused:\n{log}"
    );
    assert!(!log.contains("wake: interval"), "no interval tick: {log}");
    // Under `--nocapture` the log is the evidence an operator would read.
    println!("[#2909 record] {}", log.replace('\n', "\n[#2909 record] "));
    for l in log.lines() {
        assert_log_grammar(l, true);
    }
    let (out, refused) = f.stop_until_gone(pid);
    f.assert_stopped(&out, refused, pid);
}

#[test]
fn a_recorded_reset_wakes_the_rotator_and_frees_the_login() {
    let f = Fixture::new();
    // work and extra are spent and spare is blocked until a moment from
    // now: no target at first, then the reset passes with no interval tick.
    f.usage("work", 100, 40);
    f.usage("extra", 100, 40);
    fs::create_dir_all(f.rotate_dir()).unwrap();
    fs::set_permissions(f.rotate_dir(), fs::Permissions::from_mode(0o700)).unwrap();
    record_block(&f, "spare", "week_limit", now() - 10, now() + 4);
    let (_r, pid) = start_long(&f);
    wait_until("a swap to spare after its reset", || {
        f.log()
            .lines()
            .any(|l| l.contains(" swap ") && l.contains("->spare live "))
    });
    let (out, refused) = f.stop_until_gone(pid);
    f.assert_stopped(&out, refused, pid);
    let log = f.log();
    assert!(cycles(&f) >= 2, "{log}");
    let woke = first(&log, "wake: reset due").expect("the reset woke the rotator");
    assert!(
        Some(woke) < first(&log, "->spare live "),
        "the reset wake comes before the swap onto spare:\n{log}"
    );
    // Before its reset spare is no candidate at all: every cycle until the
    // wake found no login with capacity, and none of them moved onto spare.
    assert!(
        first(&log, "ALERT: no profile has capacity").is_some_and(|a| a < woke),
        "before its reset the block left no login with capacity:\n{log}"
    );
    println!("[#2909 reset] {}", log.replace('\n', "\n[#2909 reset] "));
    for l in log.lines() {
        assert_log_grammar(l, true);
    }
}

#[test]
fn stop_finishes_while_the_rotator_waits_out_a_long_interval() {
    let f = Fixture::new();
    f.usage("work", 10, 10);
    let (_r, pid) = start_long(&f);
    wait_until("the first cycle", || cycles(&f) >= 1);
    let out = output_within(
        "`profile rotate stop`",
        &mut f.cmd(&["profile", "rotate", "stop"]),
    );
    assert_no_secrets(&out);
    assert!(
        out.status.success(),
        "stop must not be refused during a long wait: {}",
        text(&out.stderr)
    );
    wait_until("the rotator to exit", || !alive(pid));
}

/// Any limit kind is an event and a block (#2909), in a running rotator:
/// a `credit_limit` record for the live login moves it at once, and a
/// candidate held by a kind this binary does not know is passed over until
/// its reset. The log names that kind by fixed text and never quotes the
/// stored token, which a newer writer chose.
#[test]
fn a_running_rotator_honours_a_credit_limit_and_an_unknown_limit_kind() {
    let f = Fixture::new();
    f.usage("work", 10, 10);
    fs::create_dir_all(f.rotate_dir()).unwrap();
    fs::set_permissions(f.rotate_dir(), fs::Permissions::from_mode(0o700)).unwrap();
    let spare_until = now() + 3 * 3600;
    record_block(&f, "spare", "org_monthly_cap", now() - 60, spare_until);
    let (_r, pid) = start_long(&f);
    wait_until("the first cycle", || cycles(&f) >= 1);
    assert!(!f.log().contains(" swap "), "work has room: {}", f.log());
    // The live login meets its credit limit and a step records it.
    f.usage("work", 100, 40);
    record_block(&f, "work", "credit_limit", now(), now() + 3 * 3600);
    wait_until("a swap off work caused by the credit-limit record", || {
        f.log()
            .lines()
            .any(|l| l.contains(" swap ") && l.contains("work->"))
    });
    let (out, refused) = f.stop_until_gone(pid);
    f.assert_stopped(&out, refused, pid);
    let log = f.log();
    // spare (10/20) has the most room but is held; extra (50/50) is chosen.
    assert!(log.contains(" swap work->extra live "), "{log}");
    let woke = first(&log, "wake: ledger changed").expect("the record woke the rotator");
    assert!(Some(woke) < first(&log, " swap work->"), "{log}");
    assert!(!log.contains("wake: interval"), "no interval tick: {log}");
    // spare is held by the unknown kind until `spare_until`, which is hours
    // away, so no cycle in this test moved onto it.
    assert!(
        spare_until > now() + 3600,
        "spare's block is still in force"
    );
    assert!(
        !log.contains("->spare "),
        "a login held by a recorded limit is never a target: {log}"
    );
    assert!(
        !log.contains("org_monthly_cap"),
        "a stored kind is never quoted: {log}"
    );
    // Both records are kept until their resets.
    let ledger = f.ledger().expect("the ledger");
    assert_eq!(
        ledger["overflow_blocked"]["spare"]["reason"],
        "org_monthly_cap"
    );
    assert_eq!(ledger["overflow_blocked"]["work"]["reason"], "credit_limit");
    println!("[#2909 kinds] {}", log.replace('\n', "\n[#2909 kinds] "));
    for l in log.lines() {
        assert_log_grammar(l, true);
    }
}

/// A ledger write that leaves the recorded blocks as they were starts no
/// cycle, whoever wrote it and whatever its bytes. Only a change in the
/// blocks does, which is what bounds the extra cycles events can cause
/// without a wall-clock floor: a second record of a limit already in force
/// changes nothing.
///
/// A stand-in writer records spare's block again three times, compact
/// where the rotator writes pretty JSON, so every byte may differ from the
/// rotator's own save; only then does work run out and its limit get
/// recorded. Exactly one wake and one cycle follow the first, and they are
/// the new block's.
#[test]
fn a_ledger_write_that_changes_no_block_starts_no_cycle() {
    let f = Fixture::new();
    f.usage("work", 10, 10);
    fs::create_dir_all(f.rotate_dir()).unwrap();
    fs::set_permissions(f.rotate_dir(), fs::Permissions::from_mode(0o700)).unwrap();
    let (at, until) = (now() - 60, now() + 3 * 3600);
    record_block(&f, "spare", "org_monthly_cap", at, until);
    let (_r, pid) = start_long(&f);
    wait_until("the first cycle", || measured(&f) >= 1);
    // The first cycle's own save, so the rewrites below meet a rotator that
    // has finished its cycle and is waiting, or about to.
    wait_until("the first cycle's save", || {
        f.ledger().is_some_and(|l| l.get("last_cycle").is_some())
    });
    for _ in 0..3 {
        record_block(&f, "spare", "org_monthly_cap", at, until);
    }
    f.usage("work", 100, 40);
    record_block(&f, "work", "session_limit", now(), now() + 3 * 3600);
    wait_until("a swap off work caused by the new block", || {
        f.log()
            .lines()
            .any(|l| l.contains(" swap ") && l.contains("work->"))
    });
    let (out, refused) = f.stop_until_gone(pid);
    f.assert_stopped(&out, refused, pid);
    let log = f.log();
    assert_eq!(
        log.matches("wake: ledger changed").count(),
        1,
        "only the new block woke the rotator:\n{log}"
    );
    assert_eq!(
        measured(&f),
        2,
        "the first cycle and the new block's, and no other:\n{log}"
    );
    assert!(log.contains(" swap work->extra live "), "{log}");
    assert!(!log.contains("wake: interval"), "no interval tick: {log}");
    println!("[#2909 same] {}", log.replace('\n', "\n[#2909 same] "));
    for l in log.lines() {
        assert_log_grammar(l, true);
    }
}

/// The cycles that measured, by their `cycle live=` lines.
fn measured(f: &Fixture) -> usize {
    f.log()
        .lines()
        .filter(|l| l.contains(" cycle live="))
        .count()
}

/// The `/usage` reads made so far of work, spare and extra.
fn reads(f: &Fixture) -> [usize; 3] {
    ["work", "spare", "extra"].map(|p| f.usage_probes(p))
}

/// Every login reports when its windows reset, which a kept reading needs
/// before it can stand for its login (`cycle::event_read`).
fn readings_with_resets(f: &Fixture) {
    f.usage_resetting("work", 10, 10, 3);
    f.usage_resetting("spare", 10, 20, 3);
    f.usage_resetting("extra", 50, 50, 3);
}

/// Issue #2909, "Wanted" item 3: fewer probes. A cycle woken by a limit
/// record reads the live login, takes the other logins' figures from the
/// readings the first cycle kept, and reads the target it chose once before
/// the swap. extra, which it did not choose, is not read again.
#[test]
fn an_event_cycle_reads_the_live_login_and_the_chosen_target_only() {
    let f = Fixture::new();
    readings_with_resets(&f);
    let (_r, pid) = start_long(&f);
    wait_until("the first cycle's save", || {
        f.ledger().is_some_and(|l| l.get("last_cycle").is_some())
    });
    let before = reads(&f);
    assert!(
        before.iter().all(|n| *n >= 1),
        "the first cycle reads every login: {before:?}\n{}",
        f.log()
    );
    assert!(!f.log().contains(" swap "), "work has room: {}", f.log());
    // The login runs out and a step records the limit.
    f.usage_resetting("work", 100, 40, 3);
    record_block(&f, "work", "session_limit", now(), now() + 3 * 3600);
    wait_until("a swap off work caused by the record", || {
        f.log()
            .lines()
            .any(|l| l.contains(" swap ") && l.contains("work->"))
    });
    let (out, refused) = f.stop_until_gone(pid);
    f.assert_stopped(&out, refused, pid);
    let after = reads(&f);
    let log = f.log();
    assert!(log.contains(" swap work->spare live "), "{log}");
    assert_eq!(after[0], before[0] + 1, "work, live, is read once:\n{log}");
    assert_eq!(
        after[1],
        before[1] + 1,
        "spare, the target, is read once before the swap:\n{log}"
    );
    assert_eq!(after[2], before[2], "extra is not read again:\n{log}");
    // The record woke the rotator, the cycle said what it read, the target
    // was read, and only then did the swap happen.
    let at = |needle: &str| first(&log, needle).unwrap_or_else(|| panic!("{needle}:\n{log}"));
    let woke = at("wake: ledger changed");
    let narrowed = at("wake: read 1 of 3 logins, the others from their last reading");
    let confirmed = at("reread: spare reading stale");
    let swap = at(" swap work->spare ");
    assert!(
        woke < narrowed && narrowed < confirmed && confirmed < swap,
        "{log}"
    );
    assert_eq!(measured(&f), 2, "the first cycle and the record's:\n{log}");
    println!("[#2909 reads] {}", log.replace('\n', "\n[#2909 reads] "));
    for l in log.lines() {
        assert_log_grammar(l, true);
    }
}

/// Issue #2909, item 3, the other side: a cycle woken by `--interval` reads
/// every login, as before, and writes no `wake: read` line. With no record
/// at all, every cycle after the first is an interval cycle.
#[test]
fn an_interval_cycle_reads_every_login() {
    let f = Fixture::new();
    readings_with_resets(&f);
    let (_r, pid) = start_every(&f, "3");
    wait_until("three cycles", || measured(&f) >= 3);
    let (out, refused) = f.stop_until_gone(pid);
    f.assert_stopped(&out, refused, pid);
    let log = f.log();
    assert!(log.contains("wake: interval"), "{log}");
    assert!(
        !log.contains("wake: read "),
        "an interval cycle takes no kept reading:\n{log}"
    );
    // A read still running when a cycle stops waiting is not started again
    // by the next, so three cycles start at least two reads of each login.
    let got = reads(&f);
    assert!(got.iter().all(|n| *n >= 2), "{got:?}\n{log}");
    println!(
        "[#2909 interval] {}",
        log.replace('\n', "\n[#2909 interval] ")
    );
    for l in log.lines() {
        assert_log_grammar(l, true);
    }
}
