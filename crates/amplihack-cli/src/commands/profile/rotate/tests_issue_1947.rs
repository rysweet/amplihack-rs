//! Issue #1947: `load_ledger` validates `login_identity` and refuses a block
//! that is not a well-formed account identity, with the fixed message
//! `rotate ledger: invalid login_identity`.
//!
//! Rule (`Identity::valid`): the value is a JSON object; each of
//! `accountUuid`, `organizationUuid` and `emailAddress`, when present, is a
//! string that passes `safe_account`; `emailAddress` holds exactly one `@`
//! with a non-empty part on each side and no whitespace; `accountUuid` or
//! `emailAddress` is present; other keys are ignored. An absent key or a
//! top-level `null` still loads as `None`.
//!
//! The readers that feed the writers (`Account::from_profile`,
//! `Account::from_block`) apply the same e-mail rule, so a stored identity
//! always loads again; the tests at the end pin that.
//!
//! Every refused case checks the exact message, that the value never reaches
//! Display or Debug, and that the file is left byte-identical. Assertion
//! messages name only the case index, never a body.

use super::identify::Account;
use super::ledger::{RotateLedger, RotateState};
use super::login_identity::Identity;
use crate::commands::profile::ProfileError;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;

const SENTINEL: &str = "S1947SENTINEL";
const MESSAGE: &str = "rotate ledger: invalid login_identity";

struct Root {
    _tmp: tempfile::TempDir,
    rs: RotateState,
}

fn root() -> Root {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let rs = RotateState::new(tmp.path());
    rs.ensure().unwrap();
    Root { _tmp: tmp, rs }
}

/// Writes the ledger privately, so `trusted()` never refuses it first.
fn write_raw(r: &Root, body: &str) {
    fs::write(&r.rs.ledger, body).unwrap();
    fs::set_permissions(&r.rs.ledger, fs::Permissions::from_mode(0o600)).unwrap();
}

/// A ledger that is valid apart from `login_identity`.
fn body(identity: Value) -> Value {
    json!({"version": 1, "live": "work", "account": "uuid-work", "login_identity": identity})
}

fn full() -> Value {
    json!({"accountUuid": "uuid-work", "organizationUuid": "org-work",
        "emailAddress": "work@example.com"})
}

fn identity(v: Value) -> Identity {
    serde_json::from_value(v).unwrap()
}

// --- Accepted ----------------------------------------------------------------

#[test]
fn a_block_written_by_amplihack_is_valid_and_round_trips() {
    let account = Account::from_block(&full()).unwrap();
    assert!(Identity::new(&account).valid());
    let r = root();
    let l = RotateLedger {
        live: Some("work".into()),
        account: Some("uuid-work".into()),
        login_identity: Some(Identity::new(&account)),
        ..RotateLedger::default()
    };
    r.rs.save_ledger(&l).unwrap();
    assert_eq!(r.rs.load_ledger().unwrap(), Some(l));
}

#[test]
fn an_absent_key_or_a_top_level_null_loads_as_none() {
    for (i, v) in [
        json!({"version": 1, "live": "work", "account": "uuid-work"}),
        body(Value::Null),
    ]
    .into_iter()
    .enumerate()
    {
        let r = root();
        write_raw(&r, &v.to_string());
        let l = r.rs.load_ledger().unwrap_or_else(|_| panic!("case {i}"));
        assert!(l.unwrap().login_identity.is_none(), "case {i}");
    }
}

#[test]
fn well_formed_blocks_load() {
    let long_uuid = "u".repeat(320);
    for (i, v) in [
        json!({"emailAddress": "work@example.com"}),
        json!({"organizationUuid": "org-work", "emailAddress": "work@example.com"}),
        json!({"accountUuid": "uuid-work", "note": "an unknown key", "extra": [1, 2]}),
        json!({"accountUuid": "uuid-work", "emailAddress": "Work.Person@Example.COM"}),
        json!({"accountUuid": long_uuid}),
        full(),
    ]
    .into_iter()
    .enumerate()
    {
        let r = root();
        write_raw(&r, &body(v.clone()).to_string());
        let l = r.rs.load_ledger().unwrap_or_else(|_| panic!("case {i}"));
        assert_eq!(
            l.and_then(|l| l.login_identity),
            Some(identity(v)),
            "case {i}"
        );
    }
}

// --- Refused -----------------------------------------------------------------

fn refused() -> Vec<Value> {
    let s = SENTINEL;
    let uuid = "uuid-work";
    vec![
        // Not an object.
        json!(s),
        json!([s]),
        json!(1947),
        json!(true),
        // No account and no e-mail.
        json!({}),
        json!({"note": s}),
        json!({"organizationUuid": format!("org-{s}")}),
        // `accountUuid` not a safe string.
        json!({"accountUuid": 1947, "emailAddress": "work@example.com", "note": s}),
        json!({"accountUuid": "", "emailAddress": "work@example.com", "note": s}),
        json!({"accountUuid": format!("{s}{}", "u".repeat(320))}),
        // `organizationUuid` not a safe string (AZ-1: it must never read as
        // "no organisation").
        json!({"accountUuid": uuid, "organizationUuid": format!("org-{s}\u{0007}")}),
        json!({"accountUuid": uuid, "organizationUuid": format!("org-{s}\u{202e}")}),
        json!({"accountUuid": uuid, "organizationUuid": format!("org-{s}\u{2066}")}),
        json!({"accountUuid": uuid, "organizationUuid": 7, "note": s}),
        json!({"accountUuid": uuid, "organizationUuid": "", "note": s}),
        // A known key holding `null`.
        json!({"accountUuid": uuid, "organizationUuid": null, "note": s}),
        json!({"accountUuid": null, "emailAddress": format!("{s}@example.com")}),
        json!({"accountUuid": uuid, "emailAddress": null, "note": s}),
        // `emailAddress` not a safe string or not shaped as an address.
        json!({"emailAddress": 1947, "note": s}),
        json!({"emailAddress": format!("{s}-no-at")}),
        json!({"emailAddress": format!("{s}@a@example.com")}),
        json!({"emailAddress": format!("@{s}.example.com")}),
        json!({"emailAddress": format!("{s}@")}),
        json!({"emailAddress": "@", "note": s}),
        json!({"emailAddress": format!("{s} x@example.com")}),
        json!({"emailAddress": format!("{s}\u{00a0}x@example.com")}),
        json!({"emailAddress": format!("{s}@example.com\t")}),
        json!({"emailAddress": format!("{s}@example.com\u{0007}")}),
        // A valid account does not excuse a malformed e-mail beside it.
        json!({"accountUuid": uuid, "emailAddress": format!("{s}-no-at")}),
    ]
}

#[test]
fn malformed_blocks_are_refused_with_the_fixed_message() {
    for (i, v) in refused().into_iter().enumerate() {
        let r = root();
        write_raw(&r, &body(v).to_string());
        let before = fs::read(&r.rs.ledger).unwrap();
        let err = match r.rs.load_ledger() {
            Err(e) => e,
            Ok(_) => panic!("case {i}: loaded"),
        };
        assert!(
            matches!(&err, ProfileError::RotateInconsistent(m) if m == MESSAGE),
            "case {i}: wrong error"
        );
        let shown = format!("rotate state is inconsistent: {MESSAGE}");
        assert!(err.to_string() == shown, "case {i}: Display text");
        assert!(!err.to_string().contains(SENTINEL), "case {i}: Display");
        assert!(!format!("{err:?}").contains(SENTINEL), "case {i}: Debug");
        assert_eq!(fs::read(&r.rs.ledger).unwrap(), before, "case {i}: bytes");
    }
}

#[test]
fn a_refused_ledger_is_refused_again_and_never_rewritten() {
    let r = root();
    write_raw(&r, &body(json!({"emailAddress": "no-at"})).to_string());
    let before = fs::read(&r.rs.ledger).unwrap();
    for _ in 0..2 {
        assert!(r.rs.load_ledger().is_err());
    }
    assert_eq!(fs::read(&r.rs.ledger).unwrap(), before);
}

// --- Identity::valid directly ------------------------------------------------

#[test]
fn identity_valid_applies_the_rule() {
    assert!(identity(full()).valid());
    assert!(identity(json!({"emailAddress": "a@b"})).valid());
    assert!(!identity(json!({"organizationUuid": "org-work"})).valid());
    assert!(!identity(json!({"accountUuid": "uuid-work", "organizationUuid": 7})).valid());
    assert!(!identity(json!({"emailAddress": "a@b@c"})).valid());
    assert!(!identity(json!("uuid-work")).valid());
}

// --- Writers follow the loader's rule ----------------------------------------
//
// `swap_finish` and `handfix` store `Identity::new(account)` for an account
// read by `Account::from_profile` (the server answer) or `Account::from_block`
// (`.claude.json`). Both readers apply the loader's e-mail rule, so a stored
// identity always passes `Identity::valid` and the next `load_ledger` accepts
// it. These tests pin that invariant over every e-mail the refusal table uses
// and over values that lower-casing would push past the 320-byte limit.

/// Every string e-mail from the refusal table, plus inputs that only fail
/// after lower-casing and a few the readers must still accept.
fn writer_emails() -> Vec<String> {
    let mut v: Vec<String> = refused()
        .iter()
        .filter_map(|b| b.get("emailAddress")?.as_str().map(str::to_string))
        .collect();
    assert!(v.len() >= 10, "the refusal table lost its e-mail cases");
    // 306 bytes as sent; 456 bytes once `İ` (2 bytes) lowers to `i̇` (3).
    v.push(format!("{}@x.com", "\u{0130}".repeat(150)));
    // Exactly 320 bytes as sent, 321 once lowered.
    v.push(format!("\u{0130}{}@x.com", "a".repeat(312)));
    v.extend(
        [
            "no-at",
            "a b@x.com",
            "a@b",
            "Work.Person@Example.COM",
            "\u{0130}@x.com",
        ]
        .map(String::from),
    );
    v
}

fn profile(uuid: Option<&str>, email: &str) -> Value {
    let mut account = json!({"email": email});
    if let Some(u) = uuid {
        account["uuid"] = u.into();
    }
    json!({"account": account, "organization": {"uuid": "org-work"}})
}

fn claude_block(uuid: Option<&str>, email: &str) -> Value {
    let mut b = json!({"emailAddress": email, "organizationUuid": "org-work"});
    if let Some(u) = uuid {
        b["accountUuid"] = u.into();
    }
    b
}

#[test]
fn every_account_a_reader_produces_stores_a_valid_identity() {
    for (i, email) in writer_emails().iter().enumerate() {
        for uuid in [None, Some("uuid-work")] {
            for (src, account) in [
                ("profile", Account::from_profile(&profile(uuid, email))),
                ("block", Account::from_block(&claude_block(uuid, email))),
            ] {
                let Some(account) = account else {
                    assert!(uuid.is_none(), "case {i} {src}: a uuid alone is an account");
                    continue;
                };
                assert!(
                    Identity::new(&account).valid(),
                    "case {i} {src}: writer produced an identity the loader refuses"
                );
            }
        }
    }
}

#[test]
fn a_malformed_server_email_counts_as_missing() {
    for (i, email) in writer_emails().iter().enumerate() {
        let accepted = identity(json!({"emailAddress": email})).valid();
        let read = Account::from_profile(&profile(Some("uuid-work"), email)).unwrap();
        assert_eq!(
            read.email().is_some(),
            accepted,
            "case {i}: reader and loader disagree"
        );
        assert_eq!(
            Account::from_profile(&profile(None, email)).is_some(),
            accepted,
            "case {i}: e-mail-only account"
        );
    }
}

#[test]
fn lower_casing_cannot_push_a_stored_email_over_the_limit() {
    let at_limit = format!("\u{0130}{}@x.com", "a".repeat(312));
    assert_eq!(at_limit.len(), 320);
    assert!(at_limit.to_lowercase().len() > 320);
    let account = Account::from_profile(&profile(Some("uuid-work"), &at_limit)).unwrap();
    assert!(account.email().is_none());
    assert!(!identity(json!({"emailAddress": at_limit})).valid());
    let fits = Account::from_profile(&profile(None, "\u{0130}@x.com")).unwrap();
    assert_eq!(fits.email(), Some("i\u{0307}@x.com"));
}

#[test]
fn a_swap_finish_style_save_of_any_reader_account_loads_again() {
    for (i, email) in writer_emails().iter().enumerate() {
        let Some(account) = Account::from_profile(&profile(Some("uuid-work"), email)) else {
            panic!("case {i}: uuid account missing");
        };
        let r = root();
        let l = RotateLedger {
            live: Some("work".into()),
            account: Some(account.key().to_string()),
            login_identity: Some(Identity::new(&account)),
            ..RotateLedger::default()
        };
        r.rs.save_ledger(&l).unwrap();
        assert!(r.rs.load_ledger().is_ok(), "case {i}: own write refused");
    }
}

// --- The reason reaches the operator -----------------------------------------

/// The refused field is named on the stop line and in `status`; other load
/// failures, whose text may hold a path or a position, are not.
#[test]
fn refused_field_names_only_a_refused_field() {
    use super::ledger::refused_field;
    let r = root();
    write_raw(
        &r,
        &body(json!({"emailAddress": format!("{SENTINEL}-no-at")})).to_string(),
    );
    let e = r.rs.load_ledger().unwrap_err();
    assert_eq!(refused_field(&e), Some("invalid login_identity"));

    write_raw(&r, &json!({"version": 99}).to_string());
    let e = r.rs.load_ledger().unwrap_err();
    assert_eq!(refused_field(&e), Some("unknown version"));

    write_raw(&r, "{not json");
    assert_eq!(refused_field(&r.rs.load_ledger().unwrap_err()), None);

    fs::set_permissions(&r.rs.ledger, fs::Permissions::from_mode(0o666)).unwrap();
    assert_eq!(refused_field(&r.rs.load_ledger().unwrap_err()), None);

    let other = ProfileError::RotateInconsistent("rotate journal: unknown version".into());
    assert_eq!(refused_field(&other), None);
}

#[test]
fn the_stop_line_names_the_field_and_keeps_the_1962_text_otherwise() {
    use super::handfix_msg::ledger_unreadable;
    assert_eq!(
        ledger_unreadable(Some("invalid login_identity"), "readable"),
        "rotate ledger unreadable (invalid login_identity); live identity: readable; nothing moved, no record written"
    );
    assert_eq!(
        ledger_unreadable(None, "token expired"),
        "rotate ledger unreadable; live identity: token expired; nothing moved, no record written"
    );
}

/// The readers are shared beyond rotate (auto-switch, login checks,
/// duplicates, `.default`, the limit-wait fingerprint, the direct usage
/// read). They all reach a malformed e-mail through `Account`, so these pin
/// what each sees: a record keyed by that e-mail matches nothing, an
/// e-mail-only login is no account, and the #1702 swap stop treats such a
/// `~/.claude.json` as unreadable, which stops the swap.
#[test]
fn shared_readers_treat_a_malformed_email_as_missing() {
    use super::account::home_readable;
    let with_uuid = Account::from_block(&claude_block(Some("uuid-work"), "no-at")).unwrap();
    assert!(with_uuid.matches_key("uuid-work"));
    assert!(!with_uuid.matches_key("no-at"));
    assert!(Account::from_block(&claude_block(None, "no-at")).is_none());

    let home = tempfile::tempdir().unwrap();
    let write = |block: Value| {
        fs::write(
            home.path().join(".claude.json"),
            json!({"oauthAccount": block}).to_string(),
        )
        .unwrap();
    };
    write(claude_block(None, "no-at"));
    assert!(!home_readable(home.path()));
    write(claude_block(None, "work@example.com"));
    assert!(home_readable(home.path()));
}
