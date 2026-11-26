//! Issue #2910 (D13, AC4, S5): the kinds of recorded block a ledger may hold.
//! Wired from `spend_blocks.rs`.
//!
//! A block records that a login ran into a limit and when it resets. Until
//! now the ledger took one kind, `spend_limit`. A session limit and a weekly
//! limit are recorded the same way, so the rotator keeps such a login out of
//! its choices until the recorded reset, whatever the kind.
//!
//! The rule is the one on the #2909 branch (`valid_kind`, the `SESSION_LIMIT`,
//! `WEEK_LIMIT` and `CREDIT_LIMIT` constants), so the two branches agree on it
//! line for line. It is a byte-level allow-list: `^[a-z0-9_]{1,32}$` with no
//! regex, so no case folding, no Unicode and no separator can carry a path, a
//! newline or a NUL into a log line. `kind_label` always returns fixed text
//! (`&'static str`), so a byte a writer chose never reaches the rotate log.
//!
//! This change writes no new kind. It only reads them.

use super::*;
use crate::commands::profile::rotate::ledger::{OverflowBlock, SPEND_LIMIT};
use crate::commands::profile::rotate::tests_swap::{env, private};

const NOW: i64 = 1_795_000_000;

// --- valid_kind -------------------------------------------------------------------

#[test]
fn the_kinds_this_binary_knows_are_valid() {
    for k in [SPEND_LIMIT, SESSION_LIMIT, WEEK_LIMIT, CREDIT_LIMIT] {
        assert!(valid_kind(k), "{k}");
    }
}

#[test]
fn the_constants_have_the_documented_values() {
    assert_eq!(SPEND_LIMIT, "spend_limit");
    assert_eq!(SESSION_LIMIT, "session_limit");
    assert_eq!(WEEK_LIMIT, "week_limit");
    assert_eq!(CREDIT_LIMIT, "credit_limit");
}

#[test]
fn a_kind_this_binary_does_not_know_is_still_valid_when_it_is_a_short_token() {
    // A newer writer may record a kind this binary has never seen; the login
    // must still stay out of the choices rather than make the ledger unreadable.
    for k in ["a", "x_1", "weekly_pool_7", "0", "_", "model_limit"] {
        assert!(valid_kind(k), "{k}");
    }
}

#[test]
fn a_kind_may_be_32_bytes_and_no_longer() {
    assert!(valid_kind(&"a".repeat(32)));
    assert!(!valid_kind(&"a".repeat(33)));
    assert!(!valid_kind(&"a".repeat(4096)));
}

/// S5: inputs that a regex with anchors or a case-insensitive match would
/// let through are refused byte by byte.
#[test]
fn a_kind_outside_the_byte_allow_list_is_refused() {
    for k in [
        "",
        "Session_Limit",
        "SESSION_LIMIT",
        "session-limit",
        "session limit",
        "session_limit\n",
        "\nsession_limit",
        "session_limit\r\n",
        "session_limit\0",
        "session\0limit",
        "../session_limit",
        "a/b",
        "a.b",
        "é",
        "session_limité",
        "日本語",
        "a\u{0301}",
        "\u{ff53}ession_limit",
        " session_limit",
        "session_limit ",
        "\t",
    ] {
        assert!(!valid_kind(k), "{k:?} must be refused");
    }
}

// --- kind_label ---------------------------------------------------------------------

#[test]
fn a_known_kind_gets_its_own_fixed_label() {
    assert_eq!(kind_label(SPEND_LIMIT), "spend limit");
    assert_eq!(kind_label(SESSION_LIMIT), "session limit");
    assert_eq!(kind_label(WEEK_LIMIT), "week limit");
    assert_eq!(kind_label(CREDIT_LIMIT), "credit limit");
}

#[test]
fn an_unknown_valid_kind_gets_one_fixed_fallback_and_is_never_echoed() {
    let a = kind_label("weekly_pool_7");
    let b = kind_label("model_limit");
    assert_eq!(
        a, b,
        "one fallback for every kind this binary does not know"
    );
    assert!(!a.contains("weekly_pool_7") && !a.contains("pool"), "{a}");
    // The return type is `&'static str`: a byte a writer chose cannot reach it.
    let _: &'static str = kind_label("anything");
}

#[test]
fn kind_label_never_echoes_what_it_was_given() {
    for k in [
        "SECRET-REASON",
        "session_limit\nforged log line",
        "\u{1b}[31mred",
        "../etc/passwd",
        "",
    ] {
        let label = kind_label(k);
        assert!(
            !label.contains("SECRET") && !label.contains("forged"),
            "{label}"
        );
        assert!(
            !label.contains('\n') && !label.contains('\u{1b}') && !label.contains('/'),
            "{label:?}"
        );
    }
}

// --- The ledger (AC4) --------------------------------------------------------------------

fn block(reason: &str) -> OverflowBlock {
    OverflowBlock {
        at: NOW - 10,
        until: NOW + 3600,
        reason: reason.to_string(),
    }
}

#[test]
fn a_ledger_holding_each_kind_loads() {
    let e = env();
    let mut l = e.rs.load_ledger().unwrap().unwrap();
    for (who, kind) in [
        ("a", SPEND_LIMIT),
        ("b", SESSION_LIMIT),
        ("c", WEEK_LIMIT),
        ("d", CREDIT_LIMIT),
        ("e", "weekly_pool_7"),
    ] {
        l.overflow_blocked.insert(who.into(), block(kind));
    }
    e.rs.save_ledger(&l).unwrap();
    let back = e.rs.load_ledger().unwrap().unwrap();
    assert_eq!(back.overflow_blocked.len(), 5);
    assert_eq!(back.overflow_blocked["b"].reason, SESSION_LIMIT);
    assert_eq!(back.overflow_blocked["c"].reason, WEEK_LIMIT);
}

#[test]
fn a_weekly_block_of_62_days_still_fits() {
    // `MAX_BLOCK_SECS` is 62 days, so a block that lasts a week is well inside.
    let e = env();
    let mut l = e.rs.load_ledger().unwrap().unwrap();
    l.overflow_blocked.insert(
        "a".into(),
        OverflowBlock {
            at: NOW - 10,
            until: NOW + 7 * 86_400,
            reason: WEEK_LIMIT.into(),
        },
    );
    e.rs.save_ledger(&l).unwrap();
    assert!(e.rs.load_ledger().unwrap().is_some());
}

#[test]
fn a_ledger_with_a_kind_outside_the_allow_list_is_refused_without_echoing_it() {
    let long = "a".repeat(33);
    for bad in [
        "SECRET-REASON",
        "Session_Limit",
        "",
        "session_limit\n",
        long.as_str(),
    ] {
        let e = env();
        let body = serde_json::json!({
            "version": 1,
            "overflow_blocked": { "a": { "at": NOW - 10, "until": NOW + 3600, "reason": bad } }
        })
        .to_string();
        private(&e.rs.ledger, &body);
        let err =
            e.rs.load_ledger()
                .expect_err(&format!("{bad:?} must be refused"))
                .to_string();
        assert!(!err.contains("SECRET"), "{err}");
    }
}

#[test]
fn the_time_rules_of_a_block_do_not_depend_on_its_kind() {
    for kind in [SESSION_LIMIT, WEEK_LIMIT, CREDIT_LIMIT] {
        let e = env();
        for (at, until) in [(0, 100), (NOW, NOW), (NOW, NOW - 1), (NOW, i64::MAX)] {
            let body = serde_json::json!({
                "version": 1,
                "overflow_blocked": { "a": { "at": at, "until": until, "reason": kind } }
            })
            .to_string();
            private(&e.rs.ledger, &body);
            assert!(e.rs.load_ledger().is_err(), "{kind} {at}..{until}");
        }
    }
}

// --- When a block is in force --------------------------------------------------------------

#[test]
fn a_block_of_every_kind_is_in_force_until_its_reset_and_not_at_it() {
    let e = env();
    let mut l = e.rs.load_ledger().unwrap().unwrap();
    for (who, kind) in [("a", SPEND_LIMIT), ("b", SESSION_LIMIT), ("c", WEEK_LIMIT)] {
        l.overflow_blocked.insert(who.into(), block(kind));
    }
    let until = NOW + 3600;
    assert_eq!(active_blocks(&l, until - 1).len(), 3);
    assert!(
        active_blocks(&l, until).is_empty(),
        "now >= until ends the block"
    );
    assert!(active_blocks(&l, until + 1).is_empty());
}
