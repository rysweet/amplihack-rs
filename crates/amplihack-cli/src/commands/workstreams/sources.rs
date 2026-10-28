//! The outside sources of `workstreams status`, behind traits so tests
//! inject fakes: GitHub's PR list, the live tmux sessions, a checkout's
//! origin and the login usage probe.
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

use super::logins::is_api_key_profile;
use crate::commands::profile::usage_cmd::{self, Row};
use crate::commands::recipe::run_record::{origin_url, repo_slug};
use amplihack_utils::bounded_process::run_helper;

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
    /// Every profile and whether it is an API-key profile; `Err` is a
    /// refusal to show logins at all, such as inside Docker, in fixed text
    /// that names no path.
    fn profiles(&self) -> Result<Vec<(String, bool)>, String>;
    /// The usage rows of those of `profiles` (as [`Self::profiles`] gave
    /// them) that are not API-key profiles. Stops when `cancel` is set.
    /// `Err` is fixed text, like a refusal.
    fn probe(&self, profiles: &[(String, bool)], cancel: &AtomicBool) -> Result<Vec<Row>, String>;
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

/// The profiles `amplihack profile usage` probes, read in process.
pub(crate) struct ProfileUsage {
    /// Each probe's own limit; the caller's deadline still applies.
    pub timeout: Duration,
}

impl UsageSource for ProfileUsage {
    fn profiles(&self) -> Result<Vec<(String, bool)>, String> {
        Ok(usage_cmd::listed_profiles()
            .map_err(str::to_string)?
            .into_iter()
            .map(|(name, dir)| {
                let api_key = !dir.as_os_str().is_empty() && is_api_key_profile(&dir);
                (name, api_key)
            })
            .collect())
    }

    fn probe(&self, profiles: &[(String, bool)], cancel: &AtomicBool) -> Result<Vec<Row>, String> {
        let plans: Vec<String> = profiles
            .iter()
            .filter(|(_, api_key)| !api_key)
            .map(|(name, _)| name.clone())
            .collect();
        // The errors may name a path or the claude binary; they are not shown.
        if profiles.is_empty() {
            // No profiles: `profile usage` shows nothing either.
            return Ok(Vec::new());
        }
        if plans.is_empty() {
            // Every profile uses an API key: only the `~/.claude` login.
            return usage_cmd::probe_home_listed(self.timeout, cancel)
                .map_err(|_| usage_cmd::REFUSED_PROBE.to_string());
        }
        // Every profile when none is an API-key profile. The `~/.claude` row
        // is probed either way.
        let names = if plans.len() == profiles.len() {
            Vec::new()
        } else {
            plans
        };
        usage_cmd::probe_listed(&names, self.timeout, cancel)
            .map_err(|_| usage_cmd::REFUSED_PROBE.to_string())
    }
}
