//! When a run's owned items may be removed (issue #1808, C3, D6).
//!
//! Everything here is pure; the `gh` call lives in [`lookup_pr`] and the
//! `/proc` reads in [`run_is_active`]. Any uncertainty leads to a skip.

use super::run_dir::{RunReport, parse_start_ticks};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime};

pub(crate) const GH_TIMEOUT: Duration = Duration::from_secs(30);
const GH_MAX_STDOUT: usize = 1024 * 1024;

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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPr {
    number: u64,
    state: String,
    #[serde(default)]
    closed_at: Option<String>,
    #[serde(default)]
    merged_at: Option<String>,
}

fn parse_time(value: Option<String>) -> Result<Option<DateTime<Utc>>> {
    match value.filter(|v| !v.is_empty()) {
        None => Ok(None),
        Some(text) => Ok(Some(
            DateTime::parse_from_rfc3339(&text)
                .with_context(|| format!("invalid RFC 3339 time {text:?} from gh"))?
                .with_timezone(&Utc),
        )),
    }
}

/// Parses `gh pr list --json number,state,closedAt,mergedAt`. `[]` is
/// `Ok(None)`. With several PRs, an open one wins, then the latest closed.
pub(crate) fn parse_pr_list(stdout: &[u8]) -> Result<Option<PrInfo>> {
    if stdout.len() > GH_MAX_STDOUT {
        bail!("gh output is larger than {GH_MAX_STDOUT} bytes");
    }
    let raw: Vec<RawPr> = serde_json::from_slice(stdout).context("cannot parse gh output")?;
    let mut prs = Vec::with_capacity(raw.len());
    for pr in raw {
        prs.push(PrInfo {
            number: pr.number,
            state: pr.state.to_ascii_uppercase(),
            closed_at: parse_time(pr.closed_at)?,
            merged_at: parse_time(pr.merged_at)?,
        });
    }
    if let Some(open) = prs.iter().find(|p| p.state == "OPEN") {
        return Ok(Some(open.clone()));
    }
    Ok(prs.into_iter().max_by_key(|p| p.closed_at.or(p.merged_at)))
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

/// A branch name that git's check-ref-format would accept, that cannot be
/// read as an option, and that is safe to pass as `--head=<branch>`.
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

/// Runs `gh pr list --head=<branch> ...` in `repo_dir` and parses the answer.
pub(crate) fn lookup_pr(branch: &str, repo_dir: &Path) -> Result<Option<PrInfo>> {
    if !is_valid_branch(branch) {
        bail!("invalid branch name {branch:?}");
    }
    let mut command = Command::new("gh");
    command
        .arg("pr")
        .arg("list")
        .arg(format!("--head={branch}"))
        .args(["--state", "all", "--json", "number,state,closedAt,mergedAt"])
        .current_dir(repo_dir)
        // A tty read would stall the end-of-run sweep until the deadline.
        .stdin(std::process::Stdio::null())
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1");
    harden_git_env(&mut command);
    let output = crate::util::run_output_with_timeout_limited(command, GH_TIMEOUT, GH_MAX_STDOUT)
        .map_err(|error| anyhow::anyhow!("{}", first_line(&format!("{error:#}"))))?;
    if !output.status.success() {
        bail!(
            "exit {}: {}",
            output
                .status
                .code()
                .map_or_else(|| "signal".to_string(), |c| c.to_string()),
            crate::commands::hygiene::cleanup::sanitized_stderr_short(&output.stderr)
        );
    }
    parse_pr_list(&output.stdout)
}

/// The git settings `hardened_git` passes with `-c`, as environment for a
/// child (`gh`) that runs git itself in `repo_dir`: no system config, no
/// prompts, no fsmonitor and no hooks.
pub(crate) fn harden_git_env(command: &mut Command) {
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "core.fsmonitor")
        .env("GIT_CONFIG_VALUE_0", "false")
        .env("GIT_CONFIG_KEY_1", "core.hooksPath")
        .env("GIT_CONFIG_VALUE_1", "/dev/null");
}

fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default();
    line.chars().take(200).collect()
}

#[cfg(test)]
#[path = "tests_eligibility.rs"]
mod tests;
