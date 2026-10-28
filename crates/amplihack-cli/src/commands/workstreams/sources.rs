//! The outside sources of `workstreams status`, behind traits so tests
//! inject fakes: GitHub's PR list, the live tmux sessions, a checkout's
//! origin and the login usage probe.
//!
//! Every subprocess runs through [`run_bounded`]: no shell, stdin from
//! `/dev/null`, its own process group, at most [`PIPE_CAP`] bytes kept from
//! each pipe, and the whole group killed when the limit passes.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use serde::Deserialize;

use super::logins::is_api_key_profile;
use crate::commands::profile::usage_cmd::{self, Row};
use crate::commands::recipe::run_record::{origin_url, repo_slug};

/// Bytes kept from each of a subprocess's stdout and stderr.
const PIPE_CAP: usize = 4 * 1024 * 1024;
/// How long the pipes may stay open after the process group is gone.
const READ_GRACE: Duration = Duration::from_secs(1);
const POLL: Duration = Duration::from_millis(20);
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
    /// refusal to show logins at all, such as inside Docker.
    fn profiles(&self) -> Result<Vec<(String, bool)>, String>;
    /// The usage rows of the profiles that are not API-key profiles. Stops
    /// when `cancel` is set.
    fn probe(&self, cancel: &AtomicBool) -> Result<Vec<Row>, String>;
}

/// What a bounded subprocess left.
pub(crate) struct Bounded {
    pub success: bool,
    pub stdout: Vec<u8>,
}

/// Runs `cmd` as described in the module comment. `None` when it cannot
/// start, cannot be waited for, or is still running at `timeout`.
pub(crate) fn run_bounded(mut cmd: Command, timeout: Duration) -> Option<Bounded> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = cmd.spawn().ok()?;
    let out = drain(child.stdout.take());
    // Read so a chatty process cannot fill the pipe and stall; never shown.
    let err = drain(child.stderr.take());
    let start = Instant::now();
    let finished = loop {
        match exited(&child) {
            Ok(true) => break true,
            Ok(false) if start.elapsed() < timeout => std::thread::sleep(POLL),
            _ => break false,
        }
    };
    // The child is not reaped yet, so its pid is still its group's id.
    kill_group(&child);
    let _ = child.kill();
    let status = child.wait().ok();
    if !finished {
        return None;
    }
    let deadline = Instant::now() + READ_GRACE;
    while !(out.done() && err.done()) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    Some(Bounded {
        success: status.is_some_and(|s| s.success()),
        stdout: out.take(),
    })
}

/// A pipe read on a detached thread, keeping at most [`PIPE_CAP`] bytes.
struct Drain {
    kept: Arc<Mutex<Vec<u8>>>,
    done: Arc<AtomicBool>,
}

impl Drain {
    fn done(&self) -> bool {
        self.done.load(std::sync::atomic::Ordering::Acquire)
    }

    fn take(&self) -> Vec<u8> {
        std::mem::take(&mut *self.kept.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

fn drain(source: Option<impl Read + Send + 'static>) -> Drain {
    let kept = Arc::new(Mutex::new(Vec::new()));
    let done = Arc::new(AtomicBool::new(false));
    let Some(mut source) = source else {
        done.store(true, std::sync::atomic::Ordering::Release);
        return Drain { kept, done };
    };
    let (k, d) = (Arc::clone(&kept), Arc::clone(&done));
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match source.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    let mut kept = k.lock().unwrap_or_else(PoisonError::into_inner);
                    let room = PIPE_CAP.saturating_sub(kept.len());
                    kept.extend_from_slice(&chunk[..n.min(room)]);
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        d.store(true, std::sync::atomic::Ordering::Release);
    });
    Drain { kept, done }
}

/// Whether the child has exited, without reaping it, so that
/// [`kill_group`] still targets its own group.
fn exited(child: &std::process::Child) -> std::io::Result<bool> {
    let pid = libc::id_t::from(child.id());
    // SAFETY: an all-zero `siginfo_t` is a valid value for waitid to fill.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    loop {
        // SAFETY: `info` is a valid, writable siginfo_t; `pid` is our own
        // unreaped child. WNOWAIT leaves it waitable for `Child::wait`.
        let rc = unsafe {
            libc::waitid(
                libc::P_PID,
                pid,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if rc == 0 {
            // SAFETY: waitid succeeded, so `si_pid` is initialised; it is 0
            // when the child has not exited yet (WNOHANG).
            return Ok(unsafe { info.si_pid() } != 0);
        }
        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
}

/// SIGKILL to the group the unreaped child leads.
fn kill_group(child: &std::process::Child) {
    let Ok(pid) = i32::try_from(child.id()) else {
        return;
    };
    // SAFETY: `kill` has no memory-safety preconditions. A negative pid
    // targets the group; ESRCH (group already empty) is ignored.
    let _ = unsafe { libc::kill(-pid, libc::SIGKILL) };
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
        let Some(out) = run_bounded(cmd, GH_TIMEOUT) else {
            bail!("gh pr list did not finish within {GH_TIMEOUT:?}");
        };
        if !out.success {
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
        match run_bounded(cmd, TMUX_TIMEOUT) {
            Some(out) if out.success => parse_tmux(&String::from_utf8_lossy(&out.stdout)),
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

/// `git remote get-url origin`, bounded and hardened.
pub(crate) struct GitOrigin;

impl OriginSource for GitOrigin {
    fn origin(&self, cwd: &Path) -> Option<String> {
        origin_url(cwd, GIT_TIMEOUT)
    }
}

/// The profiles `amplihack profile usage` probes, read in process.
pub(crate) struct ProfileUsage {
    /// Each probe's own limit; the caller's deadline still applies.
    pub timeout: Duration,
}

impl ProfileUsage {
    fn listed(&self) -> Result<Vec<(String, PathBuf)>, String> {
        usage_cmd::listed_profiles().map_err(|e| format!("{e:#}"))
    }
}

impl UsageSource for ProfileUsage {
    fn profiles(&self) -> Result<Vec<(String, bool)>, String> {
        Ok(self
            .listed()?
            .into_iter()
            .map(|(name, dir)| {
                let api_key = !dir.as_os_str().is_empty() && is_api_key_profile(&dir);
                (name, api_key)
            })
            .collect())
    }

    fn probe(&self, cancel: &AtomicBool) -> Result<Vec<Row>, String> {
        let profiles = self.profiles()?;
        let plans: Vec<String> = profiles
            .iter()
            .filter(|(_, api_key)| !api_key)
            .map(|(name, _)| name.clone())
            .collect();
        if plans.is_empty() {
            return Ok(Vec::new());
        }
        // Every profile when none is an API-key profile, so the `~/.claude`
        // row is probed as `profile usage` would.
        let names = if plans.len() == profiles.len() {
            Vec::new()
        } else {
            plans
        };
        usage_cmd::probe_listed(&names, self.timeout, cancel).map_err(|e| format!("{e:#}"))
    }
}
