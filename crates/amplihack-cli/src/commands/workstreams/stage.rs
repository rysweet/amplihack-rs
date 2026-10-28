//! Stage and progress (D9), the PR join by title (D6) and the stop reason
//! (D10). Pure functions.

use super::sources::PrInfo;

/// A PR's state as GitHub reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrState {
    Open,
    Merged,
    Closed,
}

impl PrState {
    /// `OPEN`, `MERGED` or `CLOSED`; anything else is unknown.
    pub(crate) fn from_github(state: &str) -> Option<Self> {
        match state {
            "OPEN" => Some(Self::Open),
            "MERGED" => Some(Self::Merged),
            "CLOSED" => Some(Self::Closed),
            _ => None,
        }
    }
}

/// What the stage is decided from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StageInput {
    /// `None` when there is no PR or GitHub could not be asked.
    pub pr: Option<PrState>,
    pub merge_ready_rounds: u32,
    pub crusty_rounds: u32,
    /// A run exists, rather than a manifest row alone.
    pub has_run: bool,
    /// A PR number is known, whether or not GitHub gave its state. The
    /// state is missing when GitHub could not be asked or the PR is older
    /// than the PRs listed.
    pub has_pr: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stage {
    pub label: String,
    pub percent: u8,
    /// The PR was closed without merging: the row is shown stopped.
    pub closed: bool,
}

fn stage(label: impl Into<String>, percent: u8, closed: bool) -> Stage {
    Stage {
        label: label.into(),
        percent,
        closed,
    }
}

/// The first rule that matches: merged, closed, merge-ready rounds, review
/// rounds, PR open, a PR whose state is unknown, a run, a manifest row
/// alone.
pub(crate) fn classify(input: &StageInput) -> Stage {
    match input.pr {
        Some(PrState::Merged) => return stage("merged", 100, false),
        Some(PrState::Closed) => return stage("closed", 0, true),
        _ => {}
    }
    if input.merge_ready_rounds > 0 {
        let m = input.merge_ready_rounds;
        return stage(
            format!("merge-ready round {m}"),
            merge_ready_percent(m),
            false,
        );
    }
    if input.crusty_rounds > 0 {
        let c = input.crusty_rounds;
        return stage(format!("review round {c}"), review_percent(c), false);
    }
    if input.pr == Some(PrState::Open) {
        return stage("PR open", 30, false);
    }
    // The PR exists but may have merged since: never report it as building.
    if input.has_pr {
        return stage("PR state unknown", 30, false);
    }
    if input.has_run {
        stage("building", 15, false)
    } else {
        stage("queued", 0, false)
    }
}

/// `min(60, 40 + 5(c - 1))`.
pub(crate) fn review_percent(rounds: u32) -> u8 {
    scaled(rounds, 40, 60)
}

/// `min(95, 70 + 5(m - 1))`.
pub(crate) fn merge_ready_percent(rounds: u32) -> u8 {
    scaled(rounds, 70, 95)
}

fn scaled(rounds: u32, base: u32, cap: u32) -> u8 {
    let percent = rounds
        .saturating_sub(1)
        .saturating_mul(5)
        .saturating_add(base)
        .min(cap);
    u8::try_from(percent).unwrap_or(u8::MAX)
}

/// The first PR whose title contains `#<issue>` followed by a non-digit or
/// the end. Titles are used only for this match.
pub(crate) fn pr_for_issue(prs: &[PrInfo], issue: u64) -> Option<&PrInfo> {
    let needle = format!("#{issue}");
    prs.iter().find(|pr| {
        pr.title
            .match_indices(&needle)
            .any(|(at, _)| !pr.title[at + needle.len()..].starts_with(|c: char| c.is_ascii_digit()))
    })
}

/// Why a run that is not live stopped, from its outcome class. A merged
/// row needs no reason.
pub(crate) fn stop_reason(class: Option<&str>, merged: bool) -> Option<String> {
    if merged {
        return None;
    }
    Some(match class {
        Some("usage_limit") => "stopped on a usage limit".to_string(),
        Some("ok") => "finished".to_string(),
        Some(other) => format!("stopped: {other}"),
        None => "stopped".to_string(),
    })
}
