//! Issue #1710 (D6, D9, D10): stage and progress, the PR join by title, and
//! the stop reason.
//!
//! Contract (`stage`):
//! - `classify(&StageInput) -> Stage`, first match wins: merged > closed >
//!   merge-ready rounds > review rounds > PR open > PR state unknown > run >
//!   manifest only. `StageInput { pr: Option<PrState>, merge_ready_rounds,
//!   crusty_rounds, has_run, has_pr }`; `pr` is `None` when there is no PR or GitHub could not be
//!   asked. `Stage { label, percent, closed }`.
//! - `review_percent(c) = min(60, 40 + 5(c-1))`,
//!   `merge_ready_percent(m) = min(95, 70 + 5(m-1))`, both saturating.
//! - `pr_for_issue(prs, issue)`: the first PR whose title contains
//!   `#<issue>` followed by a non-digit or the end.
//! - `stop_reason(class, merged) -> Option<String>`.

use super::sources::PrInfo;
use super::stage::{
    PrState, Stage, StageInput, classify, merge_ready_percent, pr_for_issue, review_percent,
    stop_reason,
};

fn input(pr: Option<PrState>, m: u32, c: u32, has_run: bool) -> StageInput {
    StageInput {
        pr,
        merge_ready_rounds: m,
        crusty_rounds: c,
        has_run,
        has_pr: pr.is_some(),
    }
}

#[test]
fn a_known_pr_without_a_state_is_never_building() {
    let mut i = input(None, 0, 0, true);
    i.has_pr = true;
    assert_eq!(classify(&i), st("PR state unknown", 30, false));
    // Rounds still decide the stage when they are known.
    i.crusty_rounds = 2;
    assert_eq!(classify(&i).label, "review round 2");
}

fn st(label: &str, percent: u8, closed: bool) -> Stage {
    Stage {
        label: label.to_string(),
        percent,
        closed,
    }
}

#[test]
fn review_progress_runs_from_40_to_60() {
    assert_eq!(review_percent(1), 40);
    assert_eq!(review_percent(2), 45);
    assert_eq!(review_percent(5), 60);
    assert_eq!(review_percent(6), 60);
    assert_eq!(review_percent(u32::MAX), 60);
}

#[test]
fn merge_ready_progress_runs_from_70_to_95() {
    assert_eq!(merge_ready_percent(1), 70);
    assert_eq!(merge_ready_percent(3), 80);
    assert_eq!(merge_ready_percent(6), 95);
    assert_eq!(merge_ready_percent(40), 95);
    assert_eq!(merge_ready_percent(u32::MAX), 95);
}

#[test]
fn each_rule_on_its_own() {
    assert_eq!(
        classify(&input(Some(PrState::Merged), 0, 0, true)),
        st("merged", 100, false)
    );
    assert_eq!(
        classify(&input(Some(PrState::Closed), 0, 0, true)),
        st("closed", 0, true)
    );
    assert_eq!(
        classify(&input(Some(PrState::Open), 2, 0, true)),
        st("merge-ready round 2", 75, false)
    );
    assert_eq!(
        classify(&input(Some(PrState::Open), 0, 3, true)),
        st("review round 3", 50, false)
    );
    assert_eq!(
        classify(&input(Some(PrState::Open), 0, 0, true)),
        st("PR open", 30, false)
    );
    assert_eq!(
        classify(&input(None, 0, 0, true)),
        st("building", 15, false)
    );
    assert_eq!(classify(&input(None, 0, 0, false)), st("queued", 0, false));
}

#[test]
fn merged_beats_every_round_count() {
    assert_eq!(
        classify(&input(Some(PrState::Merged), 4, 6, true)).label,
        "merged"
    );
}

#[test]
fn closed_beats_round_counts() {
    let s = classify(&input(Some(PrState::Closed), 3, 2, true));
    assert_eq!(s, st("closed", 0, true));
}

#[test]
fn merge_ready_rounds_beat_review_rounds() {
    assert_eq!(
        classify(&input(Some(PrState::Open), 1, 5, true)).label,
        "merge-ready round 1"
    );
}

#[test]
fn rounds_still_count_when_github_could_not_be_asked() {
    assert_eq!(
        classify(&input(None, 0, 2, true)),
        st("review round 2", 45, false)
    );
    assert_eq!(
        classify(&input(None, 1, 0, true)),
        st("merge-ready round 1", 70, false)
    );
}

#[test]
fn a_manifest_row_with_an_open_pr_is_pr_open_not_queued() {
    assert_eq!(
        classify(&input(Some(PrState::Open), 0, 0, false)).label,
        "PR open"
    );
}

// ---------------------------------------------------------------------------
// PR join (D6)
// ---------------------------------------------------------------------------

fn pr(number: u64, title: &str) -> PrInfo {
    PrInfo {
        number,
        title: title.to_string(),
        state: "OPEN".to_string(),
        mergeable: "MERGEABLE".to_string(),
    }
}

#[test]
fn the_issue_must_be_followed_by_a_non_digit_or_the_end() {
    let prs = [
        pr(1, "feat(#17100): unrelated"),
        pr(2, "chore: bump #1710a"),
        pr(3, "feat(#1710): the right one"),
    ];
    assert_eq!(pr_for_issue(&prs, 1710).map(|p| p.number), Some(2));
    assert_eq!(
        pr_for_issue(&[pr(9, "fix #1710")], 1710).map(|p| p.number),
        Some(9)
    );
    assert_eq!(pr_for_issue(&[pr(1, "feat(#17100): x")], 1710), None);
    assert_eq!(
        pr_for_issue(&[pr(1, "issue 1710 without hash")], 1710),
        None
    );
    assert_eq!(pr_for_issue(&[], 1710), None);
}

#[test]
fn the_first_matching_pr_in_list_order_wins() {
    let prs = [pr(20, "a (#5)"), pr(10, "b (#5)")];
    assert_eq!(pr_for_issue(&prs, 5).map(|p| p.number), Some(20));
}

#[test]
fn pr_state_parses_github_values() {
    assert_eq!(PrState::from_github("OPEN"), Some(PrState::Open));
    assert_eq!(PrState::from_github("MERGED"), Some(PrState::Merged));
    assert_eq!(PrState::from_github("CLOSED"), Some(PrState::Closed));
    assert_eq!(PrState::from_github("open; rm -rf /"), None);
}

// ---------------------------------------------------------------------------
// Stop reason (D10)
// ---------------------------------------------------------------------------

#[test]
fn stop_reasons_follow_the_outcome_class() {
    assert_eq!(
        stop_reason(Some("usage_limit"), false).as_deref(),
        Some("stopped on a usage limit")
    );
    assert_eq!(stop_reason(Some("ok"), false).as_deref(), Some("finished"));
    assert_eq!(
        stop_reason(Some("work"), false).as_deref(),
        Some("stopped: work")
    );
    assert_eq!(
        stop_reason(Some("error"), false).as_deref(),
        Some("stopped: error")
    );
    assert_eq!(stop_reason(None, false).as_deref(), Some("stopped"));
}

#[test]
fn a_merged_row_shows_no_stop_reason() {
    assert_eq!(stop_reason(Some("ok"), true), None);
    assert_eq!(stop_reason(Some("usage_limit"), true), None);
    assert_eq!(stop_reason(None, true), None);
}
