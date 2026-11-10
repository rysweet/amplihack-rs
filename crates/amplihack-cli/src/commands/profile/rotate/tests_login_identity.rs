//! #1753: exercise the shared switch path with an injected fake identity
//! reader. These tests launch no process or network request. Every file
//! belongs to `tests_swap::env` beneath the task's temporary root; CLI
//! coverage additionally uses `isolated_amplihack` in the integration suite.

use super::cycle::Ctx;
use super::identify::Account;
use super::ledger::{Phase, RotateLedger};
use super::settings;
use super::switch::{Switched, switch_in};
use super::tests_swap::{Env, env, ino_at, n, private};
use crate::commands::profile::auto_switch::kit::{Liveness, State, now};
use crate::commands::profile::expected;
use std::fs;
use std::path::Path;

fn account(who: &str) -> Account {
    Account::from_profile(&serde_json::json!({"account": {
        "uuid": format!("u-{who}"), "email": format!("{who}@example.com")
    }}))
    .unwrap()
}

fn credential(path: &Path, who: &str) {
    assert!(path.starts_with(std::env::temp_dir()));
    private(
        path,
        &serde_json::json!({"claudeAiOauth": {
            "accessToken": format!("fake-task1753-{who}")
        }})
        .to_string(),
    );
}

/// Only these task-owned fake credentials can produce trusted evidence.
fn read_fake(dir: &Path) -> Result<Account, &'static str> {
    assert!(dir.starts_with(std::env::temp_dir()));
    let raw = fs::read(dir.join(".credentials.json")).map_err(|_| "fake missing")?;
    let v: serde_json::Value = serde_json::from_slice(&raw).map_err(|_| "fake invalid")?;
    match v["claudeAiOauth"]["accessToken"].as_str() {
        Some("fake-task1753-work") => Ok(account("work")),
        Some("fake-task1753-spare") => Ok(account("spare")),
        _ => Err("fake unknown identity"),
    }
}

fn fixture() -> Env {
    let e = env();
    assert!(e._tmp.path().starts_with(std::env::temp_dir()));
    credential(&e.live(), "work");
    credential(&e.slot("spare"), "spare");
    let state = State::new(&e.root).unwrap();
    for who in ["work", "spare"] {
        expected::record(&state, &n(who), &account(who)).unwrap();
    }
    e.rs.save_ledger(&RotateLedger {
        live: Some("work".into()),
        account: Some("u-work".into()),
        swapped_at: Some(now() - 1200),
        ..RotateLedger::default()
    })
    .unwrap();
    e
}

fn context(e: &Env) -> Ctx {
    let settings = settings::resolve(
        &super::TuneFlags::default(),
        None,
        &|_| None,
        &crate::commands::profile::rotate::settings::no_config,
    )
    .unwrap();
    let mut ctx = Ctx::new(e.store.clone(), e.home.clone(), settings);
    ctx.scan = |_| Liveness::Known(Default::default());
    ctx.identity = read_fake;
    ctx
}

#[test]
fn switch_uses_injected_login_identity_instead_of_the_session_account() {
    let e = fixture();
    private(
        &e.home.join(".claude.json"),
        r#"{"oauthAccount":{"accountUuid":"u-spare","emailAddress":"spare@example.com"}}"#,
    );
    let switched = switch_in(context(&e), &n("spare")).unwrap().unwrap();
    assert!(matches!(switched, Switched::Moved { .. }), "{switched:?}");
    assert_eq!(ino_at(&e.live()), Some(e.t));
    assert_eq!(ino_at(&e.slot("work")), Some(e.l));
    assert_eq!(ino_at(&e.slot("spare")), None);
    assert_eq!(
        e.rs.load_ledger().unwrap().unwrap().account.as_deref(),
        Some("u-spare")
    );
    assert!(!e.root.join(".rejected-logins").exists());
}

#[test]
fn an_injected_identity_failure_cannot_be_replaced_with_session_evidence() {
    let e = fixture();
    let before = fs::read(&e.rs.ledger).unwrap();
    let mut ctx = context(&e);
    ctx.identity = |_| Err("fake identity unavailable");
    let result = switch_in(ctx, &n("spare"));
    assert!(
        !matches!(result, Ok(Ok(Switched::Moved { .. }))),
        "{result:?}"
    );
    e.assert_unchanged();
    assert_eq!(fs::read(&e.rs.ledger).unwrap(), before);
}

/// Simulate one legitimate refresh replacing the outgoing fake login
/// while its identity read is in flight. A result from the old inode must
/// not authorize moving the replacement; a later fresh reading may do so.
fn refresh_during_read(dir: &Path) -> Result<Account, &'static str> {
    let found = read_fake(dir)?;
    let marker = dir.join(".task1753-refreshed");
    if dir.file_name().is_some_and(|n| n == ".claude") {
        let reads: usize = fs::read_to_string(&marker)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        if reads == 0 {
            let replacement = dir.join(".task1753-replacement");
            credential(&replacement, "work");
            fs::rename(replacement, dir.join(".credentials.json")).unwrap();
        }
        private(&marker, &(reads + 1).to_string());
    }
    Ok(found)
}

#[test]
fn a_replaced_login_needs_fresh_evidence_but_refresh_does_not_poison_later_swaps() {
    let e = fixture();
    let mut ctx = context(&e);
    ctx.identity = refresh_during_read;
    let first = switch_in(ctx, &n("spare"));
    if matches!(&first, Ok(Ok(Switched::Moved { .. }))) {
        let reads: usize = fs::read_to_string(e.home.join(".claude/.task1753-refreshed"))
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        assert!(
            reads >= 2,
            "the replacement needs a fresh identity read: {first:?}"
        );
        assert_eq!(ino_at(&e.live()), Some(e.t));
        assert!(
            ino_at(&e.slot("work")).is_some(),
            "refreshed login conserved"
        );
        assert_ne!(ino_at(&e.slot("work")), Some(e.l));
        assert_eq!(ino_at(&e.slot("spare")), None);
        assert!(
            read_fake(&e.root.join("work"))
                .unwrap()
                .same(&account("work"))
        );
        assert!(!e.root.join(".rejected-logins").exists());
        return;
    }
    assert!(
        !matches!(first, Ok(Ok(Switched::Moved { .. }))),
        "{first:?}"
    );
    assert!(!e.slot("work").exists());
    assert_eq!(ino_at(&e.slot("spare")), Some(e.t));
    let replacement = ino_at(&e.live()).unwrap();
    assert_ne!(
        replacement, e.l,
        "the fake refresh must actually replace the inode"
    );
    let second = switch_in(context(&e), &n("spare")).unwrap().unwrap();
    assert!(matches!(second, Switched::Moved { .. }), "{second:?}");
    assert_eq!(ino_at(&e.live()), Some(e.t));
    assert_eq!(ino_at(&e.slot("work")), Some(replacement));
    assert!(!e.root.join(".rejected-logins").exists());
}

#[test]
fn interrupted_completion_records_the_incoming_login_despite_stale_session_json() {
    let e = fixture();
    e.exchanged();
    e.rs.save_journal(&e.journal(Phase::S1)).unwrap();
    private(
        &e.root.join("spare/.claude.json"),
        r#"{"oauthAccount":{"accountUuid":"u-work","emailAddress":"work@example.com"}}"#,
    );
    let result = switch_in(context(&e), &n("spare")).unwrap().unwrap();
    assert!(matches!(result, Switched::AlreadyLive(_)), "{result:?}");
    assert_eq!(ino_at(&e.live()), Some(e.t));
    assert_eq!(ino_at(&e.slot("work")), Some(e.l));
    assert_eq!(ino_at(&e.slot("spare")), None);
    assert_eq!(
        e.rs.load_ledger().unwrap().unwrap().account.as_deref(),
        Some("u-spare")
    );
    assert!(!e.rs.journal.exists());
}

#[test]
fn interrupted_recovery_without_trusted_evidence_retains_logins_and_journal() {
    let e = fixture();
    e.exchanged();
    e.rs.save_journal(&e.journal(Phase::S1)).unwrap();
    let before = fs::read(&e.rs.ledger).unwrap();
    let mut ctx = context(&e);
    ctx.identity = |_| Err("fake identity unavailable");
    let result = switch_in(ctx, &n("spare"));
    assert!(
        !matches!(result, Ok(Ok(Switched::Moved { .. }))),
        "{result:?}"
    );
    assert_eq!(ino_at(&e.live()), Some(e.t));
    assert_eq!(ino_at(&e.slot("spare")), Some(e.l));
    assert_eq!(ino_at(&e.slot("work")), None);
    assert!(
        e.rs.journal.exists(),
        "unresolved provenance retains the journal"
    );
    assert_eq!(fs::read(&e.rs.ledger).unwrap(), before);
}

#[path = "tests_identity_edges.rs"]
mod edges;
