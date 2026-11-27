//! When a run's owned items may be removed (issue #1808, C3, D6).
//!
//! The age rules are shared by the local and manual sweeps. Any uncertainty
//! leads to a skip.

use super::run_dir::{RunReport, parse_start_ticks};
use chrono::{DateTime, Utc};
use std::path::Path;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    Eligible { reason: String },
    Skip { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrInfo {
    pub number: u64,
    pub state: String,
    pub closed_at: Option<DateTime<Utc>>,
    pub merged_at: Option<DateTime<Utc>>,
}

/// The verdict for a run whose branch has a PR.
pub(crate) fn pr_verdict(pr: &PrInfo, now: DateTime<Utc>, keep: Duration) -> Verdict {
    let verb = match pr.state.as_str() {
        "MERGED" => "merged",
        "CLOSED" => "closed",
        "OPEN" => {
            return Verdict::Skip {
                reason: format!("PR #{} is open", pr.number),
            };
        }
        other => {
            return Verdict::Skip {
                reason: format!("PR #{} has unknown state {other:?}", pr.number),
            };
        }
    };
    let Some(closed) = pr.closed_at.or(pr.merged_at) else {
        return Verdict::Skip {
            reason: format!("PR #{} is {verb} but has no closedAt", pr.number),
        };
    };
    let stamp = closed.format("%Y-%m-%dT%H:%M:%SZ");
    let elapsed = (now - closed).to_std().ok();
    match elapsed {
        Some(age) if age >= keep => Verdict::Eligible {
            reason: format!("PR #{} {verb} {stamp}", pr.number),
        },
        _ => Verdict::Skip {
            reason: format!("within keep period (PR #{} {verb} {stamp})", pr.number),
        },
    }
}

/// The verdict for a run with no PR or no recorded branch: dated by the later
/// of `ended_at` and the mtime of `run.json`. A future time counts as within
/// the keep period.
pub(crate) fn no_pr_verdict(
    ended_at: Option<DateTime<Utc>>,
    run_json_mtime: SystemTime,
    now: SystemTime,
    keep: Duration,
) -> Verdict {
    let ended = ended_at.map(SystemTime::from);
    let latest = match ended {
        Some(e) if e > run_json_mtime => e,
        _ => run_json_mtime,
    };
    match now.duration_since(latest) {
        Ok(age) if age >= keep => Verdict::Eligible {
            reason: format!("no PR; last activity {} hours ago", age.as_secs() / 3600),
        },
        _ => Verdict::Skip {
            reason: "within keep period (no PR)".to_string(),
        },
    }
}

/// `^[0-9a-f-]{1,64}$`.
pub(crate) fn is_valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b) || b == b'-')
}

/// A branch name that git's check-ref-format would accept and that cannot
/// be read as an option.
pub(crate) fn is_valid_branch(s: &str) -> bool {
    if s.is_empty() || s.len() > 255 || s == "@" {
        return false;
    }
    if s.starts_with('-') || s.starts_with('/') || s.ends_with('/') || s.ends_with('.') {
        return false;
    }
    if s.contains("..") || s.contains("@{") || s.contains("//") {
        return false;
    }
    if s.bytes().any(|b| {
        b < 0x20 || b == 0x7f || matches!(b, b' ' | b'~' | b'^' | b':' | b'?' | b'*' | b'[' | b'\\')
    }) {
        return false;
    }
    s.split('/')
        .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}

/// Whether a run is active: its pid is alive with the recorded start ticks.
/// A finalised run (with `ended_at`) is never active. When `/proc` cannot
/// tell, the run is treated as active.
pub(crate) fn run_is_active(report: &RunReport) -> bool {
    if report.ended_at.as_deref().is_some_and(|s| !s.is_empty()) {
        return false;
    }
    if report.pid == 0 {
        return false;
    }
    if !Path::new("/proc/self/stat").exists() {
        return true;
    }
    let stat = match std::fs::read_to_string(format!("/proc/{}/stat", report.pid)) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    match (parse_start_ticks(&stat), report.pid_start_ticks) {
        (Some(actual), Some(recorded)) => actual == recorded,
        // Without both values a reused pid cannot be ruled out.
        _ => true,
    }
}

#[cfg(test)]
#[path = "tests_eligibility.rs"]
mod tests;
