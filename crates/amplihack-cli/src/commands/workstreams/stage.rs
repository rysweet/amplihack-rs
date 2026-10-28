//! A run's stage, progress, marker and stage text (issue #1710). Pure: every
//! input is in [`RunFacts`], and the clock is passed in.
//!
//! | Progress | Stage |
//! | --- | --- |
//! | 0% | queued (manifest only) |
//! | 15% | building |
//! | 30% | PR open |
//! | 40–60% | review loop, 4% per crusty round |
//! | 70–95% | merge-ready loop, 5% per merge-ready round |
//! | 95% | merge-ready loop done, not merged yet |
//! | 100% | merged |

use chrono::{DateTime, Utc};

use super::autodrive::{AutoDriveState, layout};
use super::sources::Pr;
use crate::commands::recipe::run::limit_patterns::limit_message;

/// What the PR source says about a run's PR.
#[derive(Debug, Clone, Default)]
pub(crate) enum PrLookup {
    Found(Pr),
    /// The list was read and holds no PR for the run.
    #[default]
    None,
    /// The list could not be read.
    Unknown,
}

/// Everything known about one row.
#[derive(Debug, Clone, Default)]
pub(crate) struct RunFacts {
    /// A manifest row with no run yet.
    pub queued: bool,
    /// Its tmux session is live.
    pub live: bool,
    pub pr: PrLookup,
    pub autodrive: Option<AutoDriveState>,
    /// The log pointer's final exit code.
    pub exit_code: Option<i32>,
    /// The end of its log reads as a usage-limit stop ([`tail_shows_limit`]).
    pub stopped_on_limit: bool,
    pub started: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Marker {
    /// Running.
    Live,
    /// Waiting or stopped.
    Waiting,
    /// Merged: no marker.
    Blank,
}

impl Marker {
    pub(crate) fn glyph(self) -> char {
        match self {
            Self::Live => '▶',
            Self::Waiting => '⏸',
            Self::Blank => ' ',
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Status {
    pub pct: u8,
    pub marker: Marker,
    pub text: String,
}

/// The run's PR: the meta file's number, else the log pointer's, else the PR
/// whose head branch is the run's branch, else the PR whose title names
/// `#<issue>`. `prs` is `None` when the PR list could not be read.
pub(crate) fn find_pr(
    prs: Option<&[Pr]>,
    meta_pr: Option<u64>,
    pointer_pr: Option<u64>,
    branch: Option<&str>,
    issue: Option<u64>,
) -> PrLookup {
    let Some(prs) = prs else {
        return PrLookup::Unknown;
    };
    let hit = if let Some(n) = meta_pr.or(pointer_pr) {
        prs.iter().find(|p| p.number == n)
    } else {
        branch
            .filter(|b| !b.is_empty())
            .and_then(|b| prs.iter().find(|p| p.head_ref_name == b))
            .or_else(|| issue.and_then(|i| prs.iter().find(|p| title_names(&p.title, i))))
    };
    hit.map_or(PrLookup::None, |p| PrLookup::Found(p.clone()))
}

/// `title` holds `#<issue>` followed by a non-digit or the end.
fn title_names(title: &str, issue: u64) -> bool {
    let needle = format!("#{issue}");
    title.match_indices(&needle).any(|(at, _)| {
        !title[at + needle.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
    })
}

pub(crate) fn status(f: &RunFacts, now: DateTime<Utc>) -> Status {
    if f.queued {
        return Status {
            pct: 0,
            marker: Marker::Waiting,
            text: "queued".to_string(),
        };
    }
    let l = layout();
    let empty = AutoDriveState::default();
    let ad = f.autodrive.as_ref().unwrap_or(&empty);
    let has = |phase: &str| ad.phases.iter().any(|p| p == phase);
    let pr = match &f.pr {
        PrLookup::Found(pr) => Some(pr),
        _ => None,
    };
    let pr_state = pr.map(|p| p.state.to_ascii_uppercase());
    let elapsed = format_elapsed(f.started, now);

    if pr_state.as_deref() == Some("MERGED") || has(&l.phase_merged) {
        return Status {
            pct: 100,
            marker: Marker::Blank,
            text: format!("merged, {elapsed}"),
        };
    }

    let crusty = ad.crusty_rounds;
    let ready = ad.merge_ready_rounds;
    let (pct, mut stage) = if has(&l.phase_merge_ready_loop) {
        (95, rounds_text("merge-ready", ready))
    } else if has(&l.phase_crusty_loop) || ready > 0 {
        (cap(70, 5, ready, 95), rounds_text("merge-ready", ready))
    } else if has(&l.phase_build) || crusty > 0 {
        (cap(40, 4, crusty, 60), rounds_text("review", crusty))
    } else if pr.is_some() {
        (30, "PR open".to_string())
    } else {
        (15, "building".to_string())
    };
    if pr_state.as_deref() == Some("CLOSED") {
        stage = "PR closed".to_string();
    }

    let mut parts = vec![stage];
    if crusty > 0
        && let Some(k) = ad.concerns
    {
        parts.push(format!("{k} concerns"));
    }
    match &f.pr {
        PrLookup::Found(p) if p.mergeable.eq_ignore_ascii_case("CONFLICTING") => {
            let base = p
                .base_ref_name
                .as_deref()
                .filter(|b| !b.is_empty())
                .unwrap_or("main");
            parts.push(format!("conflicts with {base}"));
        }
        PrLookup::Unknown => parts.push("PR state unknown".to_string()),
        _ => {}
    }
    let marker = if f.live {
        Marker::Live
    } else {
        parts.push(reason(f));
        Marker::Waiting
    };
    parts.push(elapsed);
    Status {
        pct,
        marker,
        text: parts.join(", "),
    }
}

/// `base + step·rounds`, at most `max`, without overflow.
fn cap(base: u32, step: u32, rounds: u32, max: u32) -> u8 {
    let pct = base.saturating_add(step.saturating_mul(rounds)).min(max);
    u8::try_from(pct).unwrap_or(100)
}

fn rounds_text(name: &str, rounds: u32) -> String {
    if rounds == 0 {
        format!("{name} loop")
    } else {
        format!("{name} round {rounds}")
    }
}

/// Why a run that is not live is not running. Fixed text only: the log is
/// classified, never quoted.
fn reason(f: &RunFacts) -> String {
    if f.stopped_on_limit {
        return "stopped on a usage limit".to_string();
    }
    match f.exit_code {
        Some(0) => "finished".to_string(),
        Some(code) => format!("failed (exit {code})"),
        None => "(waiting or stopped)".to_string(),
    }
}

/// The end of a log reads as a harness usage-limit stop. Discovery calls this
/// as it reads each log and keeps only the answer, so the 64 KiB tails of
/// thousands of runs are never held at once. The reset time is not used, so
/// the clock passed to the classifier does not matter.
pub(crate) fn tail_shows_limit(tail: &[u8]) -> bool {
    limit_message(tail, b"", 0).is_some()
}

/// Time since `started`: `<1m`, `Nm`, `NhMMm` or `NdNh`; `?` when the start
/// is unknown or in the future.
pub(crate) fn format_elapsed(started: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String {
    let Some(started) = started else {
        return "?".to_string();
    };
    let secs = now.signed_duration_since(started).num_seconds();
    if secs < 0 {
        return "?".to_string();
    }
    let mins = secs / 60;
    let (hours, days) = (mins / 60, mins / (60 * 24));
    if mins < 1 {
        "<1m".to_string()
    } else if hours < 1 {
        format!("{mins}m")
    } else if days < 1 {
        format!("{hours}h{:02}m", mins % 60)
    } else {
        format!("{days}d{}h", hours % 24)
    }
}
