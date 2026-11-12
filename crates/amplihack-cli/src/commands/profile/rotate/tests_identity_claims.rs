//! Deterministic filesystem-error controls; all other observations use owned fake roots.
use super::*;
use crate::commands::profile::rotate::login_identity::Budget;
use crate::commands::profile::rotate::tests_swap::{env, n};
use std::io;

#[test]
fn metadata_errors_cannot_prove_an_unclaimed_login() {
    let e = env();
    assert!(e.root.starts_with(std::env::temp_dir()));
    let dir = e.store.add(&n("unreadable")).unwrap();
    let blocked = dir.join(CREDS);
    let state = State::new(&e.root).unwrap();
    let budget = Budget::new();
    let read = super::super::login_identity::default_read();
    let reader = Reader {
        read: &read,
        budget: &budget,
    };
    let account = reader.oauth(&e.home.join(".claude")).unwrap();
    // A genuine missing login is the positive absence control.
    assert!(scan(&e.store, &state, Some(&n("work")), &account, reader).is_ok());
    for error in [
        io::ErrorKind::PermissionDenied,
        io::ErrorKind::NotADirectory,
        io::ErrorKind::Other,
    ] {
        let probe = |path: &std::path::Path| {
            if path == blocked {
                Err(io::Error::from(error))
            } else {
                std::fs::symlink_metadata(path)
            }
        };
        assert!(
            scan_with(
                &e.store,
                &state,
                Some(&n("work")),
                &account,
                reader,
                None,
                &probe
            )
            .is_err(),
            "metadata error {error:?} is uncertain, not absence"
        );
    }
}

#[test]
fn absence_revalidation_refuses_new_metadata_uncertainty() {
    let e = env();
    assert!(e.root.starts_with(std::env::temp_dir()));
    let blocked = e.store.add(&n("unreadable")).unwrap().join(CREDS);
    let state = State::new(&e.root).unwrap();
    let budget = Budget::new();
    let read = super::super::login_identity::default_read();
    let reader = Reader {
        read: &read,
        budget: &budget,
    };
    let account = reader.oauth(&e.home.join(".claude")).unwrap();
    let claims = scan(&e.store, &state, Some(&n("work")), &account, reader).unwrap();
    assert!(claims.current_ignoring(&e.store, &state, None));
    let probe = |path: &std::path::Path| {
        if path == blocked {
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        } else {
            std::fs::symlink_metadata(path)
        }
    };
    assert!(!claims.current_with(&e.store, &state, None, &probe));
}
