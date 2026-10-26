//! `rotate::swap` refusals: every check runs before anything is written, so
//! a refused swap leaves the logins, the locks and the journal as they were
//! (D9, issue #1592). Uses the fixture in `tests_swap.rs`.

use super::ledger::Phase;
use super::swap::{Source, swap};
use super::tests_swap::{LOCK, NOW, env, ino, ino_at, login, n};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::euid;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn refused<T: std::fmt::Debug>(r: Result<T, ProfileError>) {
    assert!(matches!(r, Err(ProfileError::RotateRefused(_))), "{r:?}");
}

#[test]
fn a_swap_is_refused_when_the_source_already_has_a_login() {
    let e = env();
    let s = login(&e.slot("work"));
    refused(e.swap());
    assert_eq!(ino_at(&e.slot("work")), Some(s));
    assert_eq!(ino_at(&e.live()), Some(e.l));
    assert_eq!(ino_at(&e.slot("spare")), Some(e.t));
    assert!(!e.rs.journal.exists());
}

#[test]
fn a_swap_is_refused_for_a_loosened_hard_linked_or_missing_target() {
    let e = env();
    fs::set_permissions(e.slot("spare"), fs::Permissions::from_mode(0o644)).unwrap();
    refused(e.swap());
    e.assert_unchanged();
    fs::set_permissions(e.slot("spare"), fs::Permissions::from_mode(0o600)).unwrap();
    fs::hard_link(e.slot("spare"), e.root.join("spare/extra-link")).unwrap();
    refused(e.swap());
    e.assert_unchanged();
    fs::remove_file(e.root.join("spare/extra-link")).unwrap();
    fs::rename(e.slot("spare"), e.root.join("spare/aside")).unwrap();
    refused(e.swap());
    assert_eq!(ino_at(&e.live()), Some(e.l));
}

#[test]
fn a_swap_is_refused_for_an_unsafe_home() {
    let e = env();
    fs::set_permissions(e.home.join(".claude"), fs::Permissions::from_mode(0o777)).unwrap();
    refused(e.swap());
    fs::set_permissions(e.home.join(".claude"), fs::Permissions::from_mode(0o700)).unwrap();
    e.assert_unchanged();

    let link = e.home.with_file_name("home-link");
    std::os::unix::fs::symlink(&e.home, &link).unwrap();
    refused(swap(
        &e.store,
        &e.rs,
        &link,
        Source::Profile(n("work")),
        &n("spare"),
        NOW,
    ));
    refused(swap(
        &e.store,
        &e.rs,
        Path::new("relative/home"),
        Source::Profile(n("work")),
        &n("spare"),
        NOW,
    ));
    e.assert_unchanged();
}

#[test]
fn a_swap_is_refused_for_a_profile_root_others_can_write() {
    let e = env();
    fs::set_permissions(&e.root, fs::Permissions::from_mode(0o777)).unwrap();
    refused(e.swap());
    fs::set_permissions(&e.root, fs::Permissions::from_mode(0o700)).unwrap();
    e.assert_unchanged();
}

#[test]
fn a_swap_is_refused_while_a_journal_is_pending() {
    let e = env();
    e.rs.save_journal(&e.journal(Phase::Inconsistent)).unwrap();
    assert!(e.swap().is_err());
    assert_eq!(ino_at(&e.live()), Some(e.l));
    assert_eq!(ino_at(&e.slot("spare")), Some(e.t));
}

#[test]
fn a_refresh_lock_held_in_the_target_refuses_the_swap_and_is_left_in_place() {
    let e = env();
    fs::create_dir(e.root.join("spare").join(LOCK)).unwrap();
    let foreign = ino(&e.root.join("spare").join(LOCK));
    refused(e.swap());
    e.assert_unchanged();
    assert_eq!(ino_at(&e.root.join("spare").join(LOCK)), Some(foreign));
    assert!(!e.home.join(".claude").join(LOCK).exists(), "no lock taken");
}

#[test]
fn a_refresh_lock_held_by_the_live_session_refuses_the_swap() {
    // A refresh during the exchange would write the outgoing token over
    // the incoming login, so the swap waits for the next cycle.
    let e = env();
    let lock = e.home.join(".claude").join(LOCK);
    fs::create_dir(&lock).unwrap();
    let foreign = ino(&lock);
    refused(e.swap());
    e.assert_unchanged();
    assert_eq!(
        ino_at(&lock),
        Some(foreign),
        "Claude's lock is never broken"
    );
    assert!(!e.root.join("spare").join(LOCK).exists(), "no lock taken");
}

#[cfg(target_os = "linux")]
#[test]
fn a_refused_adopting_swap_leaves_no_empty_profile() {
    let e = env();
    fs::create_dir(e.home.join(".claude").join(LOCK)).unwrap();
    refused(swap(
        &e.store,
        &e.rs,
        &e.home,
        Source::Adopt,
        &n("spare"),
        NOW,
    ));
    e.assert_unchanged();
    assert!(!e.root.join("default-original").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn a_failed_move_to_the_source_releases_the_locks() {
    let e = env();
    // The source profile cannot be written, so the second rename fails
    // after the exchange.
    fs::set_permissions(e.root.join("work"), fs::Permissions::from_mode(0o500)).unwrap();
    let r = e.swap();
    fs::set_permissions(e.root.join("work"), fs::Permissions::from_mode(0o700)).unwrap();
    if euid() == 0 {
        return; // root ignores the directory's mode, so the move succeeds.
    }
    assert!(
        matches!(r, Err(ProfileError::RotateInconsistent(_))),
        "{r:?}"
    );
    assert!(
        !e.home.join(".claude").join(LOCK).exists(),
        "home lock released"
    );
    assert!(
        !e.root.join("spare").join(LOCK).exists(),
        "target lock released"
    );
    assert_eq!(ino_at(&e.live()), Some(e.t), "the incoming login is live");
    let j = e.rs.load_journal().unwrap().unwrap();
    assert_eq!(j.phase, Phase::S1, "recovery finishes the swap later");
    assert_eq!(
        (j.home_lock_ino, j.to_lock_ino),
        (None, None),
        "released locks are forgotten, so recovery never releases them again"
    );
}

#[test]
fn a_swap_is_refused_when_home_claude_json_is_not_an_object() {
    // `s3` could not record the new account, so nothing may move.
    let e = env();
    let cj = e.home.join(".claude.json");
    fs::write(&cj, "[1, 2]").unwrap();
    refused(e.swap());
    assert_eq!(ino_at(&e.live()), Some(e.l));
    assert_eq!(ino_at(&e.slot("spare")), Some(e.t));
    assert!(!e.rs.journal.exists());
    assert!(!e.home.join(".claude").join(LOCK).exists(), "no lock taken");
    assert_eq!(fs::read_to_string(&cj).unwrap(), "[1, 2]", "left as found");
}
