//! Issue #1700: the API-key block, the key-error classifier and the two new
//! rotate ledger fields. See "API-key block" in
//! docs/reference/profile-command.md.
//!
//! Contract:
//! - `usage_parse::classify_key_error(text) -> Option<(&'static str,
//!   Option<i64>)>`, matching case-insensitively:
//!   `credit balance is too low`, `insufficient funds` → `key_exhausted`;
//!   `invalid x-api-key`, `authentication_error` → `key_revoked`;
//!   `rate_limit_error`, or `429` together with `rate limit` →
//!   `key_rate_limited`, with the `retry-after` seconds when the text gives
//!   them. Anything else, a spend limit included, is `None`.
//! - `ledger::{LIVE_KIND_PLATFORM, KEY_EXHAUSTED, KEY_REVOKED,
//!   KEY_RATE_LIMITED, KEY_REASONS}`; `OverflowBlock::key(reason, at,
//!   until)`.
//! - `RotateLedger { live_kind: Option<String>, platform_block:
//!   Option<OverflowBlock>, .. }`, both optional and left out when `None`.
//!   `load_ledger` rejects a `live_kind` other than `"platform"`, a
//!   `live_kind` without `live`, a `platform_block` whose reason is not in
//!   `KEY_REASONS`, or whose times fail `0 < at < until <= at +
//!   MAX_BLOCK_SECS`; and an `overflow_blocked` entry with a key reason.
//!   `for_older_readers` keeps both fields.
//! - `platform_block::record(root, reason, until: Option<i64>, now) ->
//!   Result<Recorded, ProfileError>`, `Recorded::{Created,
//!   AlreadyBlocked}`: writes under the rotate state lock only when no block
//!   is active. `key_exhausted` and `key_revoked` last `MAX_BLOCK_SECS`;
//!   `key_rate_limited` lasts until `until`, with a missing or past value
//!   replaced by `now + 3600` and one beyond `MAX_BLOCK_SECS` clamped to it.
//!   Never panics on extreme values. An expired block is replaced.
//! - `platform_block::clear(root) -> Result<bool, ProfileError>` and
//!   `platform_block::active(&RotateLedger, now) -> Option<&OverflowBlock>`.

use super::ledger::{
    KEY_EXHAUSTED, KEY_RATE_LIMITED, KEY_REASONS, KEY_REVOKED, LIVE_KIND_PLATFORM, MAX_BLOCK_SECS,
    OverflowBlock, RotateLedger, RotateState, for_older_readers,
};
use super::platform_block::{Recorded, active, clear, record};
use crate::commands::profile::usage_parse::classify_key_error;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const NOW: i64 = 1_760_000_000;
const HOUR: i64 = 3600;

fn root() -> (tempfile::TempDir, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    fs::set_permissions(t.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let r = t.path().join("root");
    fs::create_dir(&r).unwrap();
    fs::set_permissions(&r, fs::Permissions::from_mode(0o700)).unwrap();
    let r = r.canonicalize().unwrap();
    (t, r)
}

fn ledger(root: &Path) -> RotateLedger {
    RotateState::new(root)
        .load_ledger()
        .unwrap()
        .expect("a ledger")
}

fn raw_ledger(root: &Path, body: &str) -> Result<Option<RotateLedger>, String> {
    let rs = RotateState::new(root);
    rs.ensure().unwrap();
    fs::write(&rs.ledger, body).unwrap();
    fs::set_permissions(&rs.ledger, fs::Permissions::from_mode(0o600)).unwrap();
    rs.load_ledger().map_err(|e| e.to_string())
}

// --- classify_key_error -------------------------------------------------------------

#[test]
fn the_reason_constants_are_the_documented_strings() {
    assert_eq!(LIVE_KIND_PLATFORM, "platform");
    assert_eq!(KEY_EXHAUSTED, "key_exhausted");
    assert_eq!(KEY_REVOKED, "key_revoked");
    assert_eq!(KEY_RATE_LIMITED, "key_rate_limited");
    assert_eq!(KEY_REASONS, [KEY_EXHAUSTED, KEY_REVOKED, KEY_RATE_LIMITED]);
}

#[test]
fn key_errors_are_classified_ignoring_case() {
    for (text, want) in [
        (
            "API Error: 400 {\"type\":\"error\",\"error\":{\"message\":\"Your credit balance is too low to access the Anthropic API.\"}}",
            KEY_EXHAUSTED,
        ),
        ("Insufficient Funds on the account", KEY_EXHAUSTED),
        (
            "API Error: 401 {\"type\":\"error\",\"error\":{\"type\":\"authentication_error\",\"message\":\"invalid x-api-key\"}}",
            KEY_REVOKED,
        ),
        ("Invalid X-API-Key", KEY_REVOKED),
        (
            "API Error: 429 {\"error\":{\"type\":\"rate_limit_error\"}}",
            KEY_RATE_LIMITED,
        ),
        (
            "HTTP 429: Rate limit reached for requests",
            KEY_RATE_LIMITED,
        ),
    ] {
        let got = classify_key_error(text).map(|(r, _)| r);
        assert_eq!(got, Some(want), "{text}");
    }
}

#[test]
fn a_retry_after_is_returned_in_seconds() {
    assert_eq!(
        classify_key_error("rate_limit_error; retry-after: 120"),
        Some((KEY_RATE_LIMITED, Some(120)))
    );
    assert_eq!(
        classify_key_error("rate_limit_error"),
        Some((KEY_RATE_LIMITED, None))
    );
    // A value that does not fit is no value, never a panic.
    assert_eq!(
        classify_key_error("rate_limit_error retry-after: 99999999999999999999999"),
        Some((KEY_RATE_LIMITED, None))
    );
}

#[test]
fn other_text_is_not_a_key_error() {
    for text in [
        "",
        "OK",
        "429",                    // a bare number is not enough
        "you hit the rate limit", // no 429, no rate_limit_error
        "You've hit your org's monthly spend limit · contact your admin",
        "Session limit reached ∙ resets 3pm",
        "Invalid API key · Please run /login",
    ] {
        assert_eq!(classify_key_error(text), None, "{text}");
    }
}

// --- record / clear / active ----------------------------------------------------------

#[test]
fn the_first_record_creates_the_block_and_the_second_does_not() {
    let (_t, r) = root();
    assert_eq!(
        record(&r, KEY_EXHAUSTED, None, NOW).unwrap(),
        Recorded::Created
    );
    let b = ledger(&r).platform_block.expect("block");
    assert_eq!(b.reason, KEY_EXHAUSTED);
    assert_eq!(b.at, NOW);
    assert_eq!(b.until, NOW + MAX_BLOCK_SECS, "until cleared");
    assert_eq!(
        record(&r, KEY_REVOKED, None, NOW + 5).unwrap(),
        Recorded::AlreadyBlocked
    );
    assert_eq!(
        ledger(&r).platform_block.unwrap().reason,
        KEY_EXHAUSTED,
        "the active block is kept"
    );
}

#[test]
fn a_revoked_key_lasts_until_cleared() {
    let (_t, r) = root();
    record(&r, KEY_REVOKED, Some(NOW + 10), NOW).unwrap();
    assert_eq!(
        ledger(&r).platform_block.unwrap().until,
        NOW + MAX_BLOCK_SECS,
        "a given time is ignored for a revoked key"
    );
}

#[test]
fn a_rate_limit_lasts_until_its_time_clamped() {
    for (until, want) in [
        (Some(NOW + 600), NOW + 600),
        (None, NOW + HOUR),
        (Some(NOW), NOW + HOUR),
        (Some(NOW - 5), NOW + HOUR),
        (Some(0), NOW + HOUR),
        (Some(-1), NOW + HOUR),
        (Some(i64::MIN), NOW + HOUR),
        (Some(NOW + MAX_BLOCK_SECS + 1), NOW + MAX_BLOCK_SECS),
        (Some(i64::MAX), NOW + MAX_BLOCK_SECS),
    ] {
        let (_t, r) = root();
        assert_eq!(
            record(&r, KEY_RATE_LIMITED, until, NOW).unwrap(),
            Recorded::Created
        );
        let b = ledger(&r).platform_block.unwrap();
        assert_eq!(b.until, want, "{until:?}");
        assert!(b.at < b.until && b.until <= b.at + MAX_BLOCK_SECS);
    }
}

#[test]
fn an_expired_block_is_replaced() {
    let (_t, r) = root();
    record(&r, KEY_RATE_LIMITED, Some(NOW + 60), NOW).unwrap();
    assert_eq!(
        record(&r, KEY_EXHAUSTED, None, NOW + 61).unwrap(),
        Recorded::Created
    );
    let b = ledger(&r).platform_block.unwrap();
    assert_eq!((b.reason.as_str(), b.at), (KEY_EXHAUSTED, NOW + 61));
}

#[test]
fn active_ignores_an_expired_block() {
    let l = RotateLedger {
        platform_block: Some(OverflowBlock::key(KEY_EXHAUSTED, NOW, NOW + HOUR)),
        ..RotateLedger::default()
    };
    assert!(active(&l, NOW).is_some());
    assert!(active(&l, NOW + HOUR - 1).is_some());
    assert!(active(&l, NOW + HOUR).is_none());
    assert!(active(&RotateLedger::default(), NOW).is_none());
}

#[test]
fn clear_removes_the_block_and_keeps_everything_else() {
    let (_t, r) = root();
    let rs = RotateState::new(&r);
    rs.save_ledger(&RotateLedger {
        live: Some("console".into()),
        live_kind: Some(LIVE_KIND_PLATFORM.into()),
        since: Some(NOW - 100),
        ..RotateLedger::default()
    })
    .unwrap();
    record(&r, KEY_EXHAUSTED, None, NOW).unwrap();
    assert!(clear(&r).unwrap());
    let l = ledger(&r);
    assert_eq!(l.platform_block, None);
    assert_eq!(l.live.as_deref(), Some("console"));
    assert_eq!(l.live_kind.as_deref(), Some(LIVE_KIND_PLATFORM));
    assert!(!clear(&r).unwrap(), "nothing to clear");
}

#[test]
fn clear_with_no_ledger_creates_nothing() {
    let (_t, r) = root();
    assert!(!clear(&r).unwrap());
    assert!(!RotateState::new(&r).ledger.exists());
}

#[test]
fn a_record_keeps_the_rest_of_the_ledger() {
    let (_t, r) = root();
    let rs = RotateState::new(&r);
    let mut before = RotateLedger {
        live: Some("work".into()),
        account: Some("u-work".into()),
        since: Some(NOW - 100),
        ..RotateLedger::default()
    };
    before
        .overflow_blocked
        .insert("credits".into(), OverflowBlock::spend_limit(NOW - 50));
    rs.save_ledger(&before).unwrap();
    record(&r, KEY_REVOKED, None, NOW).unwrap();
    let after = ledger(&r);
    assert_eq!(after.live, before.live);
    assert_eq!(after.account, before.account);
    assert_eq!(after.overflow_blocked, before.overflow_blocked);
    assert!(after.platform_block.is_some());
}

// --- Ledger fields ------------------------------------------------------------------

#[test]
fn the_new_fields_round_trip_and_are_left_out_when_absent() {
    let (_t, r) = root();
    let rs = RotateState::new(&r);
    let l = RotateLedger {
        live: Some("console".into()),
        live_kind: Some(LIVE_KIND_PLATFORM.into()),
        platform_block: Some(OverflowBlock::key(KEY_RATE_LIMITED, NOW, NOW + HOUR)),
        ..RotateLedger::default()
    };
    rs.save_ledger(&l).unwrap();
    assert_eq!(ledger(&r), l);

    rs.save_ledger(&RotateLedger::default()).unwrap();
    let raw = fs::read_to_string(&rs.ledger).unwrap();
    assert!(!raw.contains("live_kind"), "{raw}");
    assert!(!raw.contains("platform_block"), "{raw}");
}

#[test]
fn a_ledger_from_before_this_change_still_loads() {
    let (_t, r) = root();
    let l = raw_ledger(
        &r,
        r#"{"version":1,"live":"work","account":"u-work","since":1,
            "last_cycle":{"at":5,"outcome":"stayed","overflow":"on"},
            "overflow_blocked":{"credits":{"at":10,"until":20,"reason":"spend_limit"}}}"#,
    )
    .unwrap()
    .unwrap();
    assert_eq!(l.live_kind, None);
    assert_eq!(l.platform_block, None);
}

#[test]
fn invalid_platform_fields_make_the_ledger_inconsistent() {
    let block = |reason: &str, at: i64, until: i64| {
        format!(r#""platform_block":{{"at":{at},"until":{until},"reason":"{reason}"}}"#)
    };
    for (what, body) in [
        (
            "live_kind subscription",
            r#""live":"console","live_kind":"subscription""#.to_string(),
        ),
        (
            "live_kind other",
            r#""live":"console","live_kind":"api-key""#.to_string(),
        ),
        (
            "live_kind without live",
            r#""live_kind":"platform""#.to_string(),
        ),
        (
            "live_kind wrong type",
            r#""live":"console","live_kind":1"#.to_string(),
        ),
        ("spend reason", block("spend_limit", NOW, NOW + HOUR)),
        ("unknown reason", block("other", NOW, NOW + HOUR)),
        ("at zero", block("key_exhausted", 0, HOUR)),
        ("until before at", block("key_exhausted", NOW, NOW - 1)),
        ("until equals at", block("key_exhausted", NOW, NOW)),
        (
            "too long",
            block("key_exhausted", NOW, NOW + MAX_BLOCK_SECS + 1),
        ),
        (
            "key reason in overflow_blocked",
            format!(
                r#""overflow_blocked":{{"credits":{{"at":{NOW},"until":{},"reason":"key_exhausted"}}}}"#,
                NOW + HOUR
            ),
        ),
    ] {
        let (_t, r) = root();
        let res = raw_ledger(&r, &format!(r#"{{"version":1,{body}}}"#));
        let err = res.expect_err(what);
        assert!(err.contains("rotate ledger"), "{what}: {err}");
    }
}

#[test]
fn the_longest_allowed_block_loads() {
    let (_t, r) = root();
    let body = format!(
        r#"{{"version":1,"platform_block":{{"at":{NOW},"until":{},"reason":"key_revoked"}}}}"#,
        NOW + MAX_BLOCK_SECS
    );
    assert!(
        raw_ledger(&r, &body)
            .unwrap()
            .unwrap()
            .platform_block
            .is_some()
    );
}

#[test]
fn older_readers_keep_the_platform_fields() {
    // An older rotator rejects them and stops: the fail-safe stop is
    // preferred to an older rotator that thinks a subscription login is
    // live while ~/.claude bills the API key.
    let l = RotateLedger {
        live: Some("console".into()),
        live_kind: Some(LIVE_KIND_PLATFORM.into()),
        platform_block: Some(OverflowBlock::key(KEY_EXHAUSTED, NOW, NOW + HOUR)),
        ..RotateLedger::default()
    };
    let out = for_older_readers(&l, true);
    assert_eq!(out.live_kind, l.live_kind);
    assert_eq!(out.platform_block, l.platform_block);
}

#[test]
fn an_error_from_a_bad_ledger_never_quotes_it() {
    let (_t, r) = root();
    let err = raw_ledger(
        &r,
        r#"{"version":1,"live":"console","live_kind":"CANARY-LEAK-PROBE"}"#,
    )
    .unwrap_err();
    assert!(!err.contains("CANARY"), "{err}");
}
