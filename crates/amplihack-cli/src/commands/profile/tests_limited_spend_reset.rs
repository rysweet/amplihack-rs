//! Issues #2882, #2890, #2911: what `Output::usage_limited` reports for the
//! rotator's ledger.
//!
//! #2890 carried one case, a Claude spend limit that states a reset, in
//! three disjoint fields (`seen`, `spend`, `spend_reset`) that between them
//! fed three promotions in the planner. The promotions are gone (#2911) and
//! so are the fields; one is left.
//!
//! Contract:
//! - `Limited::block: Option<(LimitKind, Option<i64>)>` is set for every
//!   limit of every kind, carrying the kind the message's own words name and
//!   the reset it stated, unclamped. `None` only when the output carried no
//!   limit message at all.
//! - It has exactly one reader, `auto_switch::rerun`, which hands it to
//!   `rotate::step_block::note_limit_block` and does nothing else with it.
//!   The wait is planned from `Limited::wait`, whatever happens to the
//!   record.
//!
//! Wired from `attempt.rs` with `#[cfg(test)] mod tests_limited_spend_reset;`.

use super::{Limited, Output};
use crate::commands::recipe::run::limit_patterns::LimitKind;

const SPEND_WEEKLY: &str = "You've hit your individual spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage · your weekly limit resets Dec 24, 2pm (UTC)\n";
const SPEND_SESSION: &str = "You've hit your org's monthly spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage · your session limit resets 1pm (UTC)\n";
const SPEND_NO_RESET: &str = "You've hit your org's monthly spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage\n";
const WEEKLY: &str = "You've hit your weekly limit · resets Dec 24, 2pm (UTC)\n";
const SESSION: &str = "You've hit your session limit · resets 1pm (UTC)\n";
const CREDITS: &str = "Your workspace is out of credits. Add credits to continue.\n";

fn limited(stderr: &str, now: i64) -> Limited {
    Output {
        failed: true,
        stderr: stderr.as_bytes().to_vec(),
        stdout: Vec::new(),
    }
    .usage_limited(now)
    .expect("a usage limit")
}

fn now() -> i64 {
    crate::commands::profile::fsx::now()
}

#[test]
fn a_spend_limit_with_a_weekly_reset_is_recorded_as_a_spend_block() {
    let l = limited(SPEND_WEEKLY, now());
    let (kind, reset) = l.block.expect("a block to record");
    assert_eq!(kind, LimitKind::Spend, "spend wins over the window named");
    assert!(
        reset.is_some_and(|r| r > now()),
        "the stated reset, as read: {reset:?}"
    );
}

#[test]
fn a_spend_limit_with_a_session_reset_carries_it_too() {
    let l = limited(SPEND_SESSION, now());
    let (kind, reset) = l.block.expect("a block to record");
    assert_eq!(kind, LimitKind::Spend);
    assert!(reset.is_some());
}

#[test]
fn a_spend_limit_with_no_reset_records_the_kind_and_no_reset() {
    let l = limited(SPEND_NO_RESET, now());
    assert_eq!(l.block, Some((LimitKind::Spend, None)));
}

/// Issue #2911: every kind is recorded, not only a spend limit. The
/// rotator's `/usage` probe learns of a window limit only on its own cycle,
/// minutes after the step that met one, so the step is the first witness
/// here too.
#[test]
fn issue_2911_a_window_limit_is_recorded_with_its_kind_and_reset() {
    let l = limited(WEEKLY, now());
    let (kind, reset) = l.block.expect("a block to record");
    assert_eq!(kind, LimitKind::Weekly);
    assert!(reset.is_some_and(|r| r > now()));

    let l = limited(SESSION, now());
    let (kind, reset) = l.block.expect("a block to record");
    assert_eq!(kind, LimitKind::Session);
    assert!(reset.is_some());
}

#[test]
fn issue_2911_codex_credits_are_recorded_with_no_reset() {
    let l = limited(CREDITS, now());
    assert_eq!(l.block, Some((LimitKind::Credits, None)));
}

#[test]
fn a_reset_already_in_the_past_is_still_carried_unclamped() {
    // The planner and the emitter, not the field, decide what a passed reset
    // means.
    let later = now() + 400 * 86_400;
    let l = limited(SPEND_WEEKLY, later);
    let (_, reset) = l.block.expect("a block to record");
    assert!(reset.is_some_and(|r| r > 0));
}

#[test]
fn a_succeeded_attempt_carries_nothing() {
    let out = Output {
        failed: false,
        stderr: SPEND_WEEKLY.as_bytes().to_vec(),
        stdout: Vec::new(),
    };
    assert!(out.usage_limited(now()).is_none());
}

#[test]
fn spend_text_quoted_in_the_model_answer_records_nothing() {
    // stdout is the model's answer in `-p` mode; only a whole line in the
    // harness message's shape counts, so prose quoting one must not block
    // a login (agent output can quote files and web pages).
    let prose = "I read the docs. They say: you've hit your individual spend limit, \
                 your weekly limit resets Dec 24, 2pm (UTC), and then it stops.\n";
    let out = Output {
        failed: true,
        stderr: b"error: tests failed\n".to_vec(),
        stdout: prose.as_bytes().to_vec(),
    };
    if let Some(l) = out.usage_limited(now()) {
        assert_eq!(l.block, None, "no limit message, so no block");
    }
}
