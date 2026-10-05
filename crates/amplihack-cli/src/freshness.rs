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
//! Managed runner delivery uses the immutable revision bundled in
//! `claude-plugin/recipe-runner.rev`. Capability negotiation permits compatible
//! custom binaries; an installation record separately reconciles managed source
//! drift, independent of the former cooldown. Explicit overrides are preserved.
//! Launcher freshness remains best-effort; install failures are actionable.

use crate::util::is_noninteractive;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(test)]
const COOLDOWN_SECS: u64 = 24 * 60 * 60;
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

    #[cfg(test)]
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

const RECIPE_RUNNER_GIT_URL: &str = "https://github.com/rysweet/amplihack-recipe-runner";
pub(crate) const RECIPE_RUNNER_REV: &str = include_str!("../../../claude-plugin/recipe-runner.rev");

fn recipe_runner_state_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("recipe_runner.json"))
}

thread_local! { static SELECTED_PROVIDER: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) }; }

pub(crate) fn with_provider<T>(provider: &str, action: impl FnOnce() -> T) -> T {
    struct Restore(Option<String>);
    impl Drop for Restore {
        fn drop(&mut self) {
            SELECTED_PROVIDER.with(|p| *p.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(SELECTED_PROVIDER.with(|p| p.replace(Some(provider.to_string()))));
    action()
}
pub(crate) fn codex_selected() -> bool {
    SELECTED_PROVIDER
        .with(|p| p.borrow().clone())
        .unwrap_or_else(crate::env_builder::active_agent_binary)
        == "codex"
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

    // Explicit/custom compatible runners are never replaced. A trusted managed
    // install record, independently of capability/cooldown, owns pin reconciliation.
    if std::env::var_os("RECIPE_RUNNER_RS_PATH").is_some_and(|v| !v.is_empty()) {
        return probe_recipe_runner();
    }
    let compatible = probe_recipe_runner().is_ok();
    let managed = state_path.exists();
    if compatible
        && (!managed
            || (state.installed_sha == RECIPE_RUNNER_REV.trim() && verify_managed_runner().is_ok()))
    {
        return Ok(());
    }
    install_recipe_runner_from_git(false)?;
    probe_recipe_runner()?;
    let remote_sha = RECIPE_RUNNER_REV.trim().to_string();

    state.installed_sha = remote_sha;
    state.checked_at = now_secs();
    state.write(&state_path)?;
    Ok(())
}

pub(crate) fn managed_runner_needs_reconcile() -> bool {
    let Ok(path) = recipe_runner_state_path() else {
        return false;
    };
    let state = FreshnessState::read(&path);
    path.exists()
        && (state.installed_sha != RECIPE_RUNNER_REV.trim() || verify_managed_runner().is_err())
}

pub(crate) fn recipe_runner_binary_present() -> bool {
    probe_recipe_runner().is_ok()
}

/// Compatibility is a side-effect-free producer contract, not version equality.
pub(crate) fn probe_recipe_runner() -> Result<()> {
    if let Some(explicit) = std::env::var_os("RECIPE_RUNNER_RS_PATH").filter(|v| !v.is_empty()) {
        let explicit = PathBuf::from(explicit);
        let expanded = if let Ok(rest) = explicit.strip_prefix("~") {
            PathBuf::from(
                std::env::var_os("HOME").context("HOME required for explicit runner ~ path")?,
            )
            .join(rest)
        } else {
            explicit
        };
        if expanded.components().count() > 1 && !expanded.is_file() {
            bail!(
                "RECIPE_RUNNER_RS_PATH does not select a regular executable; fix or unset the override"
            );
        }
    }
    let path = crate::rust_toolchain::find_recipe_runner().context("recipe-runner-rs not found")?;
    if !codex_selected() {
        return Ok(());
    }
    let mut cmd = Command::new(path);
    cmd.arg("--capabilities");
    let output = crate::util::run_output_with_timeout_limited(
        cmd,
        std::time::Duration::from_secs(10),
        64 * 1024,
    )?;
    if !output.status.success() {
        bail!("recipe-runner-rs --capabilities failed");
    }
    let probe: serde_json::Value = serde_json::from_slice(&output.stdout)
        .context("recipe-runner-rs must support JSON --capabilities")?;
    if probe["schema_version"].as_u64() != Some(1)
        || probe["version"].as_str().is_none_or(str::is_empty)
        || !probe["capabilities"]
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v.as_str() == Some("codex_exec")))
    {
        bail!(
            "recipe-runner-rs requires capability schema 1 and codex_exec; install the pinned runner"
        );
    }
    Ok(())
}

fn verify_managed_runner() -> Result<()> {
    let cargo_home = std::env::var_os("CARGO_HOME").map(PathBuf::from).unwrap_or(
        PathBuf::from(std::env::var_os("HOME").context("HOME required")?).join(".cargo"),
    );
    let selected =
        crate::rust_toolchain::find_recipe_runner().context("installed runner missing")?;
    anyhow::ensure!(
        selected.canonicalize()? == cargo_home.join("bin/recipe-runner-rs").canonicalize()?,
        "managed runner is shadowed; cannot record managed installation"
    );
    let receipt = std::fs::read_to_string(cargo_home.join(".crates2.json"))
        .context("managed runner cargo receipt missing")?;
    let receipt: serde_json::Value =
        serde_json::from_str(&receipt).context("invalid cargo receipt")?;
    anyhow::ensure!(
        receipt["installs"]
            .as_object()
            .is_some_and(|entries| entries.iter().any(|(source, record)| source
                .starts_with("recipe-runner-rs ")
                && source.contains(RECIPE_RUNNER_GIT_URL)
                && source.ends_with(&format!("#{})", RECIPE_RUNNER_REV.trim()))
                && record["bins"]
                    .as_array()
                    .is_some_and(|bins| bins.iter().any(|b| b == "recipe-runner-rs")))),
        "managed runner receipt does not establish requested revision"
    );
    Ok(())
}

/// `cargo install` recipe-runner-rs. With `bootstrap_toolchain`, a missing
/// Rust toolchain / C linker is installed first (see [`crate::rust_toolchain`]).
pub(crate) fn install_recipe_runner_from_git(bootstrap_toolchain: bool) -> Result<()> {
    let cargo = crate::rust_toolchain::ensure_cargo(bootstrap_toolchain)?;
    crate::rust_toolchain::ensure_c_linker(bootstrap_toolchain)?;
    let mut cmd = Command::new(&cargo);
    // A freshly bootstrapped ~/.cargo/bin is not on PATH yet; cargo needs it
    // to find rustc and to place the installed binary where we probe.
    if let Some(path) = crate::rust_toolchain::path_with_cargo_bin(&cargo) {
        cmd.env("PATH", path);
    }
    amplihack_utils::litellm_proxy::scrub_inference_environment(&mut cmd);
    cmd.arg("install")
        .arg("--git")
        .arg(RECIPE_RUNNER_GIT_URL)
        .arg("--rev")
        .arg(RECIPE_RUNNER_REV.trim())
        .arg("--locked")
        .arg("--force");
    // No timeout: building recipe-runner-rs on a slow host takes as long as
    // it takes; a false timeout would fail a healthy install.
    let status = cmd
        .status()
        .context("failed to run cargo install for recipe-runner-rs")?;
    if !status.success() {
        bail!("cargo install exited with status {status}");
    }
    verify_managed_runner()?;
    probe_recipe_runner()?;
    FreshnessState {
        installed_sha: RECIPE_RUNNER_REV.trim().to_string(),
        checked_at: now_secs(),
    }
    .write(&recipe_runner_state_path()?)?;
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
/// amplihack-rs binary version, not legacy upstream commits.
pub fn framework_needs_refresh() -> bool {
    false
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
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

#[cfg(all(test, unix))]
mod codex_delivery_contract_tests {
    use super::*;
    use crate::test_support::{EnvGuard, home_env_lock};
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn managed_receipt_and_selected_executable_must_agree() {
        let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let runner = bin.join("recipe-runner-rs");
        fs::write(&runner, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&runner, fs::Permissions::from_mode(0o700)).unwrap();
        let _env = EnvGuard::set([
            ("PATH", bin.to_str().unwrap()),
            ("CARGO_HOME", dir.path().to_str().unwrap()),
            ("RECIPE_RUNNER_RS_PATH", ""),
        ]);
        assert!(verify_managed_runner().is_err());
        let key = format!(
            "recipe-runner-rs 0.4.0 (git+{}#{})",
            RECIPE_RUNNER_GIT_URL,
            RECIPE_RUNNER_REV.trim()
        );
        fs::write(
            dir.path().join(".crates2.json"),
            serde_json::to_vec(
                &serde_json::json!({"installs":{key:{"bins":["recipe-runner-rs"]}}}),
            )
            .unwrap(),
        )
        .unwrap();
        verify_managed_runner().unwrap();
        with_provider("codex", || {
            assert!(
                probe_recipe_runner().is_err(),
                "receipt alone cannot establish Codex capability"
            )
        });
    }

    #[test]
    fn codex_runner_provisioning_uses_authoritative_immutable_revision() {
        let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("cargo-args");
        let cargo = dir.path().join("cargo");
        std::fs::write(
            &cargo,
            format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n", log.display()),
        )
        .unwrap();
        std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o700)).unwrap();
        let cc = dir.path().join("cc");
        std::fs::write(&cc, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&cc, std::fs::Permissions::from_mode(0o700)).unwrap();
        let _env = EnvGuard::set([
            ("PATH", dir.path().to_str().unwrap()),
            ("CARGO_HOME", dir.path().to_str().unwrap()),
        ]);
        assert!(
            install_recipe_runner_from_git(false).is_err(),
            "cargo success without an installed managed binary is insufficient"
        );
        let args = std::fs::read_to_string(log).unwrap();
        let args: Vec<_> = args.lines().collect();
        let pin = include_str!("../../../claude-plugin/recipe-runner.rev").trim();
        assert_eq!(pin.len(), 40);
        assert!(
            args.windows(2).any(|pair| pair == ["--rev", pin]),
            "must consume authoritative revision: {args:?}"
        );
        assert!(args.contains(&"--locked"));
        assert!(!args.contains(&"--branch"));
    }
    #[test]
    fn codex_managed_runner_reconciles_pin_even_inside_cooldown_when_capability_passes() {
        let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("install-args");
        for (name, script) in [
            ("cargo", format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n", log.display())),
            ("cc", "#!/bin/sh\nexit 0\n".to_string()),
            ("recipe-runner-rs", "#!/bin/sh\nprintf '%s\\n' '{\"schema_version\":1,\"version\":\"custom\",\"capabilities\":[\"codex_exec\"]}'\n".to_string()),
        ] {
            let path = dir.path().join(name);
            fs::write(&path, script).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let _env = EnvGuard::set([
            ("PATH", dir.path().to_str().unwrap()),
            ("HOME", dir.path().to_str().unwrap()),
            ("CARGO_HOME", dir.path().to_str().unwrap()),
            ("RECIPE_RUNNER_RS_PATH", ""),
        ]);
        FreshnessState {
            installed_sha: "0".repeat(40),
            checked_at: now_secs(),
        }
        .write(&recipe_runner_state_path().unwrap())
        .unwrap();
        assert!(
            ensure_recipe_runner_up_to_date_inner().is_err(),
            "compatible PATH shadow cannot establish managed provenance"
        );
        let args = fs::read_to_string(log)
            .expect("managed source drift cannot hide behind cooldown or capability");
        let pin = include_str!("../../../claude-plugin/recipe-runner.rev").trim();
        assert!(
            args.lines()
                .collect::<Vec<_>>()
                .windows(2)
                .any(|pair| pair == ["--rev", pin])
        );
        assert_eq!(
            FreshnessState::read(&recipe_runner_state_path().unwrap()).installed_sha,
            "0".repeat(40)
        );
    }
}
