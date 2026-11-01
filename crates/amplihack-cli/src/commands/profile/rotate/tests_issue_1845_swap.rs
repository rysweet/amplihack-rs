//! Issue #1845 (D2, D4; AC3, AC5, AC7): the swap never saves the login in
//! `~/.claude` into a profile whose recorded account is another one, and it
//! records an account only where the evidence says it is right. Uses the
//! fixture in `tests_swap.rs`: `~/.claude` holds `work`'s login (`u-work`,
//! `work@example.com`) and `spare` has its own (`u-spare`).
//!
//! Contract:
//! - `swap_expect::ACCOUNT_MISMATCH ==
//!   "the login in ~/.claude is not the account recorded for profile"`, one
//!   of `swap::DOCUMENTED` (12 entries).
//! - `swap(.., Source::Profile(from), to, ..)` refuses with
//!   `RotateRefused("{ACCOUNT_MISMATCH} {from}")` when `from` has a record and
//!   the account in `$HOME/.claude.json` does not match it. The check runs
//!   before J0 and only reads: logins, locks, journal, ledger and records
//!   are left as they were.
//! - No check for `Source::Adopt`, for a profile without a record, or when
//!   `$HOME/.claude.json` names no account.
//! - After a completed swap: `Adopt` records the outgoing account for the
//!   new `default-original[-N]`; a `from` without a record is recorded only
//!   when the ledger read before J0 has `live == from` and its `account`
//!   matches the outgoing account. `to` is never recorded, and an existing
//!   record is never overwritten by a swap from that profile.
//! - `swap::is_account_mismatch(&ProfileError) -> bool`.

use super::ledger::RotateLedger;
use super::swap::{Source, swap};
use super::tests_swap::{CREDS, LOCK, NOW, env, ino_at, json, login, n, private};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::State;
use crate::commands::profile::expected::{record, recorded};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::rotate::identify::Account;
use std::fs;
use std::path::{Path, PathBuf};

const MISMATCH: &str = "the login in ~/.claude is not the account recorded for profile";

fn state(root: &Path) -> State {
    State::new(root).unwrap()
}

fn uuid(u: &str) -> Account {
    Account::from_block(&serde_json::json!({ "accountUuid": u })).unwrap()
}

fn email(e: &str) -> Account {
    Account::from_block(&serde_json::json!({ "emailAddress": e })).unwrap()
}

fn rec(root: &Path, p: &str, a: &Account) {
    record(&state(root), &n(p), a).unwrap();
}

fn got(root: &Path, p: &str) -> Option<String> {
    recorded(&state(root), &n(p))
}

fn expected_dir(root: &Path) -> PathBuf {
    root.join(".auto-switch.d").join("expected")
}

/// Every file under `expected/`, with its bytes.
fn records(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<(String, Vec<u8>)> = fs::read_dir(expected_dir(root))
        .map(|r| {
            r.map(|e| {
                let e = e.unwrap();
                (
                    e.file_name().to_string_lossy().into_owned(),
                    fs::read(e.path()).unwrap(),
                )
            })
            .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn ledger_says(rs: &super::ledger::RotateState, live: &str, account: &str) {
    rs.save_ledger(&RotateLedger {
        version: 1,
        live: Some(live.into()),
        account: Some(account.into()),
        since: Some(NOW - 7200),
        swapped_at: Some(NOW - 7200),
        ..RotateLedger::default()
    })
    .unwrap();
}

fn refusal(r: Result<ProfileName, ProfileError>) -> String {
    match r {
        Err(e @ ProfileError::RotateRefused(_)) => {
            assert!(super::swap::is_account_mismatch(&e), "{e}");
            e.to_string()
        }
        other => panic!("expected an account-mismatch refusal, got {other:?}"),
    }
}

fn assert_no_account(s: &str) {
    for secret in [
        "u-work",
        "u-spare",
        "u-recorded",
        "@example.com",
        "SENTINEL",
        "sk-ant",
        "accessToken",
        "SECRET-KEY",
    ] {
        assert!(!s.contains(secret), "{secret:?} in {s}");
    }
}

// --- The constant -----------------------------------------------------------

#[test]
fn the_refusal_reason_is_documented() {
    assert_eq!(super::swap_expect::ACCOUNT_MISMATCH, MISMATCH);
    assert!(super::swap::DOCUMENTED.contains(&MISMATCH));
    assert_eq!(super::swap::DOCUMENTED.len(), 12);
}

#[test]
fn is_account_mismatch_matches_only_its_own_refusal() {
    use super::swap::is_account_mismatch;
    assert!(is_account_mismatch(&ProfileError::RotateRefused(format!(
        "{MISMATCH} work"
    ))));
    assert!(!is_account_mismatch(&ProfileError::RotateRefused(
        "profile work already has a login".into()
    )));
    assert!(!is_account_mismatch(&ProfileError::RotateInconsistent(
        format!("{MISMATCH} work")
    )));
}

// --- AC3: the guard refuses and changes nothing -----------------------------

#[cfg(target_os = "linux")]
#[test]
fn a_swap_is_refused_when_the_live_login_is_not_the_recorded_account() {
    let e = env();
    rec(&e.root, "work", &uuid("u-recorded-other"));
    let before_records = records(&e.root);
    let before_ledger = e.rs.load_ledger().unwrap();
    let msg = refusal(e.swap());
    assert_eq!(msg, format!("swap refused: {MISMATCH} work"));
    assert_no_account(&msg);
    e.assert_unchanged();
    assert!(!e.home.join(".claude").join(LOCK).exists(), "no lock taken");
    assert!(!e.root.join("spare").join(LOCK).exists(), "no lock taken");
    assert_eq!(
        e.rs.load_ledger().unwrap(),
        before_ledger,
        "ledger untouched"
    );
    assert_eq!(records(&e.root), before_records, "records untouched");
    assert_eq!(got(&e.root, "spare"), None);
}

#[cfg(target_os = "linux")]
#[test]
fn the_guard_compares_the_home_account_not_the_profiles_own_file() {
    // Claude may have rewritten `work/.claude.json`; only the record counts.
    let e = env();
    rec(&e.root, "work", &uuid("u-recorded-other"));
    private(
        &e.root.join("work/.claude.json"),
        &serde_json::json!({"oauthAccount": {"accountUuid": "u-recorded-other"}}).to_string(),
    );
    refusal(e.swap());
    e.assert_unchanged();
}

#[cfg(target_os = "linux")]
#[test]
fn the_guard_refuses_whatever_the_ledger_says() {
    let e = env();
    ledger_says(&e.rs, "work", "u-work");
    rec(&e.root, "work", &uuid("u-recorded-other"));
    refusal(e.swap());
    e.assert_unchanged();
    assert_eq!(got(&e.root, "work").as_deref(), Some("u-recorded-other"));
}

#[cfg(target_os = "linux")]
#[test]
fn the_refusal_names_the_from_profile() {
    let e = env();
    e.swap().unwrap();
    // `spare`'s login is now live; `spare` is recorded as another account.
    rec(&e.root, "spare", &uuid("u-recorded-other"));
    let msg = refusal(swap(
        &e.store,
        &e.rs,
        &e.home,
        Source::Profile(n("spare")),
        &n("work"),
        NOW + 10,
    ));
    assert_eq!(msg, format!("swap refused: {MISMATCH} spare"));
    assert_eq!(ino_at(&e.live()), Some(e.t), "spare's login stays live");
    assert_eq!(ino_at(&e.slot("work")), Some(e.l));
}

#[cfg(target_os = "linux")]
#[test]
fn the_guard_runs_after_the_existing_refusals() {
    // A source that already has a login is refused for that, first.
    let e = env();
    rec(&e.root, "work", &uuid("u-recorded-other"));
    login(&e.slot("work"));
    match e.swap() {
        Err(ProfileError::RotateRefused(m)) => {
            assert!(m.contains("already has a login"), "{m}");
        }
        other => panic!("{other:?}"),
    }
}

// --- AC5: swaps that go ahead -----------------------------------------------

#[cfg(target_os = "linux")]
#[test]
fn a_swap_goes_ahead_when_the_record_matches() {
    let e = env();
    rec(&e.root, "work", &uuid("u-work"));
    assert_eq!(e.swap().unwrap(), n("work"));
    e.assert_finished();
    assert_eq!(got(&e.root, "work").as_deref(), Some("u-work"));
    assert_eq!(
        got(&e.root, "spare"),
        None,
        "the incoming side is never recorded"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_swap_goes_ahead_when_an_email_record_matches_in_another_case() {
    let e = env();
    rec(&e.root, "work", &email("WORK@Example.com"));
    e.swap().unwrap();
    e.assert_finished();
}

#[cfg(target_os = "linux")]
#[test]
fn a_swap_goes_ahead_without_a_record() {
    let e = env();
    e.swap().unwrap();
    e.assert_finished();
}

#[cfg(target_os = "linux")]
#[test]
fn a_damaged_record_is_no_record_and_does_not_refuse() {
    let e = env();
    rec(&e.root, "work", &uuid("u-recorded-other"));
    private(&expected_dir(&e.root).join("work.json"), "not json");
    e.swap().unwrap();
    e.assert_finished();
}

/// Review of PR #1849: a record that exists but cannot be read is still no
/// record, but the guard warns once, naming the profile and no account. A
/// missing record stays silent.
#[cfg(target_os = "linux")]
#[test]
fn an_unreadable_record_warns_without_naming_the_account() {
    use super::swap_expect::{decide, unchecked};
    let e = env();
    let from = Source::Profile(n("work"));
    let mut note = None;
    decide(&e.store, &e.rs, &e.home, &from, &mut note).unwrap();
    assert_eq!(note, None, "no record: no warning");

    rec(&e.root, "work", &uuid("u-recorded-other"));
    private(&expected_dir(&e.root).join("work.json"), "not json");
    let mut note = None;
    decide(&e.store, &e.rs, &e.home, &from, &mut note).unwrap();
    let note = note.expect("an unreadable record warns");
    assert_eq!(note, unchecked(&n("work")));
    assert!(note.contains("profile 'work'"), "{note}");
    assert_no_account(&note);

    private(
        &expected_dir(&e.root).join("work.json"),
        &serde_json::json!({ "account": 7 }).to_string(),
    );
    let mut note = None;
    decide(&e.store, &e.rs, &e.home, &from, &mut note).unwrap();
    assert_eq!(note, Some(unchecked(&n("work"))), "no usable key: warns");
}

#[cfg(target_os = "linux")]
#[test]
fn a_home_without_an_account_is_not_checked() {
    let e = env();
    rec(&e.root, "work", &uuid("u-recorded-other"));
    private(
        &e.home.join(".claude.json"),
        &serde_json::json!({"projects": {}}).to_string(),
    );
    assert_eq!(e.swap().unwrap(), n("work"));
    assert_eq!(ino_at(&e.live()), Some(e.t));
    assert_eq!(ino_at(&e.slot("work")), Some(e.l));
    assert_eq!(
        got(&e.root, "work").as_deref(),
        Some("u-recorded-other"),
        "an existing record is never overwritten by a swap"
    );
}

// --- AC7: what a swap records ----------------------------------------------

#[cfg(target_os = "linux")]
#[test]
fn a_swap_without_a_ledger_records_nothing() {
    let e = env();
    assert_eq!(e.rs.load_ledger().unwrap(), None);
    e.swap().unwrap();
    assert_eq!(got(&e.root, "work"), None);
    assert_eq!(got(&e.root, "spare"), None);
    assert!(records(&e.root).is_empty(), "{:?}", records(&e.root));
}

#[cfg(target_os = "linux")]
#[test]
fn a_swap_backfills_from_when_the_ledger_shows_the_login_came_from_it() {
    let e = env();
    ledger_says(&e.rs, "work", "u-work");
    e.swap().unwrap();
    e.assert_finished();
    assert_eq!(got(&e.root, "work").as_deref(), Some("u-work"));
    assert_eq!(
        got(&e.root, "spare"),
        None,
        "the incoming side is never recorded"
    );
    let path = expected_dir(&e.root).join("work.json");
    assert_eq!(
        json(&path),
        serde_json::json!({"account": "u-work", "org": null})
    );
    use std::os::unix::fs::MetadataExt;
    assert_eq!(fs::symlink_metadata(&path).unwrap().mode() & 0o777, 0o600);
}

#[cfg(target_os = "linux")]
#[test]
fn no_backfill_when_the_ledger_names_another_live_profile() {
    let e = env();
    ledger_says(&e.rs, "spare", "u-work");
    e.swap().unwrap();
    assert_eq!(got(&e.root, "work"), None);
    assert_eq!(got(&e.root, "spare"), None);
}

#[cfg(target_os = "linux")]
#[test]
fn no_backfill_when_the_ledger_account_is_not_the_outgoing_one() {
    // A stranger's login in `~/.claude` is never accepted as `work`'s.
    let e = env();
    ledger_says(&e.rs, "work", "u-someone-else");
    e.swap().unwrap();
    assert_eq!(got(&e.root, "work"), None);
}

#[cfg(target_os = "linux")]
#[test]
fn no_backfill_when_the_ledger_has_no_account() {
    let e = env();
    e.rs.save_ledger(&RotateLedger {
        version: 1,
        live: Some("work".into()),
        ..RotateLedger::default()
    })
    .unwrap();
    e.swap().unwrap();
    assert_eq!(got(&e.root, "work"), None);
}

#[cfg(target_os = "linux")]
#[test]
fn no_backfill_when_the_ledger_cannot_be_read() {
    let e = env();
    fs::write(&e.rs.ledger, "not json").unwrap();
    let _ = e.swap();
    assert_eq!(got(&e.root, "work"), None);
    assert_eq!(got(&e.root, "spare"), None);
}

#[cfg(target_os = "linux")]
#[test]
fn no_backfill_when_home_has_no_account() {
    let e = env();
    ledger_says(&e.rs, "work", "u-work");
    private(
        &e.home.join(".claude.json"),
        &serde_json::json!({"projects": {}}).to_string(),
    );
    e.swap().unwrap();
    assert_eq!(got(&e.root, "work"), None);
}

#[cfg(target_os = "linux")]
#[test]
fn swapping_back_backfills_the_outgoing_profile_and_never_the_incoming_one() {
    // No ledger at first, so the first swap records nothing. Its ledger
    // names `spare` live with `u-spare`, so the swap back records `spare`
    // (outgoing) and still nothing for `work` (incoming).
    let e = env();
    e.swap().unwrap();
    assert!(records(&e.root).is_empty(), "{:?}", records(&e.root));
    swap(
        &e.store,
        &e.rs,
        &e.home,
        Source::Profile(n("spare")),
        &n("work"),
        NOW + 10,
    )
    .unwrap();
    assert_eq!(got(&e.root, "spare").as_deref(), Some("u-spare"));
    assert_eq!(
        got(&e.root, "work"),
        None,
        "the incoming side is never recorded"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn an_adopting_swap_records_the_new_profile() {
    let e = env();
    let from = swap(&e.store, &e.rs, &e.home, Source::Adopt, &n("spare"), NOW).unwrap();
    assert_eq!(from, n("default-original"));
    assert_eq!(got(&e.root, "default-original").as_deref(), Some("u-work"));
    assert_eq!(got(&e.root, "spare"), None);
    let path = expected_dir(&e.root).join("default-original.json");
    assert_eq!(
        json(&path),
        serde_json::json!({"account": "u-work", "org": null})
    );
}

#[cfg(target_os = "linux")]
#[test]
fn an_adopting_swap_is_not_checked_and_replaces_a_stale_record() {
    // D8: a record left behind by a removed `default-original` names another
    // account. Adoption is not guarded, and records the adopted login.
    let e = env();
    rec(&e.root, "default-original", &uuid("u-recorded-other"));
    let from = swap(&e.store, &e.rs, &e.home, Source::Adopt, &n("spare"), NOW).unwrap();
    assert_eq!(from, n("default-original"));
    assert_eq!(
        ino_at(&e.root.join("default-original").join(CREDS)),
        Some(e.l)
    );
    assert_eq!(got(&e.root, "default-original").as_deref(), Some("u-work"));
}

#[cfg(target_os = "linux")]
#[test]
fn a_refused_adopting_swap_records_nothing() {
    let e = env();
    fs::create_dir(e.home.join(".claude").join(LOCK)).unwrap();
    assert!(swap(&e.store, &e.rs, &e.home, Source::Adopt, &n("spare"), NOW).is_err());
    assert!(records(&e.root).is_empty(), "{:?}", records(&e.root));
}

#[cfg(target_os = "linux")]
#[test]
fn a_failed_record_write_does_not_fail_the_swap() {
    // `expected` is a file, so no record can be written; the swap has
    // completed and still reports success.
    let e = env();
    ledger_says(&e.rs, "work", "u-work");
    fs::create_dir_all(e.root.join(".auto-switch.d")).unwrap();
    fs::write(expected_dir(&e.root), "in the way").unwrap();
    assert_eq!(e.swap().unwrap(), n("work"));
    e.assert_finished();
}

// --- Organizations and the backfill gate (#1845 review) -------------------

fn uuid_in(u: &str, org: &str) -> Account {
    Account::from_block(&serde_json::json!({ "accountUuid": u, "organizationUuid": org })).unwrap()
}

/// Adds profile `p` signed in to `body`'s account.
fn add_profile(e: &super::tests_swap::Env, p: &str, body: serde_json::Value) {
    e.store.add(&n(p)).unwrap();
    private(
        &e.root.join(p).join(".claude.json"),
        &serde_json::json!({ "oauthAccount": body }).to_string(),
    );
    login(&e.root.join(p).join(CREDS));
}

#[cfg(target_os = "linux")]
#[test]
fn the_same_user_in_another_organization_is_refused() {
    // `work` is recorded in its team organization; `~/.claude` holds the
    // same user's login without it, as in another organization.
    let e = env();
    rec(&e.root, "work", &uuid_in("u-work", "o-team"));
    let before = records(&e.root);
    let msg = refusal(e.swap());
    assert_eq!(msg, format!("swap refused: {MISMATCH} work"));
    assert_eq!(records(&e.root), before, "nothing recorded");
}

#[cfg(target_os = "linux")]
#[test]
fn a_record_in_the_same_organization_lets_the_swap_through() {
    let e = env();
    private(
        &e.home.join(".claude.json"),
        &serde_json::json!({ "oauthAccount": {
            "accountUuid": "u-work", "organizationUuid": "o-team"
        }})
        .to_string(),
    );
    rec(&e.root, "work", &uuid_in("u-work", "o-team"));
    e.swap().unwrap();
    e.assert_finished();
}

#[cfg(target_os = "linux")]
#[test]
fn no_backfill_when_another_profile_holds_the_outgoing_account() {
    // The #1845 incident: a folder already holds another account's login.
    // `bbuck` holds `u-work` too, so which of them owns it is not known.
    let e = env();
    add_profile(
        &e,
        "bbuck",
        serde_json::json!({"accountUuid": "u-work", "emailAddress": "work@example.com"}),
    );
    ledger_says(&e.rs, "work", "u-work");
    e.swap().unwrap();
    e.assert_finished();
    assert_eq!(got(&e.root, "work"), None);
    assert!(records(&e.root).is_empty(), "{:?}", records(&e.root));
}

#[cfg(target_os = "linux")]
#[test]
fn no_backfill_when_another_profile_has_the_outgoing_account_recorded() {
    let e = env();
    rec(&e.root, "spare", &uuid("u-work"));
    ledger_says(&e.rs, "work", "u-work");
    e.swap().unwrap();
    e.assert_finished();
    assert_eq!(got(&e.root, "work"), None);
}

#[cfg(target_os = "linux")]
#[test]
fn another_profile_in_another_organization_does_not_stop_the_backfill() {
    let e = env();
    add_profile(
        &e,
        "personal",
        serde_json::json!({"accountUuid": "u-work", "organizationUuid": "o-personal"}),
    );
    ledger_says(&e.rs, "work", "u-work");
    e.swap().unwrap();
    e.assert_finished();
    assert_eq!(got(&e.root, "work").as_deref(), Some("u-work"));
}
