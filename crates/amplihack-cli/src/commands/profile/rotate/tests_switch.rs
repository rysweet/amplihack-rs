//! `rotate::switch`: `profile switch NAME` moves NAME's login into
//! `~/.claude` with the rotator's lock, recovery and swap, pins rotation to
//! it and writes the default pointer (issue #1691, D4-D7). In process, on the
//! fixture in `tests_swap.rs`: no `claude` binary is resolved, no daemon is
//! spawned, and nothing outside the temp dir is touched (#1690). The store
//! has no shared-assets source, so `shared_dirs::ensure_for` does nothing.
//!
//! Contract:
//! - `switch_live(&ProfileStore, home: &Path, &ProfileName) ->
//!   Result<Result<Switched, AfterSwapFailure>, ProfileError>`.
//!   `Switched::{Moved { from, to }, AlreadyLive(ProfileName),
//!   DefaultOnly { reason, held }}`.
//!   `AfterSwapFailure { what: &'static str, .. }` with `what` one of
//!   `"rotation hold"`, `"default pointer"` or `"rotate ledger"`: the swap
//!   ran, a later write failed. A refusal before the lock is held and
//!   recovery has settled (lock busy, inconsistent recovery) is
//!   `Err(ProfileError::RotateRefused(_))` and writes nothing
//!   (`assert_refused_untouched`). A refusal after that, about the live login
//!   or the target, is `DefaultOnly`: `.default` names the target and nothing
//!   else changes — no pin, no ledger ALERT, no swap (`assert_refused`).
//! - `release(&ProfileStore) -> Result<Released, ProfileError>`: removes the
//!   pin and the default pointer together under the rotate lock;
//!   `Released { pin, default }` says which were there.

use super::Released;
use super::cycle::Ctx;
use super::ledger::{Phase, RotateLedger};
use super::observe::snapshot;
use super::pin::{self, Gate, PinState};
use super::settings;
use super::swap::{DOCUMENTED, Source, swap};
use super::switch::{Switched, release, switch_live};
use super::tests_swap::{CREDS, Env, LOCK, NOW, env, ino_at, json, login, n, private};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::State;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::pointer;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::time::Duration;

fn bytes(p: &Path) -> Option<(u64, Vec<u8>)> {
    let m = fs::symlink_metadata(p).ok()?;
    let body = if m.is_file() && !p.ends_with(CREDS) {
        fs::read(p).unwrap()
    } else {
        Vec::new()
    };
    Some((m.ino(), body))
}

/// A file's inode and bytes, or `None` when it is absent.
type FileState = Option<(u64, Vec<u8>)>;

/// Every file a refused switch must leave alone, by inode and bytes. Login
/// files by inode only. The lock file and the log are not part of it.
fn snap(e: &Env) -> Vec<(String, FileState)> {
    let mut v = Vec::new();
    for (label, p) in [
        ("ledger", e.rs.ledger.clone()),
        ("journal", e.rs.journal.clone()),
        ("pin", e.rs.pin.clone()),
        (".default", e.root.join(".default")),
        ("~/.claude.json", e.home.join(".claude.json")),
        ("~/.claude login", e.live()),
        ("~/.claude refresh lock", e.home.join(".claude").join(LOCK)),
        ("work login", e.slot("work")),
        ("spare login", e.slot("spare")),
        ("work/.claude.json", e.root.join("work/.claude.json")),
        ("spare/.claude.json", e.root.join("spare/.claude.json")),
    ] {
        v.push((label.to_string(), bytes(&p)));
    }
    v
}

fn log(e: &Env) -> String {
    fs::read_to_string(&e.rs.log).unwrap_or_default()
}

pub(super) fn switch(e: &Env, name: &str) -> Result<Switched, ProfileError> {
    match switch_live(&e.store, &e.home, &n(name)) {
        Ok(Ok(s)) => Ok(s),
        Ok(Err(f)) => panic!("unexpected partial failure: {}", f.what),
        Err(err) => Err(err),
    }
}

/// Every file but `.default`, which a default-only refusal writes.
fn snap_but_default(e: &Env) -> Vec<(String, FileState)> {
    snap(e)
        .into_iter()
        .filter(|(l, _)| l != ".default")
        .collect()
}

/// The live login cannot be moved for a reason containing `want`: `name` is
/// still made the default for new launches (issue #1691, "keeps the
/// `.default` pointer behaviour"), and nothing else changes. Returns the
/// profile an earlier switch still holds rotation on.
#[track_caller]
pub(super) fn assert_refused(e: &Env, name: &str, want: &str) -> Option<ProfileName> {
    let before = snap_but_default(e);
    let pin_before = pin::read(&e.rs);
    let held = match switch_live(&e.store, &e.home, &n(name)) {
        Ok(Ok(Switched::DefaultOnly { reason, held })) => {
            assert!(reason.contains(want), "refusal {reason:?} lacks {want:?}");
            assert!(
                !reason.starts_with("ALERT: "),
                "the ALERT prefix is stripped: {reason}"
            );
            held
        }
        Ok(Ok(s)) => panic!("expected a refusal containing {want:?}, got {s:?}"),
        Ok(Err(f)) => panic!("expected a refusal, got a partial failure: {}", f.what),
        Err(other) => panic!("expected a default-only refusal ({want:?}), got {other:?}"),
    };
    assert_eq!(
        snap_but_default(e),
        before,
        "a refused switch changed something other than .default"
    );
    assert_eq!(
        pointer::read(&e.root).unwrap(),
        Some(n(name)),
        "a refused live move still sets the default for new launches"
    );
    assert_eq!(pin::read(&e.rs), pin_before, "a refusal writes no pin");
    if let Some(l) = e.rs.load_ledger().unwrap() {
        assert_eq!(l.alert, None, "a refusal writes no ledger ALERT");
    }
    held
}

/// Refused before the lock is held or while recovery is inconsistent:
/// nothing at all is written, `.default` included.
#[track_caller]
pub(super) fn assert_refused_untouched(e: &Env, name: &str, want: &str) {
    let before = snap(e);
    match switch_live(&e.store, &e.home, &n(name)) {
        Err(ProfileError::RotateRefused(m)) => {
            assert!(m.contains(want), "refusal {m:?} lacks {want:?}");
        }
        Ok(Ok(s)) => panic!("expected a refusal containing {want:?}, got {s:?}"),
        Ok(Err(f)) => panic!("expected a refusal, got a partial failure: {}", f.what),
        Err(other) => panic!("expected RotateRefused({want:?}), got {other:?}"),
    }
    assert_eq!(snap(e), before, "a refused switch changed something");
    assert_eq!(pin::read(&e.rs), PinState::None);
    assert_eq!(pointer::read(&e.root).unwrap(), None);
}

// --- AC1: the switch is the rotator's swap ----------------------------------

#[cfg(target_os = "linux")]
#[test]
fn a_switch_leaves_exactly_what_a_rotate_swap_leaves() {
    let rotated = env();
    rotated.swap().unwrap();
    let switched = env();
    assert_eq!(
        switch(&switched, "spare").unwrap(),
        Switched::Moved {
            from: n("work"),
            to: n("spare")
        }
    );
    rotated.assert_finished();
    switched.assert_finished();
    assert_eq!(switched.all_logins().len(), 2, "no copy was made");

    let (a, b) = (
        rotated.rs.load_ledger().unwrap().unwrap(),
        switched.rs.load_ledger().unwrap().unwrap(),
    );
    assert_eq!(
        (&a.version, &a.live, &a.account, &a.previous, &a.alert),
        (&b.version, &b.live, &b.account, &b.previous, &b.alert),
        "the ledger entry is the one the rotator writes"
    );
    assert!(b.swapped_at.is_some() && b.since.is_some());
    assert_eq!(
        json(&rotated.home.join(".claude.json")),
        json(&switched.home.join(".claude.json")),
        "only oauthAccount moves, as for a rotate swap"
    );
    for p in ["work", "spare"] {
        let path = |e: &Env| e.root.join(p).join(".claude.json");
        assert_eq!(
            bytes(&path(&rotated)).map(|b| b.1),
            bytes(&path(&switched)).map(|b| b.1)
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn a_switch_pins_rotation_and_writes_the_default() {
    let e = env();
    switch(&e, "spare").unwrap();
    assert_eq!(pin::read(&e.rs), PinState::Pinned(n("spare")));
    assert!(matches!(pin::gate(&pin::read(&e.rs)), Gate::Held(_)));
    assert_eq!(pointer::read(&e.root).unwrap(), Some(n("spare")));
    let m = fs::symlink_metadata(e.root.join(".default")).unwrap();
    assert_eq!(m.mode() & 0o777, 0o600);
    assert!(!e.home.join(".claude").join(LOCK).exists());
    assert!(!e.root.join("spare").join(LOCK).exists());
}

#[cfg(target_os = "linux")]
#[test]
fn a_switch_logs_a_switch_line_not_a_swap_line() {
    let e = env();
    switch(&e, "spare").unwrap();
    let text = log(&e);
    let lines: Vec<&str> = text.lines().collect();
    assert!(
        lines.iter().any(|l| l.ends_with(" switch work->spare")),
        "{text}"
    );
    assert!(
        !lines.iter().any(|l| l.contains(" swap ")),
        "a switch is not logged as a rotator swap: {text}"
    );
    for secret in [
        "SENTINEL",
        "SECRET-KEY",
        "u-work",
        "u-spare",
        "@example.com",
    ] {
        assert!(!text.contains(secret), "{secret} logged: {text}");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn a_switch_adopts_a_login_no_profile_owns() {
    let e = env();
    fs::remove_file(e.root.join("work/.claude.json")).unwrap();
    assert_eq!(
        switch(&e, "spare").unwrap(),
        Switched::Moved {
            from: n("default-original"),
            to: n("spare")
        }
    );
    assert_eq!(
        ino_at(&e.root.join("default-original").join(CREDS)),
        Some(e.l),
        "moved, not copied"
    );
    assert_eq!(ino_at(&e.live()), Some(e.t));
    assert!(log(&e).contains("switch default-original->spare"));
    assert_eq!(pin::read(&e.rs), PinState::Pinned(n("spare")));
}

// --- Already live -----------------------------------------------------------

#[test]
fn switching_to_the_live_profile_moves_nothing_and_still_pins() {
    let e = env();
    // `work` has no stored login of its own: it is live, so step 7 is skipped.
    assert!(!e.slot("work").exists());
    assert_eq!(
        switch(&e, "work").unwrap(),
        Switched::AlreadyLive(n("work"))
    );
    e.assert_unchanged();
    assert_eq!(pin::read(&e.rs), PinState::Pinned(n("work")));
    assert_eq!(pointer::read(&e.root).unwrap(), Some(n("work")));
    assert!(
        log(&e).contains("skip: work is already the live login; nothing moved"),
        "{}",
        log(&e)
    );
    let l = e.rs.load_ledger().unwrap().unwrap_or_default();
    assert_eq!(l.swapped_at, None, "no swap was recorded");
}

#[cfg(target_os = "linux")]
#[test]
fn a_second_switch_swaps_again_and_moves_the_pin() {
    let e = env();
    switch(&e, "spare").unwrap();
    assert_eq!(
        switch(&e, "work").unwrap(),
        Switched::Moved {
            from: n("spare"),
            to: n("work")
        }
    );
    assert_eq!(ino_at(&e.live()), Some(e.l));
    assert_eq!(ino_at(&e.slot("spare")), Some(e.t));
    assert_eq!(pin::read(&e.rs), PinState::Pinned(n("work")));
    assert_eq!(pointer::read(&e.root).unwrap(), Some(n("work")));
}

#[cfg(target_os = "linux")]
#[test]
fn switching_again_to_the_pinned_live_profile_is_the_already_live_path() {
    let e = env();
    switch(&e, "spare").unwrap();
    assert_eq!(
        switch(&e, "spare").unwrap(),
        Switched::AlreadyLive(n("spare"))
    );
    e.assert_finished();
}

// --- AC6: refusals move nothing; past the lock they still set the default --

#[test]
fn a_profile_without_a_stored_login_is_refused() {
    let e = env();
    fs::remove_file(e.slot("spare")).unwrap();
    assert_refused(
        &e,
        "spare",
        "profile spare has no stored login; run `amplihack profile login spare` first",
    );
}

#[test]
fn the_no_stored_login_reason_is_documented() {
    // 11 since D4 added `FOREIGN_IGNORES_HOLD`; see tests_switch_hold.rs.
    assert_eq!(DOCUMENTED.len(), 11);
    let e = env();
    fs::remove_file(e.slot("spare")).unwrap();
    let Ok(Ok(Switched::DefaultOnly { reason: m, .. })) =
        switch_live(&e.store, &e.home, &n("spare"))
    else {
        panic!("refused");
    };
    assert!(
        DOCUMENTED
            .iter()
            .any(|d| m.contains(d) && d.contains("no stored login")),
        "{m:?} is not one of the documented reasons"
    );
}

#[test]
fn a_held_state_lock_is_refused_within_the_bounded_wait() {
    let e = env();
    let state = State::new(&e.root).unwrap();
    let _held = state.try_lock(Duration::ZERO).unwrap().unwrap();
    let start = std::time::Instant::now();
    assert_refused_untouched(&e, "spare", "lock busy");
    assert!(start.elapsed() < Duration::from_secs(10), "bounded wait");
}

#[test]
fn an_inconsistent_journal_is_refused() {
    let e = env();
    e.rs.save_journal(&e.journal(Phase::Inconsistent)).unwrap();
    assert_refused_untouched(&e, "spare", "inconsistent");
    assert!(e.rs.journal.exists(), "left for a person to resolve");
}

#[test]
fn a_hand_changed_login_is_refused_without_an_alert() {
    let e = env();
    e.rs.save_ledger(&RotateLedger {
        live: Some("work".into()),
        account: Some("u-work".into()),
        ..RotateLedger::default()
    })
    .unwrap();
    private(
        &e.home.join(".claude.json"),
        &serde_json::json!({
            "oauthAccount": {"accountUuid": "u-other", "emailAddress": "other@example.com"}
        })
        .to_string(),
    );
    assert_refused(&e, "spare", "is not the one the ledger names (live=work)");
}

#[test]
fn no_login_in_home_is_refused() {
    let e = env();
    fs::remove_file(e.live()).unwrap();
    assert_refused(&e, "spare", "no login in ~/.claude");
}

#[test]
fn an_ambiguous_login_is_refused() {
    let e = env();
    // A second login-less profile with work's account: two profiles match.
    e.store.add(&n("twin")).unwrap();
    fs::copy(
        e.root.join("work/.claude.json"),
        e.root.join("twin/.claude.json"),
    )
    .unwrap();
    assert_refused(&e, "spare", "more than one profile matches");
}

#[test]
fn a_held_refresh_lock_is_refused_as_the_swap_refuses_it() {
    let e = env();
    // Claude's refresh lock is a directory; a switch never breaks it.
    fs::create_dir(e.home.join(".claude").join(LOCK)).unwrap();
    assert_refused(&e, "spare", "refresh lock");
    assert!(e.home.join(".claude").join(LOCK).is_dir());
}

#[test]
fn a_group_readable_stored_login_is_refused() {
    let e = env();
    fs::set_permissions(e.slot("spare"), fs::Permissions::from_mode(0o640)).unwrap();
    assert_refused(&e, "spare", "not a private regular file");
}

#[test]
fn a_refusal_from_the_swap_is_passed_through_unchanged() {
    let e = env();
    // The ledger names work as live: with a stored login of its own, work
    // would otherwise not match ~/.claude and the switch would adopt it.
    e.rs.save_ledger(&RotateLedger {
        live: Some("work".into()),
        account: Some("u-work".into()),
        ..RotateLedger::default()
    })
    .unwrap();
    let s = login(&e.slot("work"));
    let direct = swap(
        &e.store,
        &e.rs,
        &e.home,
        Source::Profile(n("work")),
        &n("spare"),
        NOW,
    )
    .unwrap_err()
    .to_string();
    let via_switch = match switch_live(&e.store, &e.home, &n("spare")) {
        Ok(Ok(Switched::DefaultOnly { reason, .. })) => {
            ProfileError::RotateRefused(reason).to_string()
        }
        Err(err) => panic!("expected a default-only refusal, got {err:?}"),
        Ok(other) => panic!("expected a refusal, got {:?}", other.is_ok()),
    };
    assert_eq!(via_switch, direct);
    assert_eq!(ino_at(&e.slot("work")), Some(s));
    assert_eq!(pin::read(&e.rs), PinState::None);
    assert_eq!(pointer::read(&e.root).unwrap(), Some(n("spare")));
}

// --- AC7: an interrupted swap is recovered first ----------------------------

#[cfg(target_os = "linux")]
#[test]
fn a_switch_finishes_an_interrupted_swap_before_deciding() {
    let e = env();
    e.rs.save_journal(&e.journal(Phase::S1)).unwrap();
    e.exchanged();
    assert_eq!(
        switch(&e, "spare").unwrap(),
        Switched::AlreadyLive(n("spare")),
        "recovery finished the swap, so spare is live"
    );
    e.assert_finished();
    assert!(log(&e).contains("recovery: finished an interrupted swap work->spare"));
    assert_eq!(pin::read(&e.rs), PinState::Pinned(n("spare")));
}

#[cfg(target_os = "linux")]
#[test]
fn a_switch_rolls_back_a_swap_stopped_at_j0_then_swaps() {
    let e = env();
    e.rs.save_journal(&e.journal(Phase::J0)).unwrap();
    assert_eq!(
        switch(&e, "spare").unwrap(),
        Switched::Moved {
            from: n("work"),
            to: n("spare")
        }
    );
    assert!(log(&e).contains("recovery: rolled back an interrupted swap work->spare"));
    e.assert_finished();
}

// --- After the swap: a failed write is reported, and a rerun repairs it ----

#[cfg(target_os = "linux")]
#[test]
fn a_failed_pin_write_after_the_swap_is_a_partial_failure() {
    let e = env();
    fs::create_dir(&e.rs.pin).unwrap();
    fs::write(e.rs.pin.join("blocker"), "x").unwrap();
    match switch_live(&e.store, &e.home, &n("spare")) {
        Ok(Err(f)) => assert_eq!(f.what, "rotation hold"),
        Ok(Ok(s)) => panic!("expected a partial failure, got {s:?}"),
        Err(err) => panic!("the swap ran, so this is not a refusal: {err:?}"),
    }
    e.assert_finished();
    assert_eq!(
        pointer::read(&e.root).unwrap(),
        None,
        "the pointer is written after the pin"
    );

    fs::remove_dir_all(&e.rs.pin).unwrap();
    assert_eq!(
        switch(&e, "spare").unwrap(),
        Switched::AlreadyLive(n("spare")),
        "a rerun takes the already-live path"
    );
    assert_eq!(pin::read(&e.rs), PinState::Pinned(n("spare")));
    assert_eq!(pointer::read(&e.root).unwrap(), Some(n("spare")));
}

#[cfg(target_os = "linux")]
#[test]
fn a_failed_pointer_write_after_the_swap_is_a_partial_failure() {
    let e = env();
    let blocker = e.root.join(".default");
    fs::create_dir(&blocker).unwrap();
    fs::write(blocker.join("blocker"), "x").unwrap();
    match switch_live(&e.store, &e.home, &n("spare")) {
        Ok(Err(f)) => assert_eq!(f.what, "default pointer"),
        Ok(Ok(s)) => panic!("expected a partial failure, got {s:?}"),
        Err(err) => panic!("the swap ran, so this is not a refusal: {err:?}"),
    }
    e.assert_finished();
    assert_eq!(pin::read(&e.rs), PinState::Pinned(n("spare")));

    fs::remove_dir_all(&blocker).unwrap();
    assert_eq!(
        switch(&e, "spare").unwrap(),
        Switched::AlreadyLive(n("spare"))
    );
    assert_eq!(pointer::read(&e.root).unwrap(), Some(n("spare")));
}

// --- release (`switch --none`) ----------------------------------------------

#[cfg(target_os = "linux")]
#[test]
fn release_removes_the_pin_and_the_default_together() {
    let e = env();
    let none = Released {
        pin: false,
        default: false,
    };
    assert_eq!(release(&e.store).unwrap(), none, "nothing pinned yet");
    switch(&e, "spare").unwrap();
    assert_eq!(
        release(&e.store).unwrap(),
        Released {
            pin: true,
            default: true
        }
    );
    assert_eq!(pin::read(&e.rs), PinState::None);
    assert_eq!(pin::gate(&pin::read(&e.rs)), Gate::Open);
    assert_eq!(
        pointer::read(&e.root).unwrap(),
        None,
        "the pointer is removed under the same lock as the pin"
    );
    assert_eq!(release(&e.store).unwrap(), none, "idempotent");
    e.assert_finished();
}

#[test]
fn release_under_a_held_lock_is_refused_and_removes_nothing() {
    let e = env();
    pin::write(&e.rs, &n("spare")).unwrap();
    pointer::write(&e.root, &n("spare")).unwrap();
    let before = fs::read(&e.rs.pin).unwrap();
    let state = State::new(&e.root).unwrap();
    let _held = state.try_lock(Duration::ZERO).unwrap().unwrap();
    assert!(matches!(
        release(&e.store),
        Err(ProfileError::RotateRefused(_))
    ));
    assert_eq!(fs::read(&e.rs.pin).unwrap(), before);
    assert_eq!(pointer::read(&e.root).unwrap(), Some(n("spare")));
}

#[test]
fn release_without_a_pin_still_waits_for_the_lock() {
    // The presence check is under the lock: a `switch` that holds it may be
    // between writing its pin and its pointer.
    let e = env();
    pointer::write(&e.root, &n("spare")).unwrap();
    let state = State::new(&e.root).unwrap();
    let _held = state.try_lock(Duration::ZERO).unwrap().unwrap();
    assert!(matches!(
        release(&e.store),
        Err(ProfileError::RotateRefused(_))
    ));
    assert_eq!(pointer::read(&e.root).unwrap(), Some(n("spare")));
}

#[test]
fn release_refuses_a_non_empty_directory_at_the_pin_path() {
    let e = env();
    fs::create_dir(&e.rs.pin).unwrap();
    fs::write(e.rs.pin.join("inside"), "x").unwrap();
    pointer::write(&e.root, &n("spare")).unwrap();
    assert!(matches!(
        release(&e.store),
        Err(ProfileError::RotateRefused(_))
    ));
    assert!(e.rs.pin.join("inside").exists());
    assert_eq!(
        pointer::read(&e.root).unwrap(),
        Some(n("spare")),
        "a pin that cannot be removed keeps the default too"
    );
}

// --- The cycle sees the pin -------------------------------------------------

#[test]
fn a_pin_change_invalidates_the_cycle_snapshot() {
    let e = env();
    let s = settings::resolve(&super::TuneFlags::default(), None, &|_| None).unwrap();
    let ctx = Ctx::new(e.store.clone(), e.home.clone(), s);
    let state = State::new(&e.root).unwrap();
    let before = snapshot(&ctx, &state).unwrap();
    assert_eq!(before.pin, PinState::None);
    pin::write(&e.rs, &n("spare")).unwrap();
    let after = snapshot(&ctx, &state).unwrap();
    assert_eq!(after.pin, PinState::Pinned(n("spare")));
    assert_ne!(
        before, after,
        "a switch during measurement makes step 6 skip the swap"
    );
}
