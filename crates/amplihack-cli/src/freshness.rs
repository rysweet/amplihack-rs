//! Upstream freshness checks for ancillary tooling.
//!
//! The launcher's update path keeps the amplihack binaries themselves up to
//! date.  One ancillary tool can silently drift:
//!
//! - `recipe-runner-rs`, installed via `cargo install --git`. Once present
//!   it stays on whatever commit was current at install time.
//!
//! **Framework assets** (agents, skills, commands, hook specs) are now bundled
//! in the amplihack-rs source tree and delivered via binary updates (issue
//! #254).  The former upstream freshness check against `rysweet/amplihack`
//! has been removed.
//!
//! For the recipe-runner check, this module adds optional, cooldown-gated
//! freshness checks:
//!
//! 1. Reads the installed SHA from a small JSON state file.
//! 2. If the 24h cooldown has not expired, does nothing.
//! 3. Otherwise fetches the upstream HEAD SHA via the GitHub commits API.
//! 4. If the SHAs differ, runs the upgrade (`cargo install`).
//! 5. Records the new SHA + timestamp on success.
//!
//! Every step is best-effort — a network failure, rate-limit, or even a
//! completely malformed state file results in a `tracing::warn!` and an
//! early return, not a launch failure. The whole flow can be disabled with
//! `AMPLIHACK_NO_FRESHNESS_CHECK=1` (or the usual non-interactive guards).

use crate::update::fetch_branch_head_sha;
use crate::util::{is_noninteractive, run_with_timeout};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const COOLDOWN_SECS: u64 = 24 * 60 * 60;
const CARGO_INSTALL_TIMEOUT: Duration = Duration::from_secs(600);
const NO_FRESHNESS_ENV: &str = "AMPLIHACK_NO_FRESHNESS_CHECK";

// ---------------------------------------------------------------------------
// State file
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct FreshnessState {
    /// Full git SHA of the upstream HEAD at the time of the last successful
    /// install. Empty when unknown.
    installed_sha: String,
    /// UNIX timestamp of the last freshness check (attempt — not necessarily
    /// a successful install). Used by the cooldown gate.
    checked_at: u64,
}

impl FreshnessState {
    fn read(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn write(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let body = serde_json::to_string_pretty(self)?;
        fs::write(path, body + "\n")
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(())
    }

    fn is_in_cooldown(&self) -> bool {
        let age = now_secs().saturating_sub(self.checked_at);
        age < COOLDOWN_SECS
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .context("HOME is not set")
}

/// State files live under `~/.amplihack/state/`. Keeping them outside the
/// staged `.claude/` tree means a framework reinstall (which wipes that
/// tree) doesn't also wipe the last-installed-SHA record.
fn state_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(".amplihack").join("state"))
}

fn skip_freshness_checks() -> bool {
    is_noninteractive()
        || std::env::var(NO_FRESHNESS_ENV).as_deref() == Ok("1")
        || std::env::var("AMPLIHACK_NO_UPDATE_CHECK").as_deref() == Ok("1")
}

// ---------------------------------------------------------------------------
// Recipe runner (rysweet/amplihack-recipe-runner)
// ---------------------------------------------------------------------------

const RECIPE_RUNNER_REPO: &str = "rysweet/amplihack-recipe-runner";
const RECIPE_RUNNER_GIT_URL: &str = "https://github.com/rysweet/amplihack-recipe-runner";
const RECIPE_RUNNER_BRANCH: &str = "main";

fn recipe_runner_state_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("recipe_runner.json"))
}

/// Install or upgrade `recipe-runner-rs` if the currently installed commit
/// differs from upstream HEAD.
///
/// Called best-effort from the launcher bootstrap. Any failure is logged
/// and swallowed — a missing or stale recipe runner doesn't block launching
/// Claude/Copilot/Codex, it just means recipe execution will show the
/// existing "not installed" notice.
pub fn ensure_recipe_runner_up_to_date() {
    if skip_freshness_checks() {
        return;
    }
    if let Err(err) = ensure_recipe_runner_up_to_date_inner() {
        tracing::warn!(%err, "recipe-runner freshness check failed");
    }
}

fn ensure_recipe_runner_up_to_date_inner() -> Result<()> {
    let state_path = recipe_runner_state_path()?;
    let mut state = FreshnessState::read(&state_path);

    let binary_present = recipe_runner_binary_present();

    // Fast path: binary present and cooldown hasn't expired. Nothing to do.
    if binary_present && state.is_in_cooldown() {
        return Ok(());
    }

    // Slow path: consult upstream. Network failures here are survivable —
    // we'd rather launch with a stale recipe runner than block the user.
    let remote_sha = match fetch_branch_head_sha(RECIPE_RUNNER_REPO, RECIPE_RUNNER_BRANCH) {
        Ok(sha) => sha,
        Err(err) => {
            tracing::warn!(%err, "could not fetch upstream HEAD for {RECIPE_RUNNER_REPO}");
            // Record the attempt so the cooldown suppresses repeated tries.
            state.checked_at = now_secs();
            let _ = state.write(&state_path);
            return Ok(());
        }
    };

    let needs_install = !binary_present || state.installed_sha != remote_sha;
    if !needs_install {
        state.checked_at = now_secs();
        let _ = state.write(&state_path);
        return Ok(());
    }

    if !binary_present {
        eprintln!("📦 Installing recipe-runner-rs from {RECIPE_RUNNER_GIT_URL} ...");
    } else {
        eprintln!(
            "📦 Upgrading recipe-runner-rs: {} → {}",
            short_sha(&state.installed_sha),
            short_sha(&remote_sha)
        );
    }

    if let Err(err) = install_recipe_runner_from_git() {
        eprintln!("⚠️  recipe-runner-rs install failed: {err}");
        // Still record checked_at so we don't re-try on every launch when
        // the user is offline or cargo is misconfigured.
        state.checked_at = now_secs();
        let _ = state.write(&state_path);
        return Ok(());
    }

    state.installed_sha = remote_sha;
    state.checked_at = now_secs();
    state.write(&state_path)?;
    Ok(())
}

pub(crate) fn recipe_runner_binary_present() -> bool {
    // Mirrors `commands::recipe::run::binary::find_recipe_runner_binary`
    // without pulling that private helper into this module.
    if let Ok(path) = std::env::var("RECIPE_RUNNER_RS_PATH")
        && !path.is_empty()
        && Path::new(&path).is_file()
    {
        return true;
    }
    let bin_name = "recipe-runner-rs";
    let home_candidates = home_dir().ok().into_iter().flat_map(|home| {
        [
            home.join(".cargo/bin").join(bin_name),
            home.join(".local/bin").join(bin_name),
        ]
    });
    for candidate in home_candidates {
        if candidate.is_file() {
            return true;
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            if dir.join(bin_name).is_file() {
                return true;
            }
        }
    }
    false
}

pub(crate) fn install_recipe_runner_from_git() -> Result<()> {
    let cargo = which_binary("cargo").context(
        "cargo is required to install recipe-runner-rs. Install Rust: https://rustup.rs/",
    )?;
    let mut cmd = Command::new(cargo);
    cmd.arg("install")
        .arg("--git")
        .arg(RECIPE_RUNNER_GIT_URL)
        .arg("--branch")
        .arg(RECIPE_RUNNER_BRANCH)
        .arg("--locked")
        .arg("--force");
    let status = run_with_timeout(cmd, CARGO_INSTALL_TIMEOUT)
        .context("failed to run cargo install for recipe-runner-rs")?;
    if !status.success() {
        bail!("cargo install exited with status {}", status);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Framework (rysweet/amplihack) — DEPRECATED (issue #254)
// ---------------------------------------------------------------------------
//
// Framework assets are now bundled in the amplihack-rs source tree and
// delivered via binary updates.  The upstream freshness check against
// `rysweet/amplihack` is no longer performed.  The public functions below
// are kept as no-ops so that callers in `commands::install` continue to
// compile without changes during the transition period.

/// No-op.  Upstream SHA tracking is no longer used (issue #254).
pub fn record_framework_installed_sha(_sha: &str) {}

/// Always returns `None`.  Upstream SHA fetching is no longer used (#254).
pub fn current_framework_remote_sha() -> Option<String> {
    None
}

/// Always returns `false`.  Framework freshness is now tied to the
/// amplihack-rs binary version, not upstream rysweet/amplihack commits.
pub fn framework_needs_refresh() -> bool {
    false
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

fn which_binary(tool: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let candidate = dir.join(tool);
            if candidate.is_file() {
                Some(candidate)
            } else {
                None
            }
        })
    })
}

fn short_sha(sha: &str) -> String {
    if sha.is_empty() {
        "(none)".to_string()
    } else {
        sha.chars().take(7).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_roundtrips_through_json() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("freshness.json");
        let state = FreshnessState {
            installed_sha: "abc".repeat(14),
            checked_at: 1_700_000_000,
        };
        state.write(&path).unwrap();
        let parsed = FreshnessState::read(&path);
        assert_eq!(parsed.installed_sha, state.installed_sha);
        assert_eq!(parsed.checked_at, state.checked_at);
    }

    #[test]
    fn state_read_missing_returns_default() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("absent.json");
        let parsed = FreshnessState::read(&path);
        assert!(parsed.installed_sha.is_empty());
        assert_eq!(parsed.checked_at, 0);
    }

    #[test]
    fn cooldown_gates_on_age() {
        let mut state = FreshnessState {
            checked_at: now_secs(),
            ..Default::default()
        };
        assert!(state.is_in_cooldown());
        state.checked_at = 0;
        assert!(!state.is_in_cooldown());
    }

    #[test]
    fn short_sha_handles_edge_cases() {
        assert_eq!(short_sha(""), "(none)");
        assert_eq!(short_sha("abcdef0123456789"), "abcdef0");
    }
}
