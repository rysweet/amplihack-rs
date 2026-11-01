//! Issue #1691, design D3, D4, D7a and D-Release: what a switch hold does to
//! the rotator's own cycles, to the limit-resume hook's reading of the
//! ledger, and to the warning about a rotator from another amplihack build.
//!
//! In process, on the fixture in `tests_swap.rs` (`work` live, `spare`
//! stored). No `claude` binary is stubbed: the pin gate runs before
//! `resolve_claude_binary`, so a held cycle never looks for one. No daemon
//! is spawned; the "running rotator" in the D4 tests is a fake `/proc`
//! inside the temp dir. Nothing outside the temp dir is touched (#1690).
//!
//! Contract added by these tests (beyond `tests_switch.rs`):
//! - `switch_live` records `last_cycle = { outcome: "switched", at,
//!   measured_at }` with both times at the switch, and no alert.
//! - A held `run_cycle` leaves a `switched` last cycle in place; with no
//!   switch record (a pin written by hand) it records `"held"` with no
//!   `measured_at`. Either way it keeps the live profile in the ledger and
//!   writes the held line once per change of gate state.
//! - `amplihack_utils::rotator_status::parse_cycle` reads the ledger after a
//!   switch and any number of held cycles as fresh capacity for a limit hit
//!   before the switch and as none for one hit after it; a bare `held`
//!   ledger is no capacity, fresh or not (D3).
//! - `rotate::FOREIGN_IGNORES_HOLD` is the documented tail of the warning;
//!   `rotate::foreign_note_for(&RotateState, proc_root, exe)` returns
//!   `"amplihack: the Claude login rotator (pid {pid}) " + FOREIGN_IGNORES_HOLD`
//!   when a rotator of another binary runs, else `None` (D4).
//! - `release` takes the lock even with no pin, so it cannot interleave with
//!   a switch's pin and pointer writes (D-Release).

use super::cycle::{Ctx, CycleOutcome, Mode, run_cycle};
use super::ledger::SWITCHED;
use super::log::msg;
use super::pin::{self, PinState};
use super::settings;
use super::swap::DOCUMENTED;
use super::switch::{Switched, release, switch_live};
use super::tests_swap::{Env, NOW, env, hold_state_lock, ino_at, json, n};
use super::{FOREIGN_IGNORES_HOLD, foreign_note_for};
use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::pointer;
use amplihack_utils::rotator_status::parse_cycle;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn log(e: &Env) -> String {
    fs::read_to_string(&e.rs.log).unwrap_or_default()
}

fn count(haystack: &str, needle: &str) -> usize {
    haystack.lines().filter(|l| l.ends_with(needle)).count()
}

fn watch_ctx(e: &Env) -> Ctx {
    let s = settings::resolve(&super::TuneFlags::default(), None, &|_| None).unwrap();
    Ctx::new(e.store.clone(), e.home.clone(), s)
}

fn switch_to(e: &Env, name: &str) {
    match switch_live(&e.store, &e.home, &n(name)) {
        Ok(Ok(Switched::Moved { .. })) => {}
        other => panic!("switch {name} failed: {other:?}"),
    }
}

fn private_dir(p: &Path) {
    fs::create_dir_all(p).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
}

fn private_file(p: &Path, bytes: &[u8]) {
    fs::write(p, bytes).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
}

// --- D7a: watch cycles under a hold -----------------------------------------

#[cfg(target_os = "linux")]
#[test]
fn watch_cycles_under_a_switch_hold_stay_on_the_held_login() {
    let e = env();
    switch_to(&e, "spare");
    let held_ino = ino_at(&e.live());
    assert_eq!(held_ino, Some(e.t), "spare's login is live");
    let mut ctx = watch_ctx(&e);

    for i in 0..2 {
        assert_eq!(
            run_cycle(&mut ctx, Mode::Watch).unwrap(),
            CycleOutcome::Held,
            "cycle {i} is held"
        );
        assert_eq!(ino_at(&e.live()), held_ino, "cycle {i} moved the login");
        assert!(!e.rs.journal.exists(), "cycle {i} started a swap");
    }
    assert_eq!(pin::read(&e.rs), PinState::Pinned(n("spare")));
    assert_eq!(pointer::read(&e.root).unwrap(), Some(n("spare")));
}

#[cfg(target_os = "linux")]
#[test]
fn a_switch_records_itself_as_capacity_measured_now() {
    let e = env();
    // The rotator's last word before the switch: no capacity anywhere.
    let mut l = e.rs.load_ledger().unwrap().unwrap_or_default();
    l.alert = Some("no-capacity:work".into());
    e.rs.save_ledger(&l).unwrap();
    let before = now();
    switch_to(&e, "spare");

    let l = json(&e.rs.ledger);
    let last = &l["last_cycle"];
    assert_eq!(last["outcome"], SWITCHED);
    let at = last["at"].as_i64().unwrap();
    assert!(at >= before, "{last}");
    assert_eq!(last["measured_at"].as_i64(), Some(at), "{last}");
    assert!(
        l["alert"].is_null(),
        "a switch clears the no-capacity alert"
    );
    assert_eq!(l["live"], "spare");
}

#[cfg(target_os = "linux")]
#[test]
fn a_held_cycle_keeps_the_switch_record() {
    let e = env();
    switch_to(&e, "spare");
    let switched = json(&e.rs.ledger)["last_cycle"].clone();
    let mut ctx = watch_ctx(&e);
    for _ in 0..3 {
        assert_eq!(
            run_cycle(&mut ctx, Mode::Watch).unwrap(),
            CycleOutcome::Held
        );
    }
    let l = json(&e.rs.ledger);
    assert_eq!(l["last_cycle"], switched, "held cycles never replace it");
    assert_eq!(l["live"], "spare", "the held profile stays the live one");
    assert!(l["alert"].is_null(), "a hold is not an alert");
    assert!(l.get("pin").is_none(), "the pin is never a ledger field");
}

#[test]
fn a_held_cycle_without_a_switch_records_held_with_no_measurement() {
    let e = env();
    // A pin with no switch record: written by hand, or by an older build.
    pin::write(&e.rs, &n("work")).unwrap();
    let mut ctx = watch_ctx(&e);
    run_cycle(&mut ctx, Mode::Watch).unwrap();

    let last = &json(&e.rs.ledger)["last_cycle"];
    assert_eq!(last["outcome"], "held");
    assert!(last["at"].as_i64().is_some_and(|t| t > NOW));
    assert!(
        last.get("measured_at")
            .is_none_or(serde_json::Value::is_null),
        "a held cycle measured nothing, so it records no measured_at (#1659): {last}"
    );
}
#[cfg(target_os = "linux")]
#[test]
fn the_held_line_is_logged_once_across_watch_cycles() {
    let e = env();
    switch_to(&e, "spare");
    let mut ctx = watch_ctx(&e);
    for _ in 0..3 {
        run_cycle(&mut ctx, Mode::Watch).unwrap();
    }
    let line = msg::held(&n("spare"));
    assert_eq!(count(&log(&e), &line), 1, "log:\n{}", log(&e));
}

#[cfg(target_os = "linux")]
#[test]
fn a_changed_pin_is_logged_again_by_the_same_watch() {
    let e = env();
    switch_to(&e, "spare");
    let mut ctx = watch_ctx(&e);
    run_cycle(&mut ctx, Mode::Watch).unwrap();
    // Pin moved by hand to another existing profile: still held, new line.
    pin::write(&e.rs, &n("work")).unwrap();
    run_cycle(&mut ctx, Mode::Watch).unwrap();
    run_cycle(&mut ctx, Mode::Watch).unwrap();
    let log = log(&e);
    assert_eq!(count(&log, &msg::held(&n("spare"))), 1, "{log}");
    assert_eq!(count(&log, &msg::held(&n("work"))), 1, "{log}");
    assert_eq!(ino_at(&e.live()), Some(e.t), "a pin never swaps by itself");
}

#[test]
fn an_unreadable_pin_holds_watch_and_logs_once() {
    let e = env();
    private_file(&e.rs.pin, b"../../etc/passwd\n");
    let mut ctx = watch_ctx(&e);
    for _ in 0..2 {
        assert_eq!(
            run_cycle(&mut ctx, Mode::Watch).unwrap(),
            CycleOutcome::Held
        );
    }
    e.assert_unchanged();
    let log = log(&e);
    assert_eq!(count(&log, msg::PIN_UNREADABLE), 1, "{log}");
    assert!(!log.contains("passwd"), "pin bytes are never logged: {log}");
    assert_eq!(json(&e.rs.ledger)["last_cycle"]["outcome"], "held");
}

#[test]
fn a_group_writable_pin_is_unreadable_and_holds() {
    let e = env();
    pin::write(&e.rs, &n("spare")).unwrap();
    fs::set_permissions(&e.rs.pin, fs::Permissions::from_mode(0o620)).unwrap();
    assert_eq!(pin::read(&e.rs), PinState::Unreadable);
    let mut ctx = watch_ctx(&e);
    assert_eq!(
        run_cycle(&mut ctx, Mode::Watch).unwrap(),
        CycleOutcome::Held,
        "fails closed: a pin someone else may have written still stops swaps"
    );
    e.assert_unchanged();
}

// --- D3: the limit-resume hook wakes sessions stopped before a switch ------

/// The ledger as the limit-resume hook reads it.
fn hook_view(e: &Env) -> (amplihack_utils::rotator_status::CycleStatus, i64) {
    let raw = fs::read(&e.rs.ledger).unwrap();
    let now = json(&e.rs.ledger)["last_cycle"]["at"].as_i64().unwrap();
    let status = parse_cycle(&raw, now).expect("the hook reads the ledger the rotator wrote");
    (status, now)
}

#[cfg(target_os = "linux")]
#[test]
fn a_limit_before_the_switch_is_fresh_capacity_through_any_number_of_held_cycles() {
    // Issue #1691, 27 Oct: sessions stop on a limit, then the person
    // switches to a credit-backed login while the rotator keeps cycling.
    let e = env();
    let mut l = e.rs.load_ledger().unwrap().unwrap_or_default();
    l.alert = Some("no-capacity:work".into());
    l.last_cycle = Some(super::ledger::LastCycle {
        at: NOW,
        measured_at: Some(NOW),
        outcome: "alerted".into(),
        overflow: None,
    });
    e.rs.save_ledger(&l).unwrap();
    let (stopped, _) = hook_view(&e);
    let limit_hit = now() - 1;
    assert!(
        !stopped.fresh_capacity(limit_hit),
        "no capacity before the switch"
    );

    switch_to(&e, "spare");
    let switch_at = json(&e.rs.ledger)["last_cycle"]["measured_at"]
        .as_i64()
        .unwrap();
    let mut ctx = watch_ctx(&e);
    for i in 0..4 {
        assert_eq!(
            run_cycle(&mut ctx, Mode::Watch).unwrap(),
            CycleOutcome::Held
        );
        let (status, _) = hook_view(&e);
        assert_eq!(status.outcome.as_deref(), Some(SWITCHED));
        assert_eq!(status.live.as_deref(), Some("spare"));
        assert!(status.has_capacity(), "held cycle {i}");
        assert!(
            status.fresh_capacity(limit_hit),
            "a session stopped before the switch is woken after held cycle {i}"
        );
        assert!(
            !status.fresh_capacity(switch_at),
            "a session stopped on the switched login itself is not (cycle {i})"
        );
    }
}

#[test]
fn the_limit_resume_reader_sees_no_capacity_in_a_bare_held_cycle() {
    let e = env();
    pin::write(&e.rs, &n("work")).unwrap();
    let mut ctx = watch_ctx(&e);
    run_cycle(&mut ctx, Mode::Watch).unwrap();

    let (status, now) = hook_view(&e);
    assert_eq!(status.outcome.as_deref(), Some("held"));
    assert_eq!(status.measured_at, None);
    assert!(!status.has_capacity(), "a held cycle is not capacity");
    for start in [now - 3600, now - 1, now] {
        assert!(
            !status.fresh_capacity(start),
            "no wake from a hold with no switch (limit hit at {start})"
        );
    }
}

// --- D4: a rotator from another amplihack build ignores the hold -------------

const PID: u32 = 4242;
const WATCH: &[u8] = b"/home/u/.cargo/bin/amplihack\0profile\0rotate\0watch\0";

/// A `/proc` whose pid `PID` is a `profile rotate watch` running `exe`, and
/// the pid file naming it. Both inside the temp dir.
fn fake_rotator(e: &Env, exe: &Path) -> tempfile::TempDir {
    let proc_root = tempfile::tempdir().unwrap();
    let dir = proc_root.path().join(PID.to_string());
    private_dir(&dir);
    private_file(&dir.join("cmdline"), WATCH);
    fs::hard_link(exe, dir.join("exe")).unwrap();
    private_file(&e.rs.pid, format!("{PID}\n").as_bytes());
    proc_root
}

fn binaries() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let bins = tempfile::tempdir().unwrap();
    let (old, new) = (bins.path().join("old"), bins.path().join("new"));
    fs::write(&old, b"old build").unwrap();
    fs::write(&new, b"new build").unwrap();
    (bins, old, new)
}

#[test]
fn the_hold_warning_text_is_fixed() {
    assert_eq!(
        FOREIGN_IGNORES_HOLD,
        "runs another amplihack binary and ignores the switch hold; restart it: \
         amplihack profile rotate stop && amplihack profile rotate start"
    );
}

#[test]
fn the_hold_warning_is_a_documented_line() {
    assert_eq!(DOCUMENTED.len(), 12);
    assert!(DOCUMENTED.contains(&FOREIGN_IGNORES_HOLD));
}

#[test]
fn a_rotator_of_another_build_gets_the_hold_warning() {
    let e = env();
    let (_bins, old, new) = binaries();
    let proc_root = fake_rotator(&e, &old);
    assert_eq!(
        foreign_note_for(&e.rs, proc_root.path(), Some(&new)),
        Some(format!(
            "amplihack: the Claude login rotator (pid {PID}) {FOREIGN_IGNORES_HOLD}"
        ))
    );
}

#[test]
fn a_rotator_of_this_build_gets_no_warning() {
    let e = env();
    let (_bins, _old, new) = binaries();
    let proc_root = fake_rotator(&e, &new);
    assert_eq!(foreign_note_for(&e.rs, proc_root.path(), Some(&new)), None);
}

#[test]
fn no_running_rotator_gets_no_warning() {
    let e = env();
    let (_bins, _old, new) = binaries();
    let empty_proc = tempfile::tempdir().unwrap();
    assert_eq!(
        foreign_note_for(&e.rs, empty_proc.path(), Some(&new)),
        None,
        "no pid file"
    );
    private_file(&e.rs.pid, format!("{PID}\n").as_bytes());
    assert_eq!(
        foreign_note_for(&e.rs, empty_proc.path(), Some(&new)),
        None,
        "a stale pid file names no rotator"
    );
}

#[test]
fn a_rotator_whose_build_cannot_be_checked_gets_the_warning() {
    // Unknown counts as foreign: a silent hold that does not hold is worse
    // than a spurious warning.
    let e = env();
    let (_bins, old, _new) = binaries();
    let proc_root = fake_rotator(&e, &old);
    let note = foreign_note_for(&e.rs, proc_root.path(), None).expect("warned");
    assert!(note.ends_with(FOREIGN_IGNORES_HOLD), "{note}");
}

// --- D-Release ----------------------------------------------------------------

#[test]
fn release_without_a_pin_is_refused_under_a_held_lock() {
    let e = env();
    pointer::write(&e.root, &n("work")).unwrap();
    let _held = hold_state_lock(&e.root);
    assert!(
        release(&e.store).is_err(),
        "the check for a pin is made under the lock"
    );
    assert_eq!(pin::read(&e.rs), PinState::None);
    assert_eq!(pointer::read(&e.root).unwrap(), Some(n("work")));
}
#[cfg(target_os = "linux")]
#[test]
fn release_with_the_lock_free_reopens_the_gate_and_moves_nothing() {
    let e = env();
    switch_to(&e, "spare");
    let live = ino_at(&e.live());
    let ledger = fs::read(&e.rs.ledger).unwrap();
    assert!(release(&e.store).unwrap().pin);
    assert_eq!(pin::gate(&pin::read(&e.rs)), pin::Gate::Open);
    assert_eq!(pointer::read(&e.root).unwrap(), None);
    assert_eq!(ino_at(&e.live()), live, "release moves no login");
    assert_eq!(
        fs::read(&e.rs.ledger).unwrap(),
        ledger,
        "release writes no ledger entry"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn release_under_a_held_lock_leaves_pin_and_pointer_alone() {
    let e = env();
    switch_to(&e, "spare");
    let pin_bytes = fs::read(&e.rs.pin).unwrap();
    let _held = hold_state_lock(&e.root);
    assert!(release(&e.store).is_err());
    assert_eq!(fs::read(&e.rs.pin).unwrap(), pin_bytes);
    assert_eq!(pointer::read(&e.root).unwrap(), Some(n("spare")));
}
