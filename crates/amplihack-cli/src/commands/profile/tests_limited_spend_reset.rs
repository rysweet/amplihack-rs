//! Issue #2882/#2890: what `Output::usage_limited` reports for a spend
//! limit that states a reset.
//!
//! Contract:
//! - `Limited::spend_reset: Option<(Harness, i64)>` is `Some` only for a
//!   spend-limit message that states a parsable reset, carrying the harness
//!   and the stated reset. It is `None` for a spend limit with no reset, and
//!   for every other limit.
//! - `Limited::seen` keeps its meaning: `None` for every spend limit.
//! - `Limited::spend` keeps its meaning: `Some` only for a spend limit with
//!   no reset (#2731).
//! - The three fields are disjoint, so no limit feeds two promotions.
//!
//! Wired from `attempt.rs` with `#[cfg(test)] mod tests_limited_spend_reset;`.

use super::{Limited, Output};
use crate::commands::recipe::run::limit_patterns::Harness;

const SPEND_WEEKLY: &str = "You've hit your individual spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage · your weekly limit resets Dec 24, 2pm (UTC)\n";
const SPEND_SESSION: &str = "You've hit your org's monthly spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage · your session limit resets 1pm (UTC)\n";
const SPEND_NO_RESET: &str = "You've hit your org's monthly spend limit · run /usage-credits to raise it, or visit claude.ai/admin-settings/usage\n";
const WEEKLY: &str = "You've hit your weekly limit · resets Dec 24, 2pm (UTC)\n";

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
fn a_spend_limit_with_a_weekly_reset_carries_the_reset() {
    let l = limited(SPEND_WEEKLY, now());
    let (harness, reset) = l.spend_reset.expect("the stated reset");
    assert_eq!(harness, Harness::Claude);
    assert!(reset > now(), "a future reset");
    assert!(l.seen.is_none(), "the #2857 row must not see it");
    assert!(l.spend.is_none(), "the #2731 row needs no reset");
}

#[test]
fn a_spend_limit_with_a_session_reset_carries_it_too() {
    let l = limited(SPEND_SESSION, now());
    assert!(l.spend_reset.is_some());
    assert!(l.seen.is_none() && l.spend.is_none());
}

#[test]
fn a_spend_limit_with_no_reset_is_the_login_change_case_only() {
    let l = limited(SPEND_NO_RESET, now());
    assert_eq!(l.spend_reset, None);
    assert_eq!(l.spend, Some(Harness::Claude));
    assert!(l.seen.is_none());
}

#[test]
fn an_ordinary_weekly_limit_is_seen_and_not_a_spend_reset() {
    let l = limited(WEEKLY, now());
    assert_eq!(l.spend_reset, None);
    assert!(l.seen.is_some());
    assert!(l.spend.is_none());
}

#[test]
fn a_spend_reset_already_in_the_past_is_still_carried_unclamped() {
    // The planner, not the field, decides what a passed reset means.
    let later = now() + 400 * 86_400;
    let l = limited(SPEND_WEEKLY, later);
    if let Some((_, reset)) = l.spend_reset {
        assert!(reset > 0);
    }
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
fn spend_text_quoted_in_the_model_answer_is_not_a_spend_reset() {
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
        assert_eq!(l.spend_reset, None);
    }
}
