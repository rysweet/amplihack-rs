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
