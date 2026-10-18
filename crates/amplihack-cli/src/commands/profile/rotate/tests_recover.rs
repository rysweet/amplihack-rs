//! `rotate::swap::recover`: recovery decides by where the recorded inodes
//! are, never by the journal's phase (D10, issue #1592). Shares the fixture
//! in `tests_swap.rs`; every assertion about a login is by inode.

use super::ledger::{Phase, RotateJournal};
use super::swap::RecoverOutcome;
use super::tests_swap::{LOCK, env, ino, ino_at, login, n, private};
use crate::commands::profile::ProfileError;
use std::fs;

#[test]
fn no_journal_is_nothing_to_recover() {
    let e = env();
    assert_eq!(e.recover().unwrap(), RecoverOutcome::None);
    e.assert_unchanged();
}

#[test]
fn before_the_exchange_recovery_rolls_back_and_releases_its_locks() {
    let e = env();
    fs::create_dir(e.home.join(".claude").join(LOCK)).unwrap();
    let ours = ino(&e.home.join(".claude").join(LOCK));
    fs::create_dir(e.root.join("spare").join(LOCK)).unwrap();
    let foreign = ino(&e.root.join("spare").join(LOCK));
    e.rs.save_journal(&RotateJournal {
        home_lock_ino: Some(ours),
        to_lock_ino: Some(foreign + 1),
        ..e.journal(Phase::J0)
    })
    .unwrap();
    assert_eq!(
        e.recover().unwrap(),
        RecoverOutcome::RolledBack {
            from: n("work"),
            to: n("spare")
        }
    );
    e.assert_unchanged();
    assert!(!e.home.join(".claude").join(LOCK).exists(), "ours released");
    assert_eq!(
        ino_at(&e.root.join("spare").join(LOCK)),
        Some(foreign),
        "a lock with another inode is left alone"
    );
}

#[test]
fn a_j0_journal_after_the_exchange_rolls_forward() {
    let e = env();
    e.rs.save_journal(&e.journal(Phase::J0)).unwrap();
    e.exchanged();
    assert_eq!(
        e.recover().unwrap(),
        RecoverOutcome::RolledForward {
            from: n("work"),
            to: n("spare")
        }
    );
    e.assert_finished();
}

#[test]
fn a_token_refresh_in_home_after_the_exchange_still_rolls_forward() {
    let e = env();
    e.rs.save_journal(&e.journal(Phase::S1)).unwrap();
    e.exchanged();
    let refreshed = login(&e.live());
    assert!(matches!(
        e.recover().unwrap(),
        RecoverOutcome::RolledForward { .. }
    ));
    assert_eq!(ino_at(&e.live()), Some(refreshed));
    assert_eq!(ino_at(&e.slot("work")), Some(e.l));
    assert!(!e.rs.journal.exists());
}

#[test]
fn after_s2_recovery_finishes_s3_and_s4() {
    let e = env();
    e.rs.save_journal(&e.journal(Phase::S2)).unwrap();
    e.exchanged();
    fs::rename(e.slot("spare"), e.slot("work")).unwrap();
    assert!(matches!(
        e.recover().unwrap(),
        RecoverOutcome::RolledForward { .. }
    ));
    e.assert_finished();
}

#[test]
fn s3_and_s4_are_safe_to_repeat() {
    let e = env();
    e.exchanged();
    fs::rename(e.slot("spare"), e.slot("work")).unwrap();
    for phase in [Phase::S3, Phase::S4] {
        e.rs.save_journal(&e.journal(phase)).unwrap();
        assert!(matches!(
            e.recover().unwrap(),
            RecoverOutcome::RolledForward { .. }
        ));
        e.assert_finished();
    }
}

#[test]
fn an_unrecognised_layout_is_inconsistent_sticky_and_moves_nothing() {
    let e = env();
    fs::create_dir(e.home.join(".claude").join(LOCK)).unwrap();
    let lock = ino(&e.home.join(".claude").join(LOCK));
    e.rs.save_journal(&RotateJournal {
        home_lock_ino: Some(lock),
        ..e.journal(Phase::S1)
    })
    .unwrap();
    // Both the source and the target slot hold a login: neither way is safe.
    let s = login(&e.slot("work"));
    let before = e.all_logins();
    assert!(matches!(
        e.recover().unwrap(),
        RecoverOutcome::Inconsistent(_)
    ));
    assert_eq!(e.all_logins(), before);
    assert_eq!(ino_at(&e.slot("work")), Some(s));
    assert_eq!(
        e.rs.load_journal().unwrap().unwrap().phase,
        Phase::Inconsistent
    );
    assert!(
        e.home.join(".claude").join(LOCK).exists(),
        "locks left in place"
    );

    fs::remove_file(e.slot("work")).unwrap();
    assert!(
        matches!(e.recover().unwrap(), RecoverOutcome::Inconsistent(_)),
        "inconsistent is sticky even once the layout looks recoverable"
    );
}

#[test]
fn a_missing_login_in_home_is_inconsistent() {
    let e = env();
    e.rs.save_journal(&e.journal(Phase::S1)).unwrap();
    e.exchanged();
    fs::rename(e.live(), e.root.join("moved-away")).unwrap();
    assert!(matches!(
        e.recover().unwrap(),
        RecoverOutcome::Inconsistent(_)
    ));
    assert!(e.root.join("moved-away").exists(), "never deleted");
}

#[test]
fn a_journal_for_another_home_is_inconsistent() {
    let e = env();
    e.rs.save_journal(&RotateJournal {
        home: "/somewhere/else".into(),
        ..e.journal(Phase::J0)
    })
    .unwrap();
    assert!(matches!(
        e.recover().unwrap(),
        RecoverOutcome::Inconsistent(_)
    ));
    assert_eq!(ino_at(&e.live()), Some(e.l));
    assert_eq!(ino_at(&e.slot("spare")), Some(e.t));
}

#[test]
fn a_journal_naming_a_path_outside_the_root_moves_nothing() {
    let e = env();
    let mut v = serde_json::to_value(e.journal(Phase::S1)).unwrap();
    v["to"] = "../outside".into();
    private(&e.rs.journal, &v.to_string());
    e.exchanged();
    let before = e.all_logins();
    match e.recover() {
        Ok(RecoverOutcome::Inconsistent(_)) | Err(ProfileError::RotateInconsistent(_)) => {}
        other => panic!("expected an inconsistent outcome, got {other:?}"),
    }
    assert_eq!(e.all_logins(), before);
}
