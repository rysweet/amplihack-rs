//! Issues #2911 and #2909 (emitter): `note_limit_block` records any limit
//! kind for the login a step ran under.
//!
//! Contract (these tests fail until it exists):
//! - `step_block::note_limit_block(root, login, at, until: Option<i64>,
//!   kind: LimitKind)` returns `()` and replaces `note_spend_block`.
//! - The record is `overflow_blocked[login] = { at, until, reason:
//!   "spend_limit" }` for every kind, so an older reader still loads it.
//! - `until == None`: `Spend` falls back to `spend_reset(at)`; every other
//!   kind writes nothing and reports `no stated reset`.
//! - `msg::alert_step_limit_block(name, kind, until)` for `Spend` equals the
//!   1.2.193 spend line byte for byte; `msg::step_limit_block_not_written
//!   (name, kind, why)` names the kind.
//! - An active block stands; a missing ledger is never created.
//!
//! Wired from `tests_step_block.rs` with `#[path = ...] mod kinds;`, because
//! `rotate/mod.rs` is at the 400-line budget `tests_source_scan.rs` holds it
//! to. The path form resolves beside this file.

use super::super::ledger::{
    MAX_BLOCKS, OverflowBlock, RotateLedger, RotateState, SPEND_LIMIT, spend_reset,
};
use super::super::log::{msg, timestamp};
use super::super::step_block::note_limit_block;
use crate::commands::profile::name::ProfileName;
use crate::commands::recipe::run::limit_patterns::LimitKind;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

const NOW: i64 = 1_760_000_000;
const DAY: i64 = 86_400;
const WEEK: i64 = 7 * DAY;

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

struct Root {
    _tmp: tempfile::TempDir,
    path: PathBuf,
    rs: RotateState,
}

fn root() -> Root {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = tmp.path().to_path_buf();
    let rs = RotateState::new(&path);
    rs.ensure().unwrap();
    rs.save_ledger(&RotateLedger::default()).unwrap();
    Root {
        _tmp: tmp,
        path,
        rs,
    }
}

fn block(r: &Root, login: &str) -> Option<OverflowBlock> {
    r.rs.load_ledger()
        .unwrap()
        .unwrap()
        .overflow_blocked
        .get(login)
        .cloned()
}

fn log_text(r: &Root) -> String {
    fs::read_to_string(&r.rs.log).unwrap_or_default()
}

#[test]
fn every_kind_with_a_stated_reset_is_stored_with_the_spend_reason() {
    for kind in [
        LimitKind::Session,
        LimitKind::Weekly,
        LimitKind::Spend,
        LimitKind::Credits,
    ] {
        let r = root();
        note_limit_block(&r.path, &n("wwest"), NOW, Some(NOW + 3 * DAY), kind);
        let b = block(&r, "wwest").unwrap_or_else(|| panic!("{kind:?}: no block"));
        assert_eq!((b.at, b.until), (NOW, NOW + 3 * DAY), "{kind:?}");
        assert_eq!(b.reason, SPEND_LIMIT, "{kind:?}: older readers need this");
    }
}

#[test]
fn the_stored_ledger_has_no_field_an_older_reader_would_refuse() {
    let r = root();
    note_limit_block(
        &r.path,
        &n("wwest"),
        NOW,
        Some(NOW + WEEK),
        LimitKind::Weekly,
    );
    let raw: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&r.rs.ledger).unwrap()).unwrap();
    let rec = &raw["overflow_blocked"]["wwest"];
    assert_eq!(rec["reason"], "spend_limit");
    let mut keys: Vec<_> = rec.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["at", "reason", "until"]);
}

#[test]
fn a_spend_limit_with_no_stated_reset_falls_back_to_the_monthly_reset() {
    let r = root();
    note_limit_block(&r.path, &n("wwest"), NOW, None, LimitKind::Spend);
    assert_eq!(block(&r, "wwest").unwrap().until, spend_reset(NOW));
}

#[test]
fn a_session_or_weekly_limit_with_no_stated_reset_writes_nothing() {
    for kind in [LimitKind::Session, LimitKind::Weekly, LimitKind::Credits] {
        let r = root();
        note_limit_block(&r.path, &n("wwest"), NOW, None, kind);
        assert!(block(&r, "wwest").is_none(), "{kind:?}");
        let log = log_text(&r);
        assert!(log.contains("no stated reset"), "{kind:?}: {log}");
        assert!(log.contains("wwest"), "{log}");
    }
}

#[test]
fn the_spend_alert_is_the_one_1_2_193_wrote() {
    assert_eq!(
        msg::alert_step_limit_block(&n("wwest"), LimitKind::Spend, NOW + WEEK),
        format!(
            "ALERT: login wwest hit its spend limit; blocked until {}",
            timestamp(NOW + WEEK)
        )
    );
}

#[test]
fn each_kind_has_its_own_alert_wording() {
    let lines: Vec<String> = [
        LimitKind::Session,
        LimitKind::Weekly,
        LimitKind::Spend,
        LimitKind::Credits,
    ]
    .iter()
    .map(|&k| msg::alert_step_limit_block(&n("wwest"), k, NOW + DAY))
    .collect();
    for l in &lines {
        assert!(l.starts_with("ALERT: login wwest hit its "), "{l}");
        assert!(l.contains(&timestamp(NOW + DAY)), "{l}");
    }
    let mut unique = lines.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), lines.len(), "{lines:?}");
}

#[test]
fn the_weekly_block_is_logged_once_and_never_extended() {
    let r = root();
    note_limit_block(
        &r.path,
        &n("wwest"),
        NOW,
        Some(NOW + WEEK),
        LimitKind::Weekly,
    );
    note_limit_block(
        &r.path,
        &n("wwest"),
        NOW + 60,
        Some(NOW + 2 * WEEK),
        LimitKind::Weekly,
    );
    assert_eq!(block(&r, "wwest").unwrap().until, NOW + WEEK);
    let first = msg::alert_step_limit_block(&n("wwest"), LimitKind::Weekly, NOW + WEEK);
    assert_eq!(log_text(&r).matches(&first).count(), 1, "{}", log_text(&r));
}

#[test]
fn a_reset_that_would_make_the_ledger_unreadable_is_not_stored() {
    for until in [
        Some(i64::MAX),
        Some(NOW - DAY),
        Some(NOW),
        Some(i64::MIN),
        Some(NOW + 400 * DAY),
    ] {
        let r = root();
        note_limit_block(&r.path, &n("wwest"), NOW, until, LimitKind::Weekly);
        assert!(block(&r, "wwest").is_none(), "{until:?}");
        assert!(
            r.rs.load_ledger().is_ok(),
            "{until:?}: the ledger still loads"
        );
        assert!(log_text(&r).contains("not written") || log_text(&r).contains("not blocked"));
    }
}

#[test]
fn the_cardinality_cap_holds_for_every_kind() {
    let r = root();
    for i in 0..MAX_BLOCKS + 3 {
        note_limit_block(
            &r.path,
            &n(&format!("login{i}")),
            NOW,
            Some(NOW + DAY),
            LimitKind::Session,
        );
    }
    let l = r.rs.load_ledger().unwrap().unwrap();
    assert!(l.overflow_blocked.len() <= MAX_BLOCKS);
}

#[test]
fn a_missing_ledger_is_never_created_for_any_kind() {
    for kind in [LimitKind::Session, LimitKind::Weekly, LimitKind::Credits] {
        let tmp = tempfile::tempdir().unwrap();
        fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        note_limit_block(tmp.path(), &n("wwest"), NOW, Some(NOW + DAY), kind);
        assert!(!RotateState::new(tmp.path()).ledger.exists(), "{kind:?}");
    }
}
