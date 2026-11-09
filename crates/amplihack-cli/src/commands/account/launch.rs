//! The launch integration of `amplihack account pick` (issue #2006).
//!
//! - `amplihack codex` uses an explicit `CODEX_HOME`, else
//!   `AMPLIHACK_CODEX_HOME`, unchanged and writes no lease. Otherwise it picks
//!   a home, holds the lease until Codex exits, and passes the home to Codex
//!   as `CODEX_HOME` and to nested launches as `AMPLIHACK_CODEX_HOME`.
//! - `amplihack claude` and `amplihack launch` pick only when nothing else
//!   names a profile or a config directory (see `profile::launch_profile_dir`).
//! - Docker and `--auto` never pick. `AMPLIHACK_ACCOUNT_PICK=off` turns
//!   picking off in every launch path; `codex` keeps it for Codex only.
//! - Any picker failure prints one warning and the launch continues on its
//!   default home; a launch is never blocked by the picker.

use super::Cli;
use super::config::PickConfig;
use super::lease;
use super::pick::{self, Pick, PickEnv, PickOpts};
use super::reading;
use amplihack_utils::codex_capacity;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Looks up one environment variable.
pub type Reader<'a> = &'a dyn Fn(&str) -> Option<OsString>;

pub const CODEX_HOME_ENV: &str = "CODEX_HOME";
pub const CODEX_HOME_ALIAS: &str = "AMPLIHACK_CODEX_HOME";
pub const CLAUDE_CONFIG_DIR_ENV: &str = "CLAUDE_CONFIG_DIR";
pub const CLAUDE_CONFIG_DIR_ALIAS: &str = "AMPLIHACK_CLAUDE_CONFIG_DIR";

/// A launch's lease; dropping it removes exactly that file.
#[derive(Debug)]
pub struct LeaseHandle(PathBuf);

impl LeaseHandle {
    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for LeaseHandle {
    fn drop(&mut self) {
        if let Err(e) = lease::remove(&self.0) {
            eprintln!(
                "warning: account pick: could not remove lease {}: {e}",
                self.0.display()
            );
        }
    }
}

fn non_empty(read: Reader<'_>, key: &str) -> Option<OsString> {
    read(key).filter(|v| !v.is_empty())
}

/// The Codex home a launch is pinned to: `CODEX_HOME` when set and not
/// empty (the user's own choice, passed through as it is), else
/// `AMPLIHACK_CODEX_HOME` when it is an absolute path.
pub fn codex_pin(read: Reader<'_>) -> Option<PathBuf> {
    if let Some(v) = non_empty(read, CODEX_HOME_ENV) {
        return Some(PathBuf::from(v));
    }
    non_empty(read, CODEX_HOME_ALIAS)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

/// Whether a Claude launch is pinned to a raw config directory.
pub fn claude_config_pinned(read: Reader<'_>) -> bool {
    non_empty(read, CLAUDE_CONFIG_DIR_ENV).is_some()
        || non_empty(read, CLAUDE_CONFIG_DIR_ALIAS).is_some()
}

/// What `amplihack recipe run` sets on the runner's process so a pinned home
/// survives the runner's environment filter, which keeps only `AMPLIHACK_*`
/// variables: each alias from its raw variable, only when the raw variable
/// is set and the alias is unset or empty.
pub fn recipe_aliases(read: Reader<'_>) -> Vec<(&'static str, OsString)> {
    [
        (CODEX_HOME_ENV, CODEX_HOME_ALIAS),
        (CLAUDE_CONFIG_DIR_ENV, CLAUDE_CONFIG_DIR_ALIAS),
    ]
    .into_iter()
    .filter(|(_, alias)| non_empty(read, alias).is_none())
    .filter_map(|(raw, alias)| non_empty(read, raw).map(|v| (alias, v)))
    .collect()
}

/// The raw Claude config directory a launch restores from the alias: the
/// alias when `CLAUDE_CONFIG_DIR` itself is unset or empty.
pub fn claude_config_from_alias(read: Reader<'_>) -> Option<PathBuf> {
    if non_empty(read, CLAUDE_CONFIG_DIR_ENV).is_some() {
        return None;
    }
    non_empty(read, CLAUDE_CONFIG_DIR_ALIAS).map(PathBuf::from)
}

fn env_var(k: &str) -> Option<OsString> {
    std::env::var_os(k)
}

pub(super) fn real_uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

/// Settings with their warnings printed once.
pub(super) fn config() -> PickConfig {
    let (cfg, warnings) = PickConfig::from_env();
    for w in warnings {
        eprintln!("{w}");
    }
    cfg
}

/// Runs one pick against the live system: the clock, `/proc`, this
/// process's uid and the configured lease directory.
pub(super) fn pick_live(
    cfg: &PickConfig,
    cli: Cli,
    floor: f64,
    lease_pid: Option<u32>,
    opts: PickOpts,
) -> Result<Pick, pick::PickError> {
    let read = move |c: Cli| reading::read_live(c, floor);
    let alive = lease::pid_alive;
    let env = PickEnv {
        now: chrono::Utc::now().timestamp(),
        proc_root: PathBuf::from("/proc"),
        lease_root: cfg.lease_dir.clone(),
        self_pid: std::process::id(),
        uid: real_uid(),
        lease_pid,
        job_cost_pct: cfg.job_cost_pct,
        ttl_secs: cfg.lease_ttl_secs,
        read: &read,
        alive: &alive,
    };
    pick::pick(&env, cli, opts)
}

fn docker_active(docker: bool) -> bool {
    docker
        || crate::docker::DockerDetector
            .activation_source(false)
            .is_some()
}

/// The Codex home for `amplihack codex`, and the lease to hold until Codex
/// exits. `None` keeps the current behaviour (Codex's own default home).
pub fn codex_home_for_launch(docker: bool, auto: bool) -> (Option<PathBuf>, Option<LeaseHandle>) {
    if docker_active(docker) || auto {
        return (None, None);
    }
    if let Some(pin) = codex_pin(&env_var) {
        return (Some(pin), None);
    }
    let cfg = config();
    if !cfg.switch.allows(Cli::Codex) {
        return (None, None);
    }
    let picked = pick_live(
        &cfg,
        Cli::Codex,
        codex_capacity::DEFAULT_FLOOR,
        Some(std::process::id()),
        PickOpts { write_lease: true },
    );
    match picked {
        Ok(p) => {
            let handle = p.lease.map(LeaseHandle::new);
            let Some(home) = p.chosen.map(|i| p.ranked[i].candidate.home.clone()) else {
                eprintln!("warning: account pick failed: nothing was chosen; using default home");
                return (None, None);
            };
            // A picked home did not come from the user's own setup, so it
            // gets the approval policy bootstrap gives `~/.codex`.
            if let Err(e) = amplihack_launcher::codex_config::configure(&home, true) {
                eprintln!(
                    "warning: could not configure Codex in {}: {e:#}",
                    home.display()
                );
            }
            (Some(home), handle)
        }
        Err(e) => {
            eprintln!("warning: account pick failed: {e}; using default home");
            (None, None)
        }
    }
}

/// The lease of the Claude profile picked for this process's launch, held
/// until [`release`].
static CLAUDE_LEASE: OnceLock<Mutex<Option<LeaseHandle>>> = OnceLock::new();

/// The Claude profile name to launch on, when nothing pins one: called by
/// the profile resolution only after no `--profile`, inherited profile or
/// default pointer was found, and never in Docker or `--auto`.
pub fn claude_pick() -> Option<String> {
    if claude_config_pinned(&env_var) {
        return None;
    }
    let cfg = config();
    if !cfg.switch.allows(Cli::Claude) {
        return None;
    }
    match pick_live(
        &cfg,
        Cli::Claude,
        codex_capacity::DEFAULT_FLOOR,
        Some(std::process::id()),
        PickOpts { write_lease: true },
    ) {
        Ok(p) => {
            let name = p.chosen.map(|i| p.ranked[i].candidate.account.clone());
            if let Some(path) = p.lease {
                let slot = CLAUDE_LEASE.get_or_init(|| Mutex::new(None));
                *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(LeaseHandle::new(path));
            }
            name
        }
        Err(e) => {
            eprintln!("warning: account pick failed: {e}; using default home");
            None
        }
    }
}

/// Reports a picked Claude profile the launch could not use, and drops its
/// lease.
pub fn claude_pick_refused(err: &dyn std::fmt::Display) {
    eprintln!("warning: account pick failed: {err}; using default home");
    release();
}

/// Drops the lease of this process's Claude pick, if any.
pub fn release() {
    if let Some(slot) = CLAUDE_LEASE.get() {
        slot.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}
