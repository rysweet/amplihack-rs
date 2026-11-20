//! Issue #2040 (D10): the whole-run rotate lock,
//! `<root>/.auto-switch.d/rotate/rotate-run.lock`.
//!
//! Contract (see "Rotate run lock" in docs/reference/profile-command.md):
//! - `run_cycle` takes the lock as its first step, before the dry-run branch
//!   and the prototype check, with one non-blocking attempt, and holds it
//!   until it returns. The holder writes its pid into the file.
//! - Held by another run: a one-shot run and a dry run fail with
//!   `RotateRunning(pid)` and read nothing; a `watch` cycle logs
//!   `msg::RUN_BUSY` and ends `Skipped`.
//! - The pid is for display only: at most 32 bytes, trimmed, parsed as a
//!   `u32`; `0` or anything unparsable is `None` ("pid unknown"), and the
//!   raw bytes never reach a message.
//! - A lock file that fails a check (symlink, not a regular file, not
//!   yours, open to others, more than one link) is never repaired: a
//!   one-shot or dry run fails with `RotateRefused("rotate run lock
//!   <reason>; nothing was read")`, and a `watch` cycle logs
//!   `skip: rotate run lock <reason>` and ends `Skipped`.
//! - The file is opened `O_CLOEXEC`: a child does not keep the lock.
//!
//! Contention is a real `flock` on a second descriptor, in this process.
//! Running sessions come from `ProcSource::from_fn`, counted per test; no
//! `/proc` is read and nothing waits on a clock.

use super::super::cycle::{Ctx, CycleOutcome, Mode, run_cycle};
use super::super::log::msg;
use super::super::tests_handfix_cycle::never;
use super::super::tests_revalidate_cause::ctx_with;
use super::super::tests_swap::{Env, env};
use super::{self as run_lock, Taken};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::Liveness;
use crate::commands::profile::proc_snapshot::ProcSource;
use fs2::FileExt;
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const BUSY_LINE: &str = "skip: another profile rotate run holds the run lock";

/// A context whose running-session reads are counted and must not happen.
fn counted_ctx(e: &Env) -> (Ctx, Arc<AtomicUsize>) {
    let scans = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&scans);
    let mut ctx = ctx_with(e, never);
    ctx.procs = ProcSource::from_fn(move |_| {
        c.fetch_add(1, Ordering::SeqCst);
        Liveness::Known(BTreeSet::new())
    });
    (ctx, scans)
}

/// Holds the run lock on a second descriptor, as another rotate run would,
/// with `pid` as the file's contents.
fn hold(e: &Env, pid: &[u8]) -> File {
    let mut f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&e.rs.run_lock)
        .unwrap();
    f.write_all(pid).unwrap();
    f.try_lock_exclusive()
        .expect("the test takes the run lock first");
    f
}

fn log_lines(e: &Env) -> Vec<String> {
    fs::read_to_string(&e.rs.log)
        .unwrap_or_default()
        .lines()
        .map(|l| l.split_once(' ').map_or(l, |(_, rest)| rest).to_string())
        .collect()
}

fn ledger_bytes(e: &Env) -> Vec<u8> {
    fs::read(&e.rs.ledger).unwrap()
}

#[test]
fn the_run_lock_lives_beside_the_rotate_state() {
    let e = env();
    assert_eq!(e.rs.run_lock, e.rs.dir.join("rotate-run.lock"));
    assert!(
        e.rs.run_lock
            .ends_with(".auto-switch.d/rotate/rotate-run.lock")
    );
}

#[test]
fn the_busy_line_and_messages_are_the_documented_ones() {
    assert_eq!(msg::RUN_BUSY, BUSY_LINE);
    assert_eq!(
        ProfileError::RotateRunning(Some(4711)).to_string(),
        "another profile rotate is running (pid 4711); not starting a second scan"
    );
    assert_eq!(
        ProfileError::RotateRunning(None).to_string(),
        "another profile rotate is running (pid unknown); not starting a second scan"
    );
    assert_eq!(
        ProfileError::RotateRunning(None).hint(),
        "wait for that run to finish, or stop the rotator with `amplihack profile rotate stop`"
    );
}

// --- AC3: a second run reads nothing -----------------------------------------

#[cfg(target_os = "linux")]
#[test]
fn ac3_a_one_shot_run_fails_with_rotate_running_and_reads_nothing() {
    let e = env();
    let before = ledger_bytes(&e);
    let _held = hold(&e, b"12345\n");
    let (mut ctx, scans) = counted_ctx(&e);
    let err = run_cycle(&mut ctx, Mode::OneShot).unwrap_err();
    assert!(
        matches!(err, ProfileError::RotateRunning(Some(12345))),
        "{err:?}"
    );
    assert_eq!(scans.load(Ordering::SeqCst), 0, "no /proc read");
    assert_eq!(ledger_bytes(&e), before, "nothing written");
    assert_eq!(ino_live(&e), Some(e.l), "nothing moved");
}

#[cfg(target_os = "linux")]
#[test]
fn ac3_a_dry_run_fails_with_rotate_running_and_reads_nothing() {
    let e = env();
    let before = ledger_bytes(&e);
    let _held = hold(&e, b"12345");
    let (mut ctx, scans) = counted_ctx(&e);
    let err = run_cycle(&mut ctx, Mode::DryRun).unwrap_err();
    assert!(
        matches!(err, ProfileError::RotateRunning(Some(12345))),
        "{err:?}"
    );
    assert_eq!(scans.load(Ordering::SeqCst), 0);
    assert_eq!(ledger_bytes(&e), before);
}

#[cfg(target_os = "linux")]
#[test]
fn ac3_a_watch_cycle_logs_the_busy_line_and_skips() {
    let e = env();
    let before = ledger_bytes(&e);
    let _held = hold(&e, b"12345");
    let (mut ctx, scans) = counted_ctx(&e);
    assert_eq!(
        run_cycle(&mut ctx, Mode::Watch).unwrap(),
        CycleOutcome::Skipped
    );
    assert_eq!(scans.load(Ordering::SeqCst), 0);
    assert_eq!(log_lines(&e), vec![BUSY_LINE.to_string()]);
    assert_eq!(ledger_bytes(&e), before);
    // The daemon keeps going: the next cycle, still blocked, says it again.
    assert_eq!(
        run_cycle(&mut ctx, Mode::Watch).unwrap(),
        CycleOutcome::Skipped
    );
    assert_eq!(log_lines(&e), vec![BUSY_LINE.to_string(); 2]);
}

fn ino_live(e: &Env) -> Option<u64> {
    super::super::tests_swap::ino_at(&e.live())
}

// --- take(): the holder's pid -------------------------------------------------

#[test]
fn take_writes_the_holders_pid_and_a_second_take_is_busy_until_it_is_dropped() {
    let e = env();
    let held = match run_lock::take(&e.rs).unwrap() {
        Taken::Held(l) => l,
        Taken::Busy(pid) => panic!("free lock reported busy ({pid:?})"),
        Taken::Refused(r) => panic!("a fresh lock file was refused ({r})"),
    };
    let me = std::process::id();
    assert_eq!(
        fs::read_to_string(&e.rs.run_lock).unwrap().trim(),
        me.to_string()
    );
    assert_eq!(
        fs::metadata(&e.rs.run_lock).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(matches!(run_lock::take(&e.rs).unwrap(), Taken::Busy(Some(p)) if p == me));
    drop(held);
    assert!(matches!(run_lock::take(&e.rs).unwrap(), Taken::Held(_)));
}

/// A child forked by another thread keeps a copy of the descriptor until it
/// execs; dropping the run lock must still free it.
#[test]
fn dropping_the_run_lock_frees_it_while_a_shared_descriptor_stays_open() {
    let e = env();
    let Taken::Held(held) = run_lock::take(&e.rs).unwrap() else {
        panic!("a free run lock was not taken");
    };
    let child_copy = held.file.try_clone().unwrap();
    drop(held);
    assert!(matches!(run_lock::take(&e.rs).unwrap(), Taken::Held(_)));
    drop(child_copy);
}

#[test]
fn a_previous_holders_longer_pid_is_replaced_not_appended_to() {
    let e = env();
    fs::write(&e.rs.run_lock, b"99999999\n").unwrap();
    fs::set_permissions(&e.rs.run_lock, fs::Permissions::from_mode(0o600)).unwrap();
    let _held = run_lock::take(&e.rs).unwrap();
    assert_eq!(
        fs::read_to_string(&e.rs.run_lock).unwrap().trim(),
        std::process::id().to_string()
    );
}

/// SR17: the pid is parsed strictly and never echoed raw.
#[test]
fn sr17_an_unreadable_pid_is_unknown_and_its_bytes_never_reach_the_message() {
    let forty = "1".repeat(40);
    let cases: &[(&[u8], Option<u32>)] = &[
        (b"", None),
        (b"abc", None),
        (b"0", None),
        (b"-5", None),
        (b"99999999999", None),
        (forty.as_bytes(), None),
        (b"12\x1b[31mred", None),
        (b"\xff\xfe", None),
        (b" 77 \n", Some(77)),
        (b"4294967295", Some(u32::MAX)),
    ];
    for (raw, want) in cases {
        let e = env();
        let _held = hold(&e, raw);
        let got = match run_lock::take(&e.rs).unwrap() {
            Taken::Busy(pid) => pid,
            Taken::Held(_) => panic!("{raw:?}: the lock is held"),
            Taken::Refused(r) => panic!("{raw:?}: refused ({r})"),
        };
        assert_eq!(got, *want, "{raw:?}");
        let shown = ProfileError::RotateRunning(got).to_string();
        if want.is_none() {
            assert!(shown.contains("(pid unknown)"), "{shown}");
            let raw_text = String::from_utf8_lossy(raw);
            let raw_text = raw_text.trim();
            if !raw_text.is_empty() && raw_text != "0" {
                assert!(!shown.contains(raw_text), "{shown} echoes {raw_text:?}");
            }
            assert!(!shown.contains('\u{1b}'), "{shown}");
        }
    }
}

// --- A lock file that fails a check -------------------------------------------

fn refused_reason(e: &Env) -> String {
    match run_lock::take(&e.rs) {
        Ok(Taken::Refused(r)) => match run_lock::refusal(r) {
            ProfileError::RotateRefused(m) => m,
            other => panic!("refusal() built {other:?}"),
        },
        Err(other) => panic!("expected a refusal, got {other:?}"),
        Ok(Taken::Held(_)) => panic!("an unsafe run lock file was accepted"),
        Ok(Taken::Busy(p)) => panic!("an unsafe run lock file was reported busy ({p:?})"),
    }
}

/// Each refusal: the reason word for word, for both kinds of run.
#[cfg(target_os = "linux")]
fn assert_refused(e: &Env, reason: &str) {
    let want = format!("rotate run lock {reason}; nothing was read");
    assert_eq!(refused_reason(e), want);
    for mode in [Mode::OneShot, Mode::DryRun] {
        let (mut ctx, scans) = counted_ctx(e);
        match run_cycle(&mut ctx, mode) {
            Err(ProfileError::RotateRefused(m)) => assert_eq!(m, want, "{mode:?}"),
            other => panic!("{mode:?}: {other:?}"),
        }
        assert_eq!(scans.load(Ordering::SeqCst), 0, "{mode:?}");
    }
    let (mut ctx, scans) = counted_ctx(e);
    assert_eq!(
        run_cycle(&mut ctx, Mode::Watch).unwrap(),
        CycleOutcome::Skipped
    );
    assert_eq!(scans.load(Ordering::SeqCst), 0);
    let line = format!("skip: rotate run lock {reason}");
    assert_eq!(log_lines(e).last(), Some(&line));
}

#[cfg(target_os = "linux")]
#[test]
fn a_symlinked_run_lock_is_refused_and_its_target_untouched() {
    let e = env();
    let target = e._tmp.path().join("elsewhere");
    fs::write(&target, b"keep").unwrap();
    symlink(&target, &e.rs.run_lock).unwrap();
    assert_refused(&e, "is a symlink");
    assert_eq!(fs::read(&target).unwrap(), b"keep");
    assert!(
        fs::symlink_metadata(&e.rs.run_lock)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_run_lock_that_is_not_a_regular_file_is_refused() {
    let e = env();
    fs::create_dir(&e.rs.run_lock).unwrap();
    assert_refused(&e, "is not a regular file");
}

#[cfg(target_os = "linux")]
#[test]
fn a_run_lock_open_to_others_is_refused_and_not_repaired() {
    for mode in [0o644, 0o660, 0o606] {
        let e = env();
        fs::write(&e.rs.run_lock, b"").unwrap();
        fs::set_permissions(&e.rs.run_lock, fs::Permissions::from_mode(mode)).unwrap();
        assert_refused(&e, "is open to group or other users");
        assert_eq!(
            fs::metadata(&e.rs.run_lock).unwrap().permissions().mode() & 0o777,
            mode,
            "never repaired"
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn a_hard_linked_run_lock_is_refused() {
    let e = env();
    fs::write(&e.rs.run_lock, b"").unwrap();
    fs::set_permissions(&e.rs.run_lock, fs::Permissions::from_mode(0o600)).unwrap();
    fs::hard_link(&e.rs.run_lock, e._tmp.path().join("second-name")).unwrap();
    assert_refused(&e, "has more than one link");
}

/// The five reasons are the documented ones, word for word.
#[test]
fn the_run_lock_reasons_are_documented() {
    const REFERENCE: &str = include_str!("../../../../../../docs/reference/profile-command.md");
    for reason in [
        "is a symlink",
        "is not a regular file",
        "is not owned by you",
        "is open to group or other users",
        "has more than one link",
    ] {
        assert!(
            REFERENCE.contains(&format!("`{reason}`")),
            "reference is missing `{reason}`"
        );
    }
    assert!(REFERENCE.contains("rotate run lock <reason>; nothing was read"));
    assert!(REFERENCE.contains("skip: rotate run lock <reason>"));
}

// --- SR15: a child does not keep the run lock ----------------------------------

/// The child is `cat` on a pipe, started while the lock is held; it ends
/// when the test closes the pipe, so nothing is signalled or timed.
#[test]
fn sr15_a_child_started_while_the_run_lock_is_held_does_not_keep_it() {
    let e = env();
    let held = match run_lock::take(&e.rs).unwrap() {
        Taken::Held(l) => l,
        Taken::Busy(_) => panic!("free lock reported busy"),
        Taken::Refused(r) => panic!("a fresh lock file was refused ({r})"),
    };
    let mut child = std::process::Command::new("cat")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    drop(held);
    let again = run_lock::take(&e.rs).unwrap();
    drop(child.stdin.take());
    child.wait().unwrap();
    assert!(
        matches!(again, Taken::Held(_)),
        "the child inherited the run lock"
    );
}
