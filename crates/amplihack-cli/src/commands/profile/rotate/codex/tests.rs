//! Unit tests for the Codex rotation (issue #2005). Wired from
//! `rotate/codex/mod.rs` with:
//!
//! ```ignore
//! #[cfg(test)]
//! mod tests;
//! #[cfg(test)]
//! mod tests_link;
//! #[cfg(test)]
//! mod tests_state;
//! #[cfg(test)]
//! mod tests_inuse;
//! #[cfg(test)]
//! mod tests_session;
//! #[cfg(test)]
//! mod tests_settings;
//! ```
//!
//! Contract these tests define (all `pub(super)` or wider):
//!
//! - `decide::{decide, DecideInput, Decision, NoneReason}`:
//!   `DecideInput { rows: &[AccountResult], live: Option<&str>, floor: f64,
//!   journal_open: bool, last_swap_at: Option<i64>, now: i64, hold: u64,
//!   margin: u8 }`; `Decision::{Keep, Swap { from, to }, None(NoneReason)}`;
//!   `NoneReason::{NoRoom, NoLive, Recovering, Held, Damped}`;
//!   `Decision::text()` is the cycle line's `-> ...` part.
//! - `swap::{RefuseCode, DOCUMENTED}`: `RefuseCode::ALL: [RefuseCode; 8]`,
//!   `RefuseCode::as_str()`, `DOCUMENTED: [&str; 8]`.
//! - `limit::{limit_hit_reading, limit_hit_event}`.
//! - `clock::{Clock, TestClock}`: `Clock { fn now(&self) -> i64; fn
//!   sleep_until(&self, deadline: i64); }`; `TestClock::new(now)`,
//!   `TestClock::sleeps() -> Vec<i64>`.
//! - `Hooks::real()` with a `fault` field that is `None` in release builds.
//! - `amplihack_utils::limit_resume_cap::MAX_RESUMES_PER_SESSION`.

use super::clock::{Clock, TestClock};
use super::decide::{DecideInput, Decision, NoneReason, decide};
use super::limit::{limit_hit_event, limit_hit_reading};
use super::swap::{DOCUMENTED, RefuseCode};
use amplihack_utils::codex_capacity::{
    AccountResult, Credits, DEFAULT_FLOOR, Outcome, Reading, Window, WindowName,
};
use std::path::PathBuf;

const NOW: i64 = 1_794_200_000;

fn reading(used: &[f64]) -> Reading {
    let mut r = Reading::default();
    r.windows = used
        .iter()
        .enumerate()
        .map(|(i, p)| Window {
            name: if i == 0 {
                WindowName::Primary
            } else {
                WindowName::Secondary
            },
            used_percent: Some(*p),
            window_minutes: Some(if i == 0 { 300 } else { 10080 }),
            resets_at: Some(NOW + 3600),
        })
        .collect();
    r
}

fn ok(label: &str, used: &[f64]) -> AccountResult {
    AccountResult::new(
        label,
        PathBuf::from(format!("/r/{label}")),
        Outcome::Ok(reading(used)),
        NOW,
    )
}

fn unread(label: &str) -> AccountResult {
    AccountResult::new(
        label,
        PathBuf::from(format!("/r/{label}")),
        Outcome::NotSignedIn,
        NOW,
    )
}

fn input<'a>(rows: &'a [AccountResult], live: Option<&'a str>) -> DecideInput<'a> {
    DecideInput {
        rows,
        live,
        floor: DEFAULT_FLOOR,
        journal_open: false,
        last_swap_at: None,
        now: NOW,
        hold: 3600,
        margin: 5,
    }
}

fn swap(from: &str, to: &str) -> Decision {
    Decision::Swap {
        from: from.into(),
        to: to.into(),
    }
}

// ---- decide: the documented order ----

#[test]
fn an_open_journal_is_recovering_before_every_other_rule() {
    let full = [ok("a", &[100.0]), ok("b", &[100.0, 20.0]), unread("c")];
    let open = [ok("a", &[10.0]), ok("b", &[50.0])];
    for (rows, live) in [
        (&full[..], Some("a")),
        (&full[..], None),
        (&open[..], None),
        (&open[..], Some("a")),
    ] {
        let mut i = input(rows, live);
        i.journal_open = true;
        assert_eq!(decide(&i), Decision::None(NoneReason::Recovering));
    }
}

#[test]
fn no_room_comes_before_no_live() {
    let rows = [ok("a", &[100.0]), ok("b", &[100.0, 20.0]), unread("c")];
    assert_eq!(
        decide(&input(&rows, None)),
        Decision::None(NoneReason::NoRoom)
    );
    let rows = [ok("a", &[10.0]), ok("b", &[50.0])];
    assert_eq!(
        decide(&input(&rows, None)),
        Decision::None(NoneReason::NoLive)
    );
}

#[test]
fn the_best_live_account_is_kept_even_while_held() {
    let rows = [ok("a", &[10.0]), ok("b", &[50.0])];
    let mut i = input(&rows, Some("a"));
    i.last_swap_at = Some(NOW);
    assert_eq!(decide(&i), Decision::Keep);
}

#[test]
fn a_recent_swap_holds_while_the_live_account_has_room() {
    let rows = [ok("a", &[50.0]), ok("b", &[10.0])];
    let mut i = input(&rows, Some("a"));
    i.last_swap_at = Some(NOW - 3599);
    assert_eq!(decide(&i), Decision::None(NoneReason::Held));
    i.last_swap_at = Some(NOW - 3600);
    assert_eq!(decide(&i), swap("a", "b"), "the hold is strictly less than");
    i.last_swap_at = Some(NOW);
    i.hold = 0;
    assert_eq!(decide(&i), swap("a", "b"), "hold 0 disables it");
}

#[test]
fn a_small_gain_is_damped_by_the_margin() {
    // a has room 50, b 54: a gain of 4 under margin 5.
    let rows = [ok("a", &[50.0]), ok("b", &[46.0])];
    let i = input(&rows, Some("a"));
    assert_eq!(decide(&i), Decision::None(NoneReason::Damped));
    let rows = [ok("a", &[50.0]), ok("b", &[45.0])];
    assert_eq!(
        decide(&input(&rows, Some("a"))),
        swap("a", "b"),
        "exactly the margin swaps"
    );
}

#[test]
fn held_comes_before_damped() {
    let rows = [ok("a", &[50.0]), ok("b", &[48.0])];
    let mut i = input(&rows, Some("a"));
    i.last_swap_at = Some(NOW - 10);
    assert_eq!(decide(&i), Decision::None(NoneReason::Held));
}

#[test]
fn a_full_or_unread_live_account_is_never_held_or_damped() {
    for live_row in [ok("a", &[100.0]), unread("a")] {
        let rows = [live_row, ok("b", &[99.0])];
        let mut i = input(&rows, Some("a"));
        i.last_swap_at = Some(NOW);
        i.margin = 100;
        assert_eq!(decide(&i), swap("a", "b"));
    }
}

#[test]
fn a_live_account_missing_from_the_rows_counts_as_having_no_room() {
    let rows = [ok("b", &[60.0])];
    let mut i = input(&rows, Some("a"));
    i.last_swap_at = Some(NOW);
    assert_eq!(decide(&i), swap("a", "b"));
}

#[test]
fn credits_above_the_floor_give_room_half() {
    let mut r = reading(&[100.0]);
    r.credits = Some(Credits {
        has_credits: true,
        balance: Some(4200.0),
    });
    let rows = [
        ok("a", &[100.0]),
        AccountResult::new("h", PathBuf::from("/r/h"), Outcome::Ok(r), NOW),
    ];
    assert_eq!(decide(&input(&rows, Some("a"))), swap("a", "h"));
    let mut i = input(&rows, Some("a"));
    i.floor = 5000.0;
    assert_eq!(decide(&i), Decision::None(NoneReason::NoRoom));
}

#[test]
fn decision_text_is_the_cycle_line_tail() {
    assert_eq!(Decision::Keep.text(), "keep");
    assert_eq!(swap("a", "b").text(), "swap a->b");
    for (r, t) in [
        (NoneReason::NoRoom, "none(no-room)"),
        (NoneReason::NoLive, "none(no-live)"),
        (NoneReason::Recovering, "none(recovering)"),
        (NoneReason::Held, "none(held)"),
        (NoneReason::Damped, "none(damped)"),
    ] {
        assert_eq!(Decision::None(r).text(), t);
    }
}

// ---- refusal codes ----

#[test]
fn refusal_codes_are_the_documented_eight() {
    // `proc-unreadable` is its own code (not a detail of
    // `default-home-in-use`): the reference's refusal table lists it.
    let want = [
        "home-not-link",
        "link-outside-root",
        "link-dangling",
        "journal-mismatch",
        "default-home-in-use",
        "lock-busy",
        "codex-bin-in-home",
        "proc-unreadable",
    ];
    assert_eq!(DOCUMENTED, want);
    let all: Vec<&str> = RefuseCode::ALL.iter().map(|c| c.as_str()).collect();
    assert_eq!(all, want);
}

#[test]
fn the_reference_documents_every_refusal_code_in_its_table() {
    let doc = include_str!("../../../../../../../docs/reference/profile-command.md");
    for code in DOCUMENTED {
        assert!(doc.contains(&format!("| `{code}` |")), "{code}");
    }
}

// ---- limit detection (fixtures under testdata/) ----

fn events(raw: &str) -> Vec<serde_json::Value> {
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn fixture_events_with_a_limit_are_detected() {
    for (name, raw) in [
        (
            "limit_text.jsonl",
            include_str!("testdata/limit_text.jsonl"),
        ),
        (
            "limit_curly.jsonl",
            include_str!("testdata/limit_curly.jsonl"),
        ),
        (
            "limit_code.jsonl",
            include_str!("testdata/limit_code.jsonl"),
        ),
        (
            "limit_reached_type.jsonl",
            include_str!("testdata/limit_reached_type.jsonl"),
        ),
        (
            "limit_camel.jsonl",
            include_str!("testdata/limit_camel.jsonl"),
        ),
    ] {
        assert!(events(raw).iter().any(limit_hit_event), "{name}");
    }
}

#[test]
fn fixture_events_without_a_limit_are_not() {
    let raw = include_str!("testdata/limit_none.jsonl");
    for e in events(raw) {
        assert!(!limit_hit_event(&e), "{e}");
    }
}

#[test]
fn the_limit_text_ignores_ascii_case() {
    let e =
        serde_json::json!({"type": "error", "message": "YOU'VE HIT YOUR USAGE LIMIT. try later"});
    assert!(limit_hit_event(&e));
    let e = serde_json::json!({"type": "error", "message": "you have hit your usage limit"});
    assert!(!limit_hit_event(&e), "only the documented wording");
}

#[test]
fn a_null_reached_type_is_not_a_limit() {
    let e = serde_json::json!({"msg": {"rate_limits": {"rate_limit_reached_type": null}}});
    assert!(!limit_hit_event(&e));
    let e = serde_json::json!({"msg": {"rate_limits": {"rate_limit_reached_type": "secondary"}}});
    assert!(limit_hit_event(&e));
}

#[test]
fn a_reading_is_limited_only_with_a_reached_type() {
    let mut r = reading(&[100.0]);
    assert!(!limit_hit_reading(&r), "full but not reported reached");
    r.rate_limit_reached_type = Some("primary".into());
    assert!(limit_hit_reading(&r));
}

// ---- clock ----

#[test]
fn the_test_clock_records_one_sleep_and_moves_time() {
    let c = TestClock::new(NOW);
    c.sleep_until(NOW + 600);
    assert_eq!(c.sleeps(), [NOW + 600]);
    assert_eq!(c.now(), NOW + 600);
    c.sleep_until(NOW - 5);
    assert_eq!(c.now(), NOW + 600, "a past deadline never moves time back");
}

// ---- shared cap, fault hook ----

#[test]
fn the_resume_cap_is_the_shared_three() {
    assert_eq!(
        amplihack_utils::limit_resume_cap::MAX_RESUMES_PER_SESSION,
        3
    );
}

#[cfg(not(debug_assertions))]
#[test]
fn release_builds_have_no_fault_hook() {
    assert!(super::Hooks::real().fault.is_none());
}
