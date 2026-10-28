//! The outside world `workstreams status` reads, behind traits so tests pass
//! fakes: the clock, live tmux sessions, the PR list, a repo's origin, and
//! the login usage probes. Every real source runs one program directly (no
//! shell), with stdin from `/dev/null`, a time limit and an output cap, and
//! reports a failure without its text.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use serde::Deserialize;

use crate::commands::profile::usage_cmd;
use crate::commands::profile::usage_parse::UsageReport;
use crate::util::run_output_with_timeout_limited;

const TMUX_TIMEOUT: Duration = Duration::from_secs(3);
const GIT_TIMEOUT: Duration = Duration::from_secs(3);
const GH_TIMEOUT: Duration = Duration::from_secs(10);
const SMALL_OUTPUT: usize = 64 * 1024;
const GH_OUTPUT: usize = 2 * 1024 * 1024;

pub(crate) trait Clock {
    /// Local time, for the header.
    fn now(&self) -> DateTime<FixedOffset>;
}

/// A live tmux session.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LiveSession {
    pub name: String,
    pub created: Option<DateTime<Utc>>,
}

pub(crate) trait LiveSessions {
    /// Every live session; empty when there is no tmux or no server.
    fn list(&self) -> Vec<LiveSession>;
}

/// One PR as `gh pr list --json number,title,state,mergeable,headRefName,
/// baseRefName` prints it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Pr {
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub mergeable: String,
    #[serde(default)]
    pub head_ref_name: String,
    #[serde(default)]
    pub base_ref_name: Option<String>,
}

pub(crate) trait PrSource: Sync {
    /// The repo's recent PRs, run from `cwd`. `Err(())` when they cannot be
    /// read; the cause is never shown.
    #[allow(clippy::result_unit_err)]
    fn list(&self, cwd: &Path) -> Result<Vec<Pr>, ()>;
}

pub(crate) trait GitOrigin: Sync {
    /// `git remote get-url origin` in `cwd`.
    fn origin(&self, cwd: &Path) -> Option<String>;
}

/// A profile the usage source lists.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProfileInfo {
    pub name: String,
    /// Its login is live in `~/.claude`.
    pub live: bool,
    /// An API-key profile: billed per token, never probed.
    pub api_key: bool,
}

/// Probes under way: the profiles, a channel that gets one report per
/// probed profile, and a way to stop the probes still running.
pub(crate) struct UsageStream {
    pub profiles: Vec<ProfileInfo>,
    pub results: Receiver<(String, UsageReport)>,
    pub cancel: Box<dyn FnOnce() + Send>,
}

pub(crate) trait UsageSource {
    /// Start probing every profile, each probe limited to `timeout`. Returns
    /// at once; results arrive on the stream.
    fn start(&self, timeout: Duration) -> anyhow::Result<UsageStream>;
}

/// Everything `run_with` reads.
pub(crate) struct Sources<'a> {
    pub clock: &'a dyn Clock,
    pub live: &'a dyn LiveSessions,
    pub prs: &'a dyn PrSource,
    pub git: &'a dyn GitOrigin,
    pub usage: &'a dyn UsageSource,
    /// The recipe log directory; `None` when it cannot be named.
    pub log_dir: Option<PathBuf>,
    /// The auto-drive state root.
    pub state_root: Option<PathBuf>,
    /// The manifest read when `--manifest` is not given.
    pub default_manifest: Option<PathBuf>,
}

// ------------------------------------------------------------------ real

pub(crate) struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<FixedOffset> {
        chrono::Local::now().fixed_offset()
    }
}

/// `tmux ls` on the user's default server.
pub(crate) struct TmuxSessions;

impl LiveSessions for TmuxSessions {
    fn list(&self) -> Vec<LiveSession> {
        let mut cmd = Command::new("tmux");
        cmd.args(["list-sessions", "-F", "#{session_name} #{session_created}"])
            .stdin(Stdio::null());
        let Ok(out) = run_output_with_timeout_limited(cmd, TMUX_TIMEOUT, SMALL_OUTPUT) else {
            return Vec::new();
        };
        if !out.status.success() {
            return Vec::new();
        }
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| {
                let (name, created) = line.rsplit_once(' ')?;
                Some(LiveSession {
                    name: name.to_string(),
                    created: created
                        .parse::<i64>()
                        .ok()
                        .and_then(|secs| Utc.timestamp_opt(secs, 0).single()),
                })
            })
            .collect()
    }
}

/// `gh pr list`, once per repo directory.
#[derive(Default)]
pub(crate) struct GhPrs {
    cache: Mutex<HashMap<PathBuf, Result<Vec<Pr>, ()>>>,
}

impl PrSource for GhPrs {
    fn list(&self, cwd: &Path) -> Result<Vec<Pr>, ()> {
        let key = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
        if let Some(hit) = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key)
        {
            return hit.clone();
        }
        let mut cmd = Command::new("gh");
        cmd.args([
            "pr",
            "list",
            "--state",
            "all",
            "--limit",
            "100",
            "--json",
            "number,title,state,mergeable,headRefName,baseRefName",
        ])
        .current_dir(&key)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("NO_COLOR", "1")
        .env("GH_PAGER", "")
        .stdin(Stdio::null());
        let result = run_output_with_timeout_limited(cmd, GH_TIMEOUT, GH_OUTPUT)
            .ok()
            .filter(|out| out.status.success())
            .and_then(|out| serde_json::from_slice::<Vec<Pr>>(&out.stdout).ok())
            .ok_or(());
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key, result.clone());
        result
    }
}

/// `git remote get-url origin`, with the repo's own config unable to run
/// anything.
pub(crate) struct GitCli;

impl GitOrigin for GitCli {
    fn origin(&self, cwd: &Path) -> Option<String> {
        let mut cmd = amplihack_git::command();
        cmd.args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "protocol.allow=never",
            "-C",
        ])
        .arg(cwd)
        .args(["remote", "get-url", "origin"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null());
        let out = run_output_with_timeout_limited(cmd, GIT_TIMEOUT, SMALL_OUTPUT).ok()?;
        if !out.status.success() {
            return None;
        }
        let url = String::from_utf8(out.stdout).ok()?.trim().to_string();
        (!url.is_empty()).then_some(url)
    }
}

/// The probes `amplihack profile usage` runs, read only.
pub(crate) struct ProfileUsage;

impl UsageSource for ProfileUsage {
    fn start(&self, timeout: Duration) -> anyhow::Result<UsageStream> {
        let c = usage_cmd::collect_readonly(timeout, super::logins::is_api_key_profile)?;
        Ok(UsageStream {
            profiles: c
                .profiles
                .into_iter()
                .map(|p| ProfileInfo {
                    name: p.name,
                    live: p.live,
                    api_key: p.api_key,
                })
                .collect(),
            results: c.results,
            cancel: Box::new(usage_cmd::cancel_probes),
        })
    }
}
