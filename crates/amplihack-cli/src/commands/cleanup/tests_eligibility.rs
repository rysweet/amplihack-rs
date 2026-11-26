//! Unit tests for `eligibility.rs` (issue #1808, C3, D6).
//!
//! Wire with, at the end of `eligibility.rs`:
//! `#[cfg(test)] #[path = "tests_eligibility.rs"] mod tests;`
//!
//! Contract (pure functions; the `gh` call and `/proc` reads live elsewhere):
//! - `pub(crate) enum Verdict { Active, Eligible { reason: String }, Skip { reason: String } }`
//! - `pub(crate) struct PrInfo { pub number: u64, pub state: String,
//!   pub closed_at: Option<DateTime<Utc>>, pub merged_at: Option<DateTime<Utc>> }`
//! - `pub(crate) fn pr_verdict(pr: &PrInfo, now: DateTime<Utc>, keep: Duration) -> Verdict`;
//! - `pub(crate) fn no_pr_verdict(ended_at: Option<DateTime<Utc>>, run_json_mtime: SystemTime,
//!   now: SystemTime, keep: Duration) -> Verdict`;
//! - `pub(crate) fn is_valid_id(s: &str) -> bool` (`^[0-9a-f-]{1,64}$`);
//! - `pub(crate) fn is_valid_branch(s: &str) -> bool` (git check-ref-format
//!   rules; never starts with `-`).

use super::*;
use chrono::{DateTime, Utc};
use std::time::{Duration, SystemTime};

const DAY: Duration = Duration::from_secs(24 * 3600);

fn hours_ago(h: i64) -> DateTime<Utc> {
    Utc::now() - chrono::Duration::hours(h)
}

fn skip_reason(v: &Verdict) -> &str {
    match v {
        Verdict::Skip { reason } => reason,
        other => panic!("expected Skip, got {other:?}"),
    }
}

#[test]
fn closed_pr_within_keep_is_skipped_and_after_keep_is_eligible() {
    let now = Utc::now();
    let young = PrInfo {
        number: 1,
        state: "MERGED".into(),
        closed_at: Some(hours_ago(1)),
        merged_at: Some(hours_ago(1)),
    };
    assert!(skip_reason(&pr_verdict(&young, now, DAY)).contains("within keep period"));
    let old = PrInfo {
        number: 2,
        state: "CLOSED".into(),
        closed_at: Some(hours_ago(25)),
        merged_at: None,
    };
    assert!(matches!(
        pr_verdict(&old, now, DAY),
        Verdict::Eligible { .. }
    ));
    assert!(matches!(
        pr_verdict(&young, now, Duration::ZERO),
        Verdict::Eligible { .. }
    ));
}

#[test]
fn open_pr_is_skipped_with_its_number() {
    let open = PrInfo {
        number: 1831,
        state: "OPEN".into(),
        closed_at: None,
        merged_at: None,
    };
    let reason = skip_reason(&pr_verdict(&open, Utc::now(), Duration::ZERO)).to_string();
    assert!(
        reason.contains("#1831") && reason.contains("open"),
        "{reason}"
    );
}

#[test]
fn closed_at_in_the_future_counts_as_within_keep() {
    let pr = PrInfo {
        number: 3,
        state: "MERGED".into(),
        closed_at: Some(hours_ago(-5)),
        merged_at: None,
    };
    assert!(skip_reason(&pr_verdict(&pr, Utc::now(), DAY)).contains("within keep period"));
}

#[test]
fn pr_and_no_pr_rules_are_eligible_at_the_keep_boundary() {
    let now = Utc::now();
    let boundary = now - chrono::Duration::days(1);
    let pr = PrInfo {
        number: 3,
        state: "MERGED".into(),
        closed_at: Some(boundary),
        merged_at: Some(now),
    };
    assert!(matches!(
        pr_verdict(&pr, now, DAY),
        Verdict::Eligible { .. }
    ));
    assert!(matches!(
        no_pr_verdict(None, boundary.into(), now.into(), DAY),
        Verdict::Eligible { .. }
    ));
}

#[test]
fn no_pr_rule_uses_later_of_ended_at_and_mtime() {
    let now = SystemTime::now();
    let old_mtime = now - 3 * DAY;
    let recent_mtime = now - Duration::from_secs(3600);
    assert!(matches!(
        no_pr_verdict(Some(hours_ago(72)), old_mtime, now, DAY),
        Verdict::Eligible { .. }
    ));
    assert!(
        skip_reason(&no_pr_verdict(Some(hours_ago(72)), recent_mtime, now, DAY))
            .contains("within keep period")
    );
    assert!(
        skip_reason(&no_pr_verdict(Some(hours_ago(1)), old_mtime, now, DAY))
            .contains("within keep period")
    );
    assert!(matches!(
        no_pr_verdict(None, old_mtime, now, DAY),
        Verdict::Eligible { .. }
    ));
    let future = now + DAY;
    assert!(matches!(
        no_pr_verdict(None, future, now, DAY),
        Verdict::Skip { .. }
    ));
}

#[test]
fn ids_and_branches_are_validated() {
    assert!(is_valid_id("3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3"));
    for bad in ["", "ABC", "../x", "a b", &"a".repeat(65)] {
        assert!(!is_valid_id(bad), "{bad:?}");
    }
    assert!(is_valid_branch("feat/issue-1808"));
    for bad in [
        "",
        "-x",
        "--upload-pack=evil",
        "a..b",
        "a b",
        "a~1",
        "a\nb",
        "/a",
        "a/",
        "a.lock",
    ] {
        assert!(!is_valid_branch(bad), "{bad:?}");
    }
}
