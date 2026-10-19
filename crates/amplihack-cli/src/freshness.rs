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

use crate::runner_ancestry::{
    AncestrySource, FORCE_ENV, RunnerOrder, classify, downgrade_forced, is_sha, skip_message,
};
use crate::util::is_noninteractive;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// `Diverged` and `Unverifiable` skips are retried after this long.
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
    /// Set when an install was skipped because the installed runner is not
    /// older than the pin (issue #1593). An unreadable record counts as absent.
    #[serde(default, deserialize_with = "lenient_skip")]
    downgrade_skip: Option<DowngradeSkip>,
}

/// Why an installer kept the installed runner instead of installing the pin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DowngradeSkip {
    /// Revision recorded in the Cargo receipt.
    pub(crate) installed: String,
    /// Pin that was not installed.
    pub(crate) pin: String,
    pub(crate) class: RunnerOrder,
    /// UNIX timestamp of the decision.
    pub(crate) at: u64,
}

fn lenient_skip<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<DowngradeSkip>, D::Error> {
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| serde_json::from_value(value).ok()))
}

/// What [`install_recipe_runner_from_git_with`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InstallResult {
    /// The pinned revision is installed and validated.
    Installed,
    /// The installed runner was kept; see [`DowngradeSkip`].
    Kept(DowngradeSkip),
}

/// The runner revision an installer reconciles to, and where ancestry is
/// looked up. Production code uses [`RunnerTarget::pinned`]; tests inject a
/// fixture because the pin is fixed at compile time.
#[derive(Debug, Clone)]
pub(crate) struct RunnerTarget {
    pub(crate) rev: String,
    pub(crate) url: String,
    pub(crate) ancestry: AncestrySource,
}

impl RunnerTarget {
    /// The bundled pin, with Cargo's git databases and the private cache
    /// `~/.amplihack/state/recipe-runner.git` as the ancestry source.
    pub(crate) fn pinned() -> Result<Self> {
        Ok(Self {
            rev: RECIPE_RUNNER_REV.trim().to_string(),
            url: RECIPE_RUNNER_GIT_URL.to_string(),
            ancestry: AncestrySource::Default {
                cargo_home: managed_cargo_home()?,
                cache: state_dir()?.join("recipe-runner.git"),
                url: RECIPE_RUNNER_GIT_URL.to_string(),
            },
        })
    }
}

impl FreshnessState {
    fn read(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    /// Written to a temporary file and renamed, so a reader never sees a
    /// partial record.
    fn write(&self, path: &Path) -> Result<()> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        create_state_dir(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
        let body = serde_json::to_string_pretty(self)? + "\n";
        let mut temp = tempfile::NamedTempFile::new_in(parent)
            .with_context(|| format!("failed to write {}", path.display()))?;
        std::io::Write::write_all(&mut temp, body.as_bytes())
            .with_context(|| format!("failed to write {}", path.display()))?;
        temp.persist(path)
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(())
    }

    #[cfg(test)]
    fn is_in_cooldown(&self) -> bool {
        let age = now_secs().saturating_sub(self.checked_at);
        age < COOLDOWN_SECS
    }
}

#[cfg(unix)]
fn create_state_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
}

#[cfg(not(unix))]
fn create_state_dir(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)
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

/// Reconcile managed `recipe-runner-rs` delivery with the shared immutable
/// `claude-plugin/recipe-runner.rev` used by Rust and shell installation.
/// Compatible user-owned overrides are validated separately and preserved.
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
    ensure_recipe_runner_up_to_date_inner_with(&RunnerTarget::pinned()?)
}

fn ensure_recipe_runner_up_to_date_inner_with(target: &RunnerTarget) -> Result<()> {
    let state_path = recipe_runner_state_path()?;
    let state = FreshnessState::read(&state_path);

    // Explicit/custom compatible runners are never replaced. A trusted managed
    // install record, independently of capability/cooldown, owns pin reconciliation.
    if std::env::var_os("RECIPE_RUNNER_RS_PATH").is_some_and(|v| !v.is_empty()) {
        return probe_recipe_runner();
    }
    let compatible = probe_recipe_runner();
    let managed = state_path.exists();
    if compatible.is_ok()
        && (!managed
            || (state.installed_sha == target.rev
                && verify_managed_runner_at(&target.url, &target.rev).is_ok()))
    {
        return Ok(());
    }
    // A runner kept on an earlier launch stays kept while the pin and the
    // receipt are unchanged, without any git work.
    if skip_is_current(&state, target) {
        if let Err(err) = compatible {
            tracing::warn!(
                "kept recipe-runner-rs fails the capability probe: {err:#}; set {FORCE_ENV}=1 to replace it"
            );
        }
        return Ok(());
    }
    match install_recipe_runner_from_git_with(target, false)? {
        InstallResult::Installed => probe_recipe_runner(),
        InstallResult::Kept(skip) => {
            tracing::warn!("{}", skip_message(&skip.installed, &skip.pin, skip.class));
            Ok(())
        }
    }
}

pub(crate) fn managed_runner_needs_reconcile_with(target: &RunnerTarget) -> bool {
    let Ok(path) = recipe_runner_state_path() else {
        return false;
    };
    if !path.exists() {
        return false;
    }
    let state = FreshnessState::read(&path);
    if skip_is_current(&state, target) {
        return false;
    }
    state.installed_sha != target.rev || verify_managed_runner_at(&target.url, &target.rev).is_err()
}

/// Whether `state` holds a skip record that still describes this machine: a
/// valid record for the current pin and the revision in the current receipt,
/// whose class is `Newer`, or `Diverged`/`Unverifiable` within the cooldown.
/// Forcing a downgrade makes every record stale.
fn skip_is_current(state: &FreshnessState, target: &RunnerTarget) -> bool {
    if downgrade_forced() {
        return false;
    }
    let Some(skip) = &state.downgrade_skip else {
        return false;
    };
    if !is_sha(&skip.installed)
        || !is_sha(&skip.pin)
        || skip.pin != target.rev
        || state.installed_sha != skip.installed
    {
        return false;
    }
    let Ok(cargo_home) = managed_cargo_home() else {
        return false;
    };
    if installed_runner_revision(&cargo_home, &target.url).as_deref() != Some(&skip.installed) {
        return false;
    }
    match skip.class {
        RunnerOrder::Newer => true,
        RunnerOrder::Diverged | RunnerOrder::Unverifiable => {
            now_secs().saturating_sub(skip.at) < COOLDOWN_SECS
        }
        _ => false,
    }
}

pub(crate) fn recipe_runner_binary_present() -> bool {
    probe_recipe_runner().is_ok()
}

/// Compatibility is a side-effect-free producer contract, not version equality.
pub(crate) fn probe_recipe_runner() -> Result<()> {
    if !codex_selected() {
        return crate::rust_toolchain::find_recipe_runner()
            .context("recipe-runner-rs not found")
            .map(|_| ());
    }
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
                "RECIPE_RUNNER_RS_PATH does not select a regular recipe-runner-rs executable; fix or unset the override"
            );
        }
    }
    let path = crate::rust_toolchain::find_recipe_runner().context("recipe-runner-rs not found")?;
    crate::runner_validation::probe(&path, "codex")
        .context("recipe-runner-rs requires a valid capability report with codex_exec; fix the override or install the pinned runner")?;
    Ok(())
}

fn managed_cargo_home() -> Result<PathBuf> {
    crate::rust_toolchain::cargo_home().context("HOME required")
}

/// The revision in the Cargo receipt, counted only while the managed binary
/// it describes exists.
fn installed_runner_revision(cargo_home: &Path, url: &str) -> Option<String> {
    if !cargo_home.join("bin").join("recipe-runner-rs").is_file() {
        return None;
    }
    crate::runner_validation::installed_revision(cargo_home, url)
}

#[cfg(test)]
fn verify_managed_runner() -> Result<()> {
    verify_managed_runner_at(RECIPE_RUNNER_GIT_URL, RECIPE_RUNNER_REV.trim())
}

fn verify_managed_runner_at(url: &str, rev: &str) -> Result<()> {
    let cargo_home = managed_cargo_home()?;
    let selected =
        crate::rust_toolchain::find_recipe_runner().context("installed runner missing")?;
    crate::runner_validation::provenance(&selected, &cargo_home, url, rev).context(
        "managed runner receipt or selected executable does not establish requested revision",
    )?;
    crate::runner_validation::probe(&selected, if codex_selected() { "codex" } else { "claude" })
        .context("managed runner capability contract invalid")?;
    Ok(())
}

/// `cargo install` the pinned recipe-runner-rs.
#[cfg(test)]
pub(crate) fn install_recipe_runner_from_git(bootstrap_toolchain: bool) -> Result<()> {
    install_recipe_runner_from_git_with(&RunnerTarget::pinned()?, bootstrap_toolchain).map(|_| ())
}

/// `cargo install` recipe-runner-rs at `target.rev`, unless the installed
/// runner is newer than, diverged from, or not comparable with it (issue
/// #1593). A kept runner is recorded in the state file and returned as
/// [`InstallResult::Kept`]; [`FORCE_ENV`]`=1` installs regardless. With
/// `bootstrap_toolchain`, a missing Rust toolchain / C linker is installed
/// first (see [`crate::rust_toolchain`]).
pub(crate) fn install_recipe_runner_from_git_with(
    target: &RunnerTarget,
    bootstrap_toolchain: bool,
) -> Result<InstallResult> {
    let state_path = recipe_runner_state_path()?;
    let forced = downgrade_forced();
    let cargo_home = managed_cargo_home()?;
    let installed = installed_runner_revision(&cargo_home, &target.url);
    let class = classify(installed.as_deref(), &target.rev, &target.ancestry);
    match (class, installed) {
        (class, Some(installed)) if class.keeps_installed() => {
            if !forced {
                return Ok(InstallResult::Kept(keep_installed(
                    &state_path,
                    target,
                    installed,
                    class,
                )));
            }
            let message = format!(
                "downgrade forced: replacing recipe-runner-rs {installed} ({class:?}) with pinned {} because {FORCE_ENV}=1",
                target.rev
            );
            tracing::warn!("{message}");
            eprintln!("   ⚠️  {message}");
        }
        (RunnerOrder::Equal, _)
            if !forced && verify_managed_runner_at(&target.url, &target.rev).is_ok() =>
        {
            record_pin(&state_path, &target.rev)?;
            return Ok(InstallResult::Installed);
        }
        _ => {}
    }

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
        .arg(&target.url)
        .arg("--rev")
        .arg(&target.rev)
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
    verify_managed_runner_at(&target.url, &target.rev)?;
    probe_recipe_runner()?;
    record_pin(&state_path, &target.rev)?;
    Ok(InstallResult::Installed)
}

/// Record `rev` as installed, clearing any earlier skip record.
fn record_pin(state_path: &Path, rev: &str) -> Result<()> {
    FreshnessState {
        installed_sha: rev.to_string(),
        checked_at: now_secs(),
        downgrade_skip: None,
    }
    .write(state_path)
}

/// Record that `installed` was kept and validate it. Neither a failed write
/// nor a failed validation replaces the runner; both are logged.
fn keep_installed(
    state_path: &Path,
    target: &RunnerTarget,
    installed: String,
    class: RunnerOrder,
) -> DowngradeSkip {
    let now = now_secs();
    let skip = DowngradeSkip {
        installed: installed.clone(),
        pin: target.rev.clone(),
        class,
        at: now,
    };
    let state = FreshnessState {
        installed_sha: installed.clone(),
        checked_at: now,
        downgrade_skip: Some(skip.clone()),
    };
    if let Err(err) = state.write(state_path) {
        tracing::warn!("could not record the kept recipe-runner-rs: {err:#}");
    }
    if let Err(err) = verify_managed_runner_at(&target.url, &installed) {
        tracing::warn!(
            "kept recipe-runner-rs {installed} is incompatible: {err:#}; set {FORCE_ENV}=1 to replace it"
        );
    }
    skip
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
            ..Default::default()
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
    use std::os::unix::fs::PermissionsExt;

    fn worker(test: &str) -> bool {
        if std::env::var("AMPLIHACK_DELIVERY_WORKER").as_deref() != Ok(test) {
            return false;
        }
        match test {
            "managed_receipt_and_selected_executable_must_agree" => {
                assert!(verify_managed_runner().is_err());
                let key = format!(
                    "recipe-runner-rs 0.4.0 (git+{}#{})",
                    RECIPE_RUNNER_GIT_URL,
                    RECIPE_RUNNER_REV.trim()
                );
                fs::write(
                    PathBuf::from(std::env::var_os("CARGO_HOME").unwrap()).join(".crates2.json"),
                    serde_json::to_vec(
                        &serde_json::json!({"installs":{key:{"bins":["recipe-runner-rs"]}}}),
                    )
                    .unwrap(),
                )
                .unwrap();
                with_provider("claude", verify_managed_runner).unwrap();
                with_provider("codex", || {
                    assert!(
                        probe_recipe_runner().is_err(),
                        "receipt cannot establish capability"
                    )
                });
            }
            "codex_runner_provisioning_uses_authoritative_immutable_revision" => {
                assert!(
                    install_recipe_runner_from_git(false).is_err(),
                    "cargo success without managed executable is insufficient"
                );
                assert_pin(&fs::read_to_string(std::env::var_os("DELIVERY_LOG").unwrap()).unwrap());
            }
            "codex_managed_runner_reconciles_pin_even_inside_cooldown_when_capability_passes" => {
                FreshnessState {
                    installed_sha: "0".repeat(40),
                    checked_at: now_secs(),
                    ..Default::default()
                }
                .write(&recipe_runner_state_path().unwrap())
                .unwrap();
                assert!(
                    ensure_recipe_runner_up_to_date_inner().is_err(),
                    "PATH shadow cannot establish managed provenance"
                );
                assert_pin(&fs::read_to_string(std::env::var_os("DELIVERY_LOG").unwrap()).unwrap());
                assert_eq!(
                    FreshnessState::read(&recipe_runner_state_path().unwrap()).installed_sha,
                    "0".repeat(40)
                );
            }
            _ => panic!("unknown worker"),
        }
        println!("MANAGED_DELIVERY_WORKER_COMPLETE:{test}");
        true
    }
    fn assert_pin(args: &str) {
        let args: Vec<_> = args.lines().collect();
        let pin = RECIPE_RUNNER_REV.trim();
        assert_eq!(pin.len(), 40);
        assert!(args.windows(2).any(|p| p == ["--rev", pin]), "{args:?}");
        assert!(args.contains(&"--locked"));
        assert!(!args.contains(&"--branch"));
    }
    fn run_worker(test: &str, home: &Path, bin: &Path) {
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!("freshness::codex_delivery_contract_tests::{test}"),
                "--nocapture",
            ])
            .env("AMPLIHACK_DELIVERY_WORKER", test)
            .env("PATH", bin)
            .env("HOME", home)
            .env("CARGO_HOME", home)
            .env("DELIVERY_LOG", home.join("cargo-args"))
            .env("AMPLIHACK_AGENT_BINARY", "codex")
            .env_remove("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL")
            .env_remove("AMPLIHACK_SKIP_AUTO_INSTALL")
            .env_remove("RECIPE_RUNNER_RS_PATH")
            .env_remove("AMPLIHACK_NO_FRESHNESS_CHECK")
            .env_remove("CI")
            .output()
            .unwrap();
        let transcript = String::from_utf8_lossy(&result.stdout);
        assert!(
            result.status.success(),
            "isolated worker failed: {test}: {transcript}"
        );
        assert!(transcript.contains("running 1 test"), "{transcript}");
        assert!(
            transcript.contains(&format!("MANAGED_DELIVERY_WORKER_COMPLETE:{test}")),
            "{transcript}"
        );
        assert!(transcript.contains("1 passed; 0 failed"), "{transcript}");
    }
    fn executable(path: &Path, script: &str) {
        fs::write(path, script).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fn cargo_fixture(home: &Path, bin: &Path) {
        executable(
            &bin.join("cargo"),
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n",
                home.join("cargo-args").display()
            ),
        );
        executable(&bin.join("cc"), "#!/bin/sh\nexit 0\n");
    }
    #[test]
    fn managed_receipt_and_selected_executable_must_agree() {
        let test = "managed_receipt_and_selected_executable_must_agree";
        if worker(test) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        fs::create_dir(&bin).unwrap();
        executable(
            &bin.join("recipe-runner-rs"),
            "#!/bin/sh\nprintf '%s\\n' '{\"schema_version\":1,\"version\":\"fixture\",\"capabilities\":[]}'\n",
        );
        run_worker(test, dir.path(), &bin);
    }
    #[test]
    fn codex_runner_provisioning_uses_authoritative_immutable_revision() {
        let test = "codex_runner_provisioning_uses_authoritative_immutable_revision";
        if worker(test) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        cargo_fixture(dir.path(), dir.path());
        run_worker(test, dir.path(), dir.path());
    }
    #[test]
    fn codex_managed_runner_reconciles_pin_even_inside_cooldown_when_capability_passes() {
        let test =
            "codex_managed_runner_reconciles_pin_even_inside_cooldown_when_capability_passes";
        if worker(test) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        cargo_fixture(dir.path(), dir.path());
        executable(
            &dir.path().join("recipe-runner-rs"),
            "#!/bin/sh\nprintf '%s\\n' '{\"schema_version\":1,\"version\":\"custom\",\"capabilities\":[\"codex_exec\"]}'\n",
        );
        run_worker(test, dir.path(), dir.path());
    }

    /// Run only in an isolated caller-provided HOME/CARGO_HOME with real Cargo.
    #[test]
    #[ignore = "builds the published runner; caller must isolate HOME and CARGO_HOME"]
    fn real_managed_delivery() {
        install_recipe_runner_from_git(false).unwrap();
        with_provider("codex", verify_managed_runner).unwrap();
        with_provider("codex", probe_recipe_runner).unwrap();
    }
}

/// Installer downgrade guard (issue #1593, AC7). Each test re-runs itself as an
/// isolated worker process with a fixture HOME and CARGO_HOME, a stub cargo
/// that logs its arguments, and a fixture runner repository injected through
/// `AncestrySource::Fixed`. The pin is always fixture commit B; an installed
/// C is Newer, A is Older and D is Diverged.
#[cfg(all(test, unix))]
mod downgrade_guard_tests {
    use super::*;
    use crate::runner_ancestry::{AncestrySource, FORCE_ENV, fixture};
    use serde_json::Value;
    use std::os::unix::fs::PermissionsExt;

    const WORKER_ENV: &str = "AMPLIHACK_DOWNGRADE_GUARD_WORKER";
    const MISSING: &str = "0123456789abcdef0123456789abcdef01234567";

    struct Shas {
        a: String,
        b: String,
        c: String,
        d: String,
    }

    fn shas() -> Shas {
        let get = |k: &str| std::env::var(k).unwrap();
        Shas {
            a: get("FIX_A"),
            b: get("FIX_B"),
            c: get("FIX_C"),
            d: get("FIX_D"),
        }
    }

    fn target() -> RunnerTarget {
        RunnerTarget {
            rev: std::env::var("FIX_B").unwrap(),
            url: RECIPE_RUNNER_GIT_URL.to_string(),
            ancestry: AncestrySource::Fixed(PathBuf::from(std::env::var("FIX_PATH").unwrap())),
        }
    }

    fn cargo_home() -> PathBuf {
        PathBuf::from(std::env::var_os("CARGO_HOME").unwrap())
    }

    fn write_receipt(sha: &str) {
        let key = format!("recipe-runner-rs 0.5.0 (git+{RECIPE_RUNNER_GIT_URL}#{sha})");
        fs::write(
            cargo_home().join(".crates2.json"),
            serde_json::to_vec(
                &serde_json::json!({"installs":{key:{"bins":["recipe-runner-rs"]}}}),
            )
            .unwrap(),
        )
        .unwrap();
    }

    fn cargo_log() -> String {
        fs::read_to_string(std::env::var_os("DELIVERY_LOG").unwrap()).unwrap_or_default()
    }

    fn assert_cargo_not_called(context: &str) {
        let log = cargo_log();
        assert!(log.is_empty(), "{context}: cargo was invoked: {log}");
    }

    fn assert_cargo_installed_pin(context: &str) {
        let log = cargo_log();
        let args: Vec<_> = log.lines().collect();
        let pin = std::env::var("FIX_B").unwrap();
        assert!(
            args.windows(2).any(|p| p == ["--rev", pin.as_str()]),
            "{context}: {args:?}"
        );
        assert!(args.contains(&"--locked"), "{context}: {args:?}");
    }

    fn state() -> Value {
        let raw = fs::read_to_string(recipe_runner_state_path().unwrap()).unwrap();
        serde_json::from_str(&raw).unwrap()
    }

    fn write_state(installed_sha: &str, checked_at: u64, skip: Option<Value>) {
        let mut body = serde_json::json!({
            "installed_sha": installed_sha,
            "checked_at": checked_at,
        });
        if let Some(skip) = skip {
            body["downgrade_skip"] = skip;
        }
        let path = recipe_runner_state_path().unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec_pretty(&body).unwrap()).unwrap();
    }

    fn skip_record(installed: &str, pin: &str, class: &str, at: u64) -> Value {
        serde_json::json!({"installed": installed, "pin": pin, "class": class, "at": at})
    }

    fn reset_cargo_log() {
        let _ = fs::remove_file(std::env::var_os("DELIVERY_LOG").unwrap());
    }

    fn worker(test: &str) -> bool {
        if std::env::var(WORKER_ENV).as_deref() != Ok(test) {
            return false;
        }
        let s = shas();
        let t = target();
        match test {
            "newer_installed_runner_is_kept_and_recorded" => {
                write_receipt(&s.c);
                write_state(&s.c, 0, None);
                ensure_recipe_runner_up_to_date_inner_with(&t).unwrap();
                assert_cargo_not_called("first launch");
                let st = state();
                assert_eq!(st["installed_sha"], s.c.as_str(), "real SHA, never the pin");
                let skip = &st["downgrade_skip"];
                assert_eq!(skip["installed"], s.c.as_str(), "{st}");
                assert_eq!(skip["pin"], s.b.as_str(), "{st}");
                assert_eq!(skip["class"], "Newer", "{st}");
                let at = skip["at"].as_u64().expect("at is a timestamp");
                assert!(at > 0, "{st}");
                assert!(
                    !managed_runner_needs_reconcile_with(&t),
                    "a current Newer record needs no reconcile"
                );

                // Second launch: the record answers, so no ancestry lookup.
                // With the fixture gone a fresh lookup would be Unverifiable.
                fs::remove_dir_all(std::env::var("FIX_PATH").unwrap()).unwrap();
                ensure_recipe_runner_up_to_date_inner_with(&t).unwrap();
                assert_cargo_not_called("second launch");
                let st = state();
                assert_eq!(st["downgrade_skip"]["class"], "Newer", "{st}");
                assert_eq!(st["downgrade_skip"]["at"].as_u64(), Some(at), "{st}");
                assert_eq!(st["installed_sha"], s.c.as_str());
            }
            "install_from_git_reports_kept_for_a_newer_runner" => {
                write_receipt(&s.c);
                match install_recipe_runner_from_git_with(&t, false).unwrap() {
                    InstallResult::Kept(skip) => {
                        let skip = serde_json::to_value(&skip).unwrap();
                        assert_eq!(skip["installed"], s.c.as_str(), "{skip}");
                        assert_eq!(skip["pin"], s.b.as_str(), "{skip}");
                        assert_eq!(skip["class"], "Newer", "{skip}");
                    }
                    InstallResult::Installed => panic!("a newer runner must be kept"),
                }
                assert_cargo_not_called("install_recipe_runner_from_git_with");
            }
            "older_installed_runner_is_upgraded" => {
                write_receipt(&s.a);
                write_state(&s.a, 0, None);
                ensure_recipe_runner_up_to_date_inner_with(&t).unwrap();
                assert_cargo_installed_pin("Older");
                let st = state();
                assert_eq!(st["installed_sha"], s.b.as_str(), "{st}");
                assert!(st["downgrade_skip"].is_null(), "{st}");
            }
            "unknown_installed_runner_is_installed" => {
                // No receipt: the D1 rule says Unknown, which installs.
                write_state(&"0".repeat(40), now_secs(), None);
                let _ = fs::remove_file(cargo_home().join(".crates2.json"));
                let _ = ensure_recipe_runner_up_to_date_inner_with(&t);
                assert_cargo_installed_pin("Unknown");
            }
            "diverged_and_unverifiable_are_kept_and_retried_after_cooldown" => {
                for (installed, class) in [(s.d.as_str(), "Diverged"), (MISSING, "Unverifiable")] {
                    reset_cargo_log();
                    write_receipt(installed);
                    write_state(installed, 0, None);
                    ensure_recipe_runner_up_to_date_inner_with(&t).unwrap();
                    assert_cargo_not_called(class);
                    let st = state();
                    assert_eq!(st["downgrade_skip"]["class"], class, "{st}");
                    assert_eq!(st["installed_sha"], installed, "{st}");
                    assert!(
                        !managed_runner_needs_reconcile_with(&t),
                        "{class}: a fresh record suppresses retries within the cooldown"
                    );
                    write_state(installed, 0, Some(skip_record(installed, &s.b, class, 1)));
                    assert!(
                        managed_runner_needs_reconcile_with(&t),
                        "{class}: an expired record is retried"
                    );
                }
            }
            "forced_downgrade_installs_and_clears_the_record" => {
                assert_eq!(std::env::var(FORCE_ENV).as_deref(), Ok("1"));
                write_receipt(&s.c);
                write_state(
                    &s.c,
                    now_secs(),
                    Some(skip_record(&s.c, &s.b, "Newer", now_secs())),
                );
                assert!(
                    managed_runner_needs_reconcile_with(&t),
                    "force overrides a current skip record"
                );
                ensure_recipe_runner_up_to_date_inner_with(&t).unwrap();
                assert_cargo_installed_pin("forced");
                let st = state();
                assert_eq!(st["installed_sha"], s.b.as_str(), "{st}");
                assert!(st["downgrade_skip"].is_null(), "record cleared: {st}");
            }
            "invalid_or_stale_records_are_ignored" => {
                let cases = [
                    (
                        "bad installed SHA",
                        skip_record("zz", &s.b, "Newer", now_secs()),
                    ),
                    (
                        "uppercase installed SHA",
                        skip_record(&s.c.to_uppercase(), &s.b, "Newer", now_secs()),
                    ),
                    (
                        "bad pin SHA",
                        skip_record(&s.c, "not-a-sha", "Newer", now_secs()),
                    ),
                    (
                        "unknown class",
                        skip_record(&s.c, &s.b, "Bogus", now_secs()),
                    ),
                    (
                        "Older is never a skip",
                        skip_record(&s.c, &s.b, "Older", now_secs()),
                    ),
                    ("stale pin", skip_record(&s.c, &s.a, "Newer", now_secs())),
                    (
                        "stale receipt",
                        skip_record(&s.a, &s.b, "Newer", now_secs()),
                    ),
                ];
                for (label, record) in cases {
                    reset_cargo_log();
                    write_receipt(&s.c);
                    write_state(&s.c, now_secs(), Some(record));
                    assert!(
                        managed_runner_needs_reconcile_with(&t),
                        "{label}: an invalid record must not suppress the guard"
                    );
                    ensure_recipe_runner_up_to_date_inner_with(&t).unwrap();
                    assert_cargo_not_called(label);
                    let st = state();
                    let skip = &st["downgrade_skip"];
                    assert_eq!(skip["installed"], s.c.as_str(), "{label}: {st}");
                    assert_eq!(skip["pin"], s.b.as_str(), "{label}: {st}");
                    assert_eq!(skip["class"], "Newer", "{label}: {st}");
                }
            }
            "a_kept_runner_failing_the_probe_is_still_kept" => {
                // The stub runner in this worker lacks codex_exec.
                write_receipt(&s.c);
                write_state(&s.c, 0, None);
                let _ = ensure_recipe_runner_up_to_date_inner_with(&t);
                assert_cargo_not_called("incompatible Newer runner");
                assert_eq!(state()["downgrade_skip"]["class"], "Newer");
            }
            "path_override_is_never_replaced" => {
                write_receipt(&s.a);
                write_state(&s.a, 0, None);
                ensure_recipe_runner_up_to_date_inner_with(&t).unwrap();
                assert_cargo_not_called("RECIPE_RUNNER_RS_PATH override");
            }
            "pinned_target_uses_the_bundled_pin_and_private_cache" => {
                let pinned = RunnerTarget::pinned().unwrap();
                assert_eq!(pinned.rev, RECIPE_RUNNER_REV.trim());
                assert_eq!(pinned.url, RECIPE_RUNNER_GIT_URL);
                match pinned.ancestry {
                    AncestrySource::Default {
                        cargo_home: home,
                        cache,
                        url,
                    } => {
                        assert_eq!(home, cargo_home());
                        assert_eq!(
                            cache,
                            PathBuf::from(std::env::var_os("HOME").unwrap())
                                .join(".amplihack/state/recipe-runner.git")
                        );
                        assert_eq!(url, RECIPE_RUNNER_GIT_URL);
                    }
                    AncestrySource::Fixed(_) => panic!("production target must not be Fixed"),
                }
            }
            _ => panic!("unknown worker {test}"),
        }
        println!("DOWNGRADE_GUARD_WORKER_COMPLETE:{test}");
        true
    }

    fn executable(path: &Path, script: &str) {
        fs::write(path, script).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    const COMPATIBLE: &str = "#!/bin/sh\nprintf '%s\\n' '{\"schema_version\":1,\"version\":\"fixture\",\"capabilities\":[\"codex_exec\"]}'\n";
    const INCOMPATIBLE: &str = "#!/bin/sh\nprintf '%s\\n' '{\"schema_version\":1,\"version\":\"fixture\",\"capabilities\":[]}'\n";

    /// Runs `test` in a child process. `runner` is the stub installed in
    /// `$CARGO_HOME/bin`; `extra` adds environment variables.
    fn run_worker(test: &str, runner: &str, extra: &[(&str, &str)]) {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let cargo = home.join(".cargo");
        let bin = dir.path().join("bin");
        fs::create_dir_all(cargo.join("bin")).unwrap();
        fs::create_dir_all(&bin).unwrap();
        let fix = fixture::build(dir.path());
        executable(&cargo.join("bin/recipe-runner-rs"), runner);
        // Builtins only: the stub logs its arguments, then records the
        // requested revision in the receipt as a real install would.
        executable(
            &bin.join("cargo"),
            &format!(
                "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done >> \"$DELIVERY_LOG\"\n\
                 rev=''; prev=''\nfor a in \"$@\"; do [ \"$prev\" = --rev ] && rev=$a; prev=$a; done\n\
                 printf '{{\"installs\":{{\"recipe-runner-rs 0.5.0 (git+{RECIPE_RUNNER_GIT_URL}#%s)\":{{\"bins\":[\"recipe-runner-rs\"]}}}}}}' \"$rev\" > \"$CARGO_HOME/.crates2.json\"\n"
            ),
        );
        executable(&bin.join("cc"), "#!/bin/sh\nexit 0\n");
        let path = format!("{}:/usr/bin:/bin", bin.display());
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                &format!("freshness::downgrade_guard_tests::{test}"),
                "--nocapture",
            ])
            .env(WORKER_ENV, test)
            .env("PATH", path)
            .env("HOME", &home)
            .env("CARGO_HOME", &cargo)
            .env("DELIVERY_LOG", dir.path().join("cargo-args"))
            .env("AMPLIHACK_AGENT_BINARY", "codex")
            .env("FIX_PATH", &fix.path)
            .env("FIX_A", &fix.a)
            .env("FIX_B", &fix.b)
            .env("FIX_C", &fix.c)
            .env("FIX_D", &fix.d)
            .env_remove(FORCE_ENV)
            .env_remove("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL")
            .env_remove("AMPLIHACK_SKIP_AUTO_INSTALL")
            .env_remove("RECIPE_RUNNER_RS_PATH")
            .env_remove("RECIPE_RUNNER_ANCESTRY_REPO")
            .env_remove("AMPLIHACK_NO_FRESHNESS_CHECK")
            .env_remove("CI");
        for (key, value) in extra {
            command.env(key, value);
        }
        let result = command.output().unwrap();
        let transcript = String::from_utf8_lossy(&result.stdout);
        assert!(
            result.status.success(),
            "isolated worker failed: {test}: {transcript}{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            transcript.contains(&format!("DOWNGRADE_GUARD_WORKER_COMPLETE:{test}")),
            "{transcript}"
        );
        assert!(transcript.contains("1 passed; 0 failed"), "{transcript}");
    }

    #[test]
    fn newer_installed_runner_is_kept_and_recorded() {
        let test = "newer_installed_runner_is_kept_and_recorded";
        if worker(test) {
            return;
        }
        run_worker(test, COMPATIBLE, &[]);
    }

    #[test]
    fn install_from_git_reports_kept_for_a_newer_runner() {
        let test = "install_from_git_reports_kept_for_a_newer_runner";
        if worker(test) {
            return;
        }
        run_worker(test, COMPATIBLE, &[]);
    }

    #[test]
    fn older_installed_runner_is_upgraded() {
        let test = "older_installed_runner_is_upgraded";
        if worker(test) {
            return;
        }
        run_worker(test, COMPATIBLE, &[]);
    }

    #[test]
    fn unknown_installed_runner_is_installed() {
        let test = "unknown_installed_runner_is_installed";
        if worker(test) {
            return;
        }
        run_worker(test, COMPATIBLE, &[]);
    }

    #[test]
    fn diverged_and_unverifiable_are_kept_and_retried_after_cooldown() {
        let test = "diverged_and_unverifiable_are_kept_and_retried_after_cooldown";
        if worker(test) {
            return;
        }
        run_worker(test, COMPATIBLE, &[]);
    }

    #[test]
    fn forced_downgrade_installs_and_clears_the_record() {
        let test = "forced_downgrade_installs_and_clears_the_record";
        if worker(test) {
            return;
        }
        run_worker(test, COMPATIBLE, &[(FORCE_ENV, "1")]);
    }

    #[test]
    fn invalid_or_stale_records_are_ignored() {
        let test = "invalid_or_stale_records_are_ignored";
        if worker(test) {
            return;
        }
        run_worker(test, COMPATIBLE, &[]);
    }

    #[test]
    fn a_kept_runner_failing_the_probe_is_still_kept() {
        let test = "a_kept_runner_failing_the_probe_is_still_kept";
        if worker(test) {
            return;
        }
        run_worker(test, INCOMPATIBLE, &[]);
    }

    #[test]
    fn path_override_is_never_replaced() {
        let test = "path_override_is_never_replaced";
        if worker(test) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let explicit = dir.path().join("recipe-runner-rs");
        executable(&explicit, COMPATIBLE);
        run_worker(
            test,
            COMPATIBLE,
            &[("RECIPE_RUNNER_RS_PATH", explicit.to_str().unwrap())],
        );
    }

    #[test]
    fn pinned_target_uses_the_bundled_pin_and_private_cache() {
        let test = "pinned_target_uses_the_bundled_pin_and_private_cache";
        if worker(test) {
            return;
        }
        run_worker(test, COMPATIBLE, &[]);
    }

    #[test]
    fn downgrade_skip_defaults_when_absent_from_old_state_files() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("recipe_runner.json");
        fs::write(&path, r#"{"installed_sha":"abc","checked_at":5}"#).unwrap();
        let parsed = FreshnessState::read(&path);
        assert_eq!(parsed.installed_sha, "abc");
        assert_eq!(parsed.checked_at, 5);
        assert!(parsed.downgrade_skip.is_none());
    }
}
