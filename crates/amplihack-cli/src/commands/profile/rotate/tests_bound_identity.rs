//! File binding and budget contracts with task-owned fake logins only.
use super::{Budget, Reader};
use crate::commands::profile::rotate::tests::tests_fixture_identity;
use crate::commands::profile::rotate::tests_swap::{env, private};
use std::fs;

#[test]
fn an_exhausted_shared_budget_starts_no_identity_read() {
    let e = env();
    let budget = Budget::new();
    budget.0.set(
        std::time::Instant::now()
            .checked_sub(super::super::cred_identity::identity_budget())
            .expect(
                "host uptime is shorter than the identity budget; \
                 unset AMPLIHACK_TEST_IDENTITY_* or reboot later",
            ),
    );
    let read = |_: &std::path::Path| -> Result<super::Account, &'static str> {
        panic!("identity read started after the shared deadline")
    };
    let reader = Reader {
        read: &read,
        budget: &budget,
    };
    assert!(reader.bound(&e.home.join(".claude"), false).is_err());
    e.assert_unchanged();
}

#[test]
fn a_hard_link_or_symlink_never_reaches_the_identity_reader() {
    let e = env();
    let budget = Budget::new();
    let read = |_: &std::path::Path| -> Result<super::Account, &'static str> {
        panic!("unsafe login reached the reader")
    };
    let reader = Reader {
        read: &read,
        budget: &budget,
    };
    fs::hard_link(e.live(), e.home.join("fake-link")).unwrap();
    assert!(reader.bound(&e.home.join(".claude"), false).is_err());
    fs::remove_file(e.home.join("fake-link")).unwrap();
    let held = e.home.join("held-fake-login");
    fs::rename(e.live(), &held).unwrap();
    std::os::unix::fs::symlink(&held, e.live()).unwrap();
    assert!(reader.bound(&e.home.join(".claude"), false).is_err());
}

#[test]
fn a_session_rewrite_during_a_read_does_not_invalidate_the_login() {
    let e = env();
    let budget = Budget::new();
    let read = |dir: &std::path::Path| {
        let account = tests_fixture_identity::read(dir)?;
        private(
            &e.home.join(".claude.json"),
            r#"{"oauthAccount":{"accountUuid":"u-stale"}}"#,
        );
        Ok(account)
    };
    let reader = Reader {
        read: &read,
        budget: &budget,
    };
    let bound = reader.bound(&e.home.join(".claude"), false).unwrap();
    assert_eq!(bound.account().unwrap().key(), "u-work");
    assert!(bound.current());
    assert!(!format!("{bound:?}").contains("u-work"));
}

/// Issue #1962 (D1): group write is accepted only for the directory's group, so
/// changing the group after a reading invalidates that reading.
#[test]
fn a_changed_directory_group_invalidates_the_reading() {
    let e = env();
    let dir = e.home.join(".claude");
    let budget = Budget::new();
    let read = tests_fixture_identity::read;
    let reader = Reader {
        read: &read,
        budget: &budget,
    };
    let bound = reader.bound(&dir, false).expect("a private fake login");
    assert!(bound.current());
    // SAFETY: getgroups fills at most the buffer's length.
    let mut groups = [0 as libc::gid_t; 64];
    let count = unsafe { libc::getgroups(64, groups.as_mut_ptr()) };
    let own = std::os::unix::fs::MetadataExt::gid(&fs::metadata(&dir).unwrap());
    let other = groups[..count.max(0) as usize].iter().find(|g| **g != own);
    let Some(other) = other else {
        eprintln!("SKIP: this user is in only one group");
        return;
    };
    std::os::unix::fs::chown(&dir, None, Some(*other)).unwrap();
    assert!(!bound.current(), "a new group is a new set of writers");
}
