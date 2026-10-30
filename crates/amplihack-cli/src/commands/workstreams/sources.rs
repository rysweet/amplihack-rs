//! The outside sources of `workstreams status`, behind traits so tests
//! inject fakes: GitHub's PR list, the live tmux sessions, a checkout's
//! origin and `amplihack profile usage --json`.
//!
//! Every subprocess runs through
//! [`bounded_process`](amplihack_utils::bounded_process), which bounds its
//! time, its output and its process group.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use anyhow::{Result, bail};
use serde::Deserialize;

use crate::commands::recipe::run_record::{origin_url, repo_slug};
use amplihack_utils::bounded_process::{self, Ended, Limits, run_helper};

const TMUX_TIMEOUT: Duration = Duration::from_secs(3);
const GH_TIMEOUT: Duration = Duration::from_secs(10);
const GIT_TIMEOUT: Duration = Duration::from_secs(3);

/// One PR from `gh pr list`. The title is used only to match an issue.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct PrInfo {
    pub number: u64,
    pub title: String,
    pub state: String,
    #[serde(default)]
    pub mergeable: String,
}

pub(crate) trait PrSource {
    /// The PRs of `repo` (`owner/name`), newest first.
    fn list(&self, repo: &str) -> Result<Vec<PrInfo>>;
}

pub(crate) trait LiveSessions {
    /// Every live tmux session's name and creation time (Unix seconds).
    fn list(&self) -> Vec<(String, i64)>;
}

pub(crate) trait OriginSource {
    /// The `origin` URL of the checkout at `cwd`. It may hold a credential:
    /// it is used only to derive the auto-drive state key.
    fn origin(&self, cwd: &Path) -> Option<String>;
}

pub(crate) trait UsageSource: Send + Sync {
    /// The standard output of `amplihack profile usage --json`. Stops when
    /// `cancel` is set. `Err` is fixed text that names no path, such as a
    /// refusal inside Docker.
    fn usage_json(&self, cancel: &AtomicBool) -> Result<Vec<u8>, String>;
}

// ---------------------------------------------------------------------------
// Production sources
// ---------------------------------------------------------------------------

/// `gh pr list` for one repository.
pub(crate) struct GhPrs;

impl PrSource for GhPrs {
    fn list(&self, repo: &str) -> Result<Vec<PrInfo>> {
        // Only a validated `owner/name` reaches gh, as one `--repo=` word.
        if repo_slug(&format!("https://github.com/{repo}")).as_deref() != Some(repo) {
            bail!("not a GitHub owner/name");
        }
        let mut cmd = Command::new("gh");
        cmd.args([
            "pr",
            "list",
            &format!("--repo={repo}"),
            "--state",
            "all",
            "--limit",
            "100",
            "--json",
            "number,title,state,mergeable",
        ])
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1");
        let Some(out) = run_helper(cmd, GH_TIMEOUT) else {
            bail!("gh pr list did not finish within {GH_TIMEOUT:?}");
        };
        if !out.success() {
            bail!("gh pr list failed");
        }
        serde_json::from_slice(&out.stdout)
            .map_err(|_| anyhow::anyhow!("gh pr list output is not a PR list"))
    }
}

/// `tmux ls` on the caller's server.
pub(crate) struct TmuxSessions;

impl LiveSessions for TmuxSessions {
    fn list(&self) -> Vec<(String, i64)> {
        let mut cmd = Command::new("tmux");
        cmd.args(["ls", "-F", "#{session_name} #{session_created}"]);
        match run_helper(cmd, TMUX_TIMEOUT) {
            Some(out) if out.success() => parse_tmux(&String::from_utf8_lossy(&out.stdout)),
            // No tmux, no server or no answer: nothing is live.
            _ => Vec::new(),
        }
    }
}

/// `<name> <created>` lines; others are skipped.
pub(crate) fn parse_tmux(text: &str) -> Vec<(String, i64)> {
    text.lines()
        .filter_map(|line| {
            let (name, created) = line.trim_end().rsplit_once(' ')?;
            Some((name.to_string(), created.parse().ok()?))
        })
        .collect()
}

/// `git remote get-url origin`, bounded and hardened, run once per
/// checkout: runs launched from the same directory share the answer.
#[derive(Default)]
pub(crate) struct GitOrigin {
    seen: RefCell<HashMap<PathBuf, Option<String>>>,
}

impl OriginSource for GitOrigin {
    fn origin(&self, cwd: &Path) -> Option<String> {
        self.seen
            .borrow_mut()
            .entry(cwd.to_path_buf())
            .or_insert_with(|| origin_url(cwd, GIT_TIMEOUT))
            .clone()
    }
}

/// Bytes kept from `profile usage --json`; one row is well under 4 KiB.
const USAGE_CAP: usize = 1024 * 1024;
/// How long the pipes may stay open after `profile usage` exits.
const USAGE_READ_GRACE: Duration = Duration::from_secs(1);
/// Shown when `profile usage` could not be run or failed. It refuses inside
/// Docker and without a `claude` binary; its own stderr says why and is not
/// shown here, as it may name a path.
pub(crate) const USAGE_FAILED: &str =
    "amplihack profile usage failed; run it on its own to see why";

/// `amplihack profile usage --json --timeout-secs <probe>`, run as this
/// same binary through [`bounded_process`]. Using its stable JSON keeps one
/// implementation of every login rule (API-key, overflow and live
/// detection) in `profile usage`.
pub(crate) struct ProfileUsage {
    /// Each probe's own limit, passed as `--timeout-secs`; a probe that runs
    /// out gives a `timeout` row rather than holding up the others.
    pub probe_timeout: Duration,
    /// When the whole command is stopped.
    pub deadline: Duration,
}

impl UsageSource for ProfileUsage {
    fn usage_json(&self, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
        let exe = std::env::current_exe().map_err(|_| USAGE_FAILED.to_string())?;
        let mut cmd = Command::new(exe);
        cmd.args([
            "profile",
            "usage",
            "--json",
            "--timeout-secs",
            &self.probe_timeout.as_secs().max(1).to_string(),
        ]);
        let limits = Limits {
            timeout: self.deadline,
            cap: USAGE_CAP,
            read_grace: USAGE_READ_GRACE,
        };
        let out = bounded_process::run(cmd, limits, Some(cancel))
            .map_err(|_| USAGE_FAILED.to_string())?;
        match out.ended {
            Ended::Exited(_) if out.success() && out.complete => Ok(out.stdout),
            Ended::Stopped => Err(no_reading_by(self.deadline)),
            _ => Err(USAGE_FAILED.to_string()),
        }
    }
}

/// The text shown when `profile usage` did not finish by the deadline.
pub(crate) fn no_reading_by(deadline: Duration) -> String {
    format!(
        "no reading within {}s (raise --logins-deadline)",
        deadline.as_secs()
    )
}
