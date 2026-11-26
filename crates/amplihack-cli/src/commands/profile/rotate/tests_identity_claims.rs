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

// --- Issue #2218 (S1): an excluded folder is never read by a claim scan -----

mod issue_2218 {
    use super::*;
    use crate::commands::profile::auto_switch::config::{self, ReadMode};
    use crate::commands::profile::expected;
    use crate::commands::profile::rotate::cycle::Ctx;
    use crate::commands::profile::rotate::settings;
    use crate::commands::profile::rotate::tests_swap::{env, login, n, private};
    use std::cell::Cell;

    thread_local! {
        static EXTRA_READS: Cell<usize> = const { Cell::new(0) };
    }

    fn acct(uuid: &str) -> Account {
        Account::from_claude_json(
            serde_json::json!({"oauthAccount": {
                "accountUuid": uuid, "emailAddress": format!("{uuid}@example.com")}})
            .to_string()
            .as_bytes(),
        )
        .unwrap()
    }

    /// Counts reads of `extra`; answers `u-<folder name>` for every folder.
    fn counting(dir: &std::path::Path) -> Result<Account, &'static str> {
        if dir.ends_with("extra") {
            EXTRA_READS.with(|c| c.set(c.get() + 1));
        }
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        Ok(acct(&format!("u-{name}")))
    }

    /// `extra` holds a login, a local account `u-extra` and a record, and is
    /// excluded and marked local-only; the scan looks for `target`.
    fn scan_counting(target: &str) -> (usize, Claims) {
        let e = env();
        assert!(e.root.starts_with(std::env::temp_dir()));
        e.store.add(&n("extra")).unwrap();
        login(&e.root.join("extra").join(CREDS));
        private(
            &e.root.join("extra/.claude.json"),
            &serde_json::json!({"oauthAccount": {
                "accountUuid": "u-extra", "emailAddress": "u-extra@example.com"}})
            .to_string(),
        );
        let state = State::new(&e.root).unwrap();
        expected::record(&state, &n("extra"), &acct("u-extra")).unwrap();
        config::update(&e.root, ReadMode::Editable, |c| {
            c.exclude.push(n("extra"));
            Ok(())
        })
        .unwrap();
        let s = settings::resolve(
            &crate::commands::profile::rotate::TuneFlags::default(),
            None,
            &|_| None,
        )
        .unwrap();
        let mut ctx = Ctx::new(e.store.clone(), e.home.clone(), s);
        ctx.identity = counting;
        ctx.set_excluded_dirs(vec![e.root.join("extra")]);
        EXTRA_READS.with(|c| c.set(0));
        let claims = scan(
            &e.store,
            &state,
            Some(&n("work")),
            &acct(target),
            ctx.identity_reader(),
        )
        .expect("an excluded folder does not block the scan");
        (EXTRA_READS.with(Cell::get), claims)
    }

    #[test]
    fn a_claim_scan_makes_no_read_of_an_excluded_folder() {
        let (calls, _) = scan_counting("u-other");
        assert_eq!(calls, 0);
    }

    #[test]
    fn an_excluded_folder_still_claims_its_local_account() {
        let (calls, claims) = scan_counting("u-extra");
        assert_eq!(calls, 0);
        assert!(matches!(claims.claim, Claim::Claimed));
        assert_eq!(claims.claimers, vec![n("extra")]);
    }
}
