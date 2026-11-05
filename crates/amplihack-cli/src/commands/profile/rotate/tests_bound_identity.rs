//! File binding and budget contracts with task-owned fake logins only.
use super::{Budget, Reader};
use crate::commands::profile::rotate::tests::tests_fixture_identity;
use crate::commands::profile::rotate::tests_swap::{env, private};
use std::fs;

#[test]
fn an_exhausted_shared_budget_starts_no_identity_read() {
    let e = env();
    let budget = Budget::new();
    budget
        .0
        .set(std::time::Instant::now() - super::IDENTITY_BUDGET);
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
