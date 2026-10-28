//! Native install and uninstall commands.

mod binary;
pub(crate) mod bundle_compat;
pub(crate) mod bundle_compat_cache;
mod claude_commands;
mod claude_publication;
mod clone;
mod codex_plugin;
mod command_staging;
mod copilot_plugin;
mod directories;
pub(crate) mod filesystem;
mod hooks;
pub(crate) mod interactive;
mod manifest;
mod mermaid_cli;
pub(crate) mod paths;
mod recipe_runner;
mod settings;
mod stale_wrappers;
mod types;
mod uninstall;
mod verification;
pub(crate) mod version_stamp;

#[cfg(all(test, unix))]
#[path = "tests/codex_alias_fixture.rs"]
mod codex_alias_fixture;
#[cfg(test)]
mod tests;

pub(crate) use binary::resolve_running_binary_source;
use binary::{deploy_binaries, find_hooks_binary, stable_hooks_binary};
use bundle_compat::validate_framework_bundle_compatibility;
use clone::{download_and_extract_framework_repo, find_bundled_framework_root};
use directories::*;
use filesystem::{all_rel_dirs, get_all_files_and_dirs};
use manifest::{manifest_path, write_manifest};
use paths::*;
use settings::*;
#[cfg(test)]
pub(in crate::commands) use types::SourceLayout as SourceLayoutForTest;
use types::*;
#[cfg(test)]
pub(crate) use uninstall::remove_hook_registrations;
pub use uninstall::run_uninstall;

/// Test-only view of [`types::essential_files`].
///
/// Exists so `launch::tests_system_prompt_append` can assert that the
/// system-prompt fragment is NOT listed — adding it there is what armed a
/// cwd-sourced restage on every install (see `types::essential_files`), and a
/// ratchet in the module that consumes the fragment is the one place a future
/// edit would actually look.
#[cfg(test)]
pub(in crate::commands) fn essential_files_for_test(
    layout: types::SourceLayout,
) -> &'static [&'static str] {
    types::essential_files(layout)
}
use verification::verify_install_completeness;

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};

/// Who asked for an install (issue #1526).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallMode {
    /// `amplihack install` or `amplihack update`: replaces the binaries in
    /// `~/.local/bin` whatever their version, and merges hooks into
    /// settings.json. A deliberate downgrade goes through here.
    Explicit,
    /// A re-stage started by startup self-heal or the launch bootstrap. It
    /// never writes settings.json, and replaces an installed binary only when
    /// this build is strictly newer than the install stamp.
    Implicit,
}

pub fn run_install(local: Option<PathBuf>, interactive: bool, force_refresh: bool) -> Result<()> {
    run_install_scoped(
        local,
        interactive,
        force_refresh,
        true,
        InstallMode::Explicit,
    )
}

/// Repair generic assets without reconciling an unrelated optional provider.
pub(crate) fn repair_framework_assets() -> Result<()> {
    run_install_scoped(
        None,
        false,
        false,
        crate::freshness::codex_selected(),
        InstallMode::Implicit,
    )
}

fn run_install_scoped(
    local: Option<PathBuf>,
    interactive: bool,
    force_refresh: bool,
    reconcile_codex: bool,
    mode: InstallMode,
) -> Result<()> {
    // Run the interactive wizard if --interactive was passed.
    // The wizard produces an optional config; if None, we proceed with defaults.
    let wizard_config = interactive::maybe_run_wizard(interactive)?;

    if let Some(local_path) = local {
        // Validate and canonicalize the --local path
        let canonical = local_path.canonicalize().with_context(|| {
            format!(
                "--local path does not exist or cannot be canonicalized: {}",
                local_path.display()
            )
        })?;
        if !canonical.is_dir() {
            bail!("--local path is not a directory: {}", canonical.display());
        }
        return local_install_scoped(&canonical, wizard_config.as_ref(), reconcile_codex, mode);
    }

    // Issue #675: when triggered by `amplihack update`, force_refresh=true
    // skips the bundled-root check so we always download fresh assets from
    // upstream.  This prevents stale Python-era recipes from persisting after
    // a binary update.
    if !force_refresh {
        // Issue #254: prefer bundled framework assets from the amplihack-rs source
        // tree.  Only fall back to network download when the local source tree is
        // not reachable (e.g. binary installed via `cargo install` on a machine
        // that doesn't have the checkout).
        if let Some(bundled) = find_bundled_framework_root() {
            // Issue #1275: name the source *and* the step that chose it. The
            // resolution order is not obvious from the outside, and staging
            // from an unexpected directory used to be silent.
            println!(
                "📦 Using bundled framework assets from {} ({})",
                bundled.root.display(),
                bundled.origin.describe()
            );
            return local_install_scoped(
                &bundled.root,
                wizard_config.as_ref(),
                reconcile_codex,
                mode,
            );
        }
    } else {
        println!("📦 Forcing fresh framework download from upstream...");
    }

    println!("⚠️  Bundled framework source not found, falling back to network download...");
    let temp_dir = tempfile::tempdir().context("failed to create temp dir for install")?;
    let extracted_root = download_and_extract_framework_repo(temp_dir.path())?;
    local_install_scoped(
        &extracted_root,
        wizard_config.as_ref(),
        reconcile_codex,
        mode,
    )?;

    // Network-fallback hard-error: every entry in the active layout's
    // destination set must have been staged. Read the .layout marker the
    // install just wrote to know which layout to verify.
    let staging_dir = staging_claude_dir()?;
    let layout = read_layout_marker(&staging_dir)?.unwrap_or(SourceLayout::LegacyClaude);
    let mut missing_essentials = Vec::new();
    for dst in essential_destinations(layout) {
        if !staging_dir.join(dst).exists() {
            missing_essentials.push(*dst);
        }
    }
    if !missing_essentials.is_empty() {
        bail!(
            "network-fallback install completed but the staged tree at {} is missing \
             required essentials for layout `{}`: {:?}. \
             Re-run `amplihack install` or check upstream archive integrity.",
            staging_dir.display(),
            layout.marker_str(),
            missing_essentials
        );
    }

    // Record the upstream SHA the staged framework now reflects, so the
    // freshness check can compare against it on subsequent launches. This
    // is best-effort — a failed SHA fetch doesn't roll back the install.
    if let Some(sha) = crate::freshness::current_framework_remote_sha() {
        crate::freshness::record_framework_installed_sha(&sha);
    }
    Ok(())
}

/// Path of the `.layout` marker inside the staged `.claude` dir.
fn layout_marker_path(claude_dir: &Path) -> PathBuf {
    claude_dir.join(".layout")
}

/// Atomically write the `.layout` marker via temp-file + rename so partial
/// writes never produce a torn read in `read_layout_marker`.
pub(super) fn write_layout_marker(claude_dir: &Path, layout: SourceLayout) -> Result<()> {
    fs::create_dir_all(claude_dir)
        .with_context(|| format!("failed to create {}", claude_dir.display()))?;
    let final_path = layout_marker_path(claude_dir);
    let tmp_path = claude_dir.join(".layout.tmp");
    let body = format!("{}\n", layout.marker_str());
    fs::write(&tmp_path, body.as_bytes())
        .with_context(|| format!("failed to write {}", tmp_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o644));
    }
    fs::rename(&tmp_path, &final_path).with_context(|| {
        format!(
            "failed to rename {} to {}",
            tmp_path.display(),
            final_path.display()
        )
    })?;
    Ok(())
}

/// Read the `.layout` marker. Returns `Ok(None)` for a missing marker
/// (silent — pre-fix installs lack one). Malformed contents are warned and
/// treated as None (caller may default to `LegacyClaude` for compat).
pub(super) fn read_layout_marker(claude_dir: &Path) -> Result<Option<SourceLayout>> {
    let path = layout_marker_path(claude_dir);
    if !path.exists() {
        return Ok(None);
    }
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    // Hard cap to avoid logging huge attacker-controlled blobs in the warn
    // path; the legitimate file is a single-line word.
    if raw.len() > 64 {
        tracing::warn!(
            "{} is unexpectedly large ({} bytes); ignoring",
            path.display(),
            raw.len()
        );
        return Ok(None);
    }
    match raw.trim() {
        "bundle" => Ok(Some(SourceLayout::Bundle)),
        "legacy" => Ok(Some(SourceLayout::LegacyClaude)),
        other => {
            tracing::warn!(
                "{} contains unrecognised layout `{}` ({} bytes); defaulting to legacy",
                path.display(),
                other,
                raw.len()
            );
            Ok(None)
        }
    }
}

/// Whether the staged framework assets need a restage.
///
/// Pure, so the "restage on every launch" loop it exists to prevent is
/// testable without performing an install.
///
/// The load-bearing invariant is sharper than "every gap
/// `missing_framework_paths` can emit is one a restage closes": it is that **no
/// gap `missing_framework_paths` can emit is tolerated**. A tolerated gap
/// survives `verify_framework_assets`, stays missing on disk, and re-satisfies
/// `!missing.is_empty()` on the next launch — restaging forever. That is #1266
/// verbatim, and it is pinned by `settings::tests::
/// no_emittable_asset_gap_is_ever_tolerated`, which crosses the real producer
/// against `is_tolerated_asset_gap` on both source layouts.
///
/// The gaps that are emitted must also each be closable by a restage. Issue
/// #1266's loop came from listing an asset the restage source could not supply;
/// the fix was to stop listing it (the system-prompt fragment is `include_str!`d
/// into the binary now — see `launch::system_prompt_append`), not to special-
/// case it here. Before adding an entry to `essential_files`, check that a
/// restage can actually satisfy it, or this becomes a loop again.
fn framework_restage_needed(staging_exists: bool, missing: &[String]) -> bool {
    !staging_exists || !missing.is_empty()
}

/// Announce entries a publisher carried across because amplihack could not
/// prove it had staged them.
///
/// Loud on purpose: before issue #1344's review these were renamed aside and
/// deleted while the installer printed a green success line.
fn report_preserved_entries(preserved: &[String], target: &Path) {
    if preserved.is_empty() {
        return;
    }
    println!(
        "  ⚠️  Kept {} file(s) in {} that amplihack did not stage: {}",
        preserved.len(),
        target.display(),
        preserved.join(", ")
    );
}

/// Issue #1526: the install stamp's version when this binary is older than
/// it (by semver), else `None`. Every implicit installer checks this first.
pub(crate) fn newer_install_stamp() -> Result<Option<String>> {
    Ok(version_stamp::read_installed_version_quiet()
        .context("reading install version stamp")?
        .filter(|installed| {
            version_stamp::relation(crate::VERSION, installed)
                == version_stamp::VersionRelation::Older
        }))
}

pub(crate) fn ensure_framework_installed() -> Result<()> {
    ensure_framework_installed_with(&mut std::io::stderr())
}

/// [`ensure_framework_installed`], writing the #1526 refusal warning to
/// `notice` (stderr in the binary).
pub(crate) fn ensure_framework_installed_with<W: std::io::Write>(notice: &mut W) -> Result<()> {
    // Issue #1526: this bootstrap is an implicit installer too. A binary older
    // than the installed stamp must not stage, top up commands or repair
    // settings.json over the newer install.
    if let Some(installed) = newer_install_stamp()? {
        // debug: tracing writes to stdout. The user-facing warning goes to
        // `notice`, once per process: startup self-heal has usually printed it.
        tracing::debug!(
            "binary {} is older than install stamp {installed}; skipping launch bootstrap",
            crate::VERSION
        );
        // This calls back into `self_heal`, which depends on `install`. The
        // cycle is deliberate: the warning shares self_heal's once-per-process
        // flag and its AMPLIHACK_SKIP_AUTO_INSTALL check.
        crate::self_heal::warn_implicit_downgrade_once(notice, &installed);
        return Ok(());
    }

    let staging_dir = staging_claude_dir()?;
    let staging_exists = staging_dir.exists();
    let missing = if staging_exists {
        missing_framework_paths(&staging_dir)?
    } else {
        Vec::new()
    };
    // Issue #254: framework assets are now bundled in the amplihack-rs source
    // tree.  The legacy upstream freshness check is removed;
    // framework updates are delivered via amplihack-rs binary updates instead.
    if framework_restage_needed(staging_exists, &missing) {
        println!("🔧 Bootstrapping amplihack framework assets...");
        repair_framework_assets()?;
    }

    // Issue #1344: `~/.claude/commands/amplihack/` lives outside `claude_dir`,
    // so `missing_framework_paths` never sees it — and must not, per the
    // restage-loop invariant documented on `framework_restage_needed`. Top it
    // up directly instead, so a namespace lost after install comes back.
    if let Err(error) = claude_commands::ensure_claude_commands_staged() {
        tracing::warn!("could not verify amplihack slash commands: {error:#}");
    }

    // Verify hooks are registered in settings.json — even after a fresh install.
    // This catches the case where `run_install` completed but hooks were not
    // wired into settings.json (issue #202: silent unwiring on fresh env).
    // Issue #911: settings.json outlives this process, so it must record the
    // DEPLOYED path, never a build-tree sibling of the running executable.
    let hooks_bin = stable_hooks_binary().context(
        "amplihack-hooks binary not found. Run `amplihack install` to set up hooks, \
         or set AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH to the binary location.",
    )?;
    // Issue #1538: a selected Codex is reconciled strictly, installed or not.
    // The Codex transaction is a no-op when current and preserves every byte
    // on failure; an older binary never gets here (#1526 refuses it first).
    if crate::freshness::codex_selected() {
        let source = find_bundled_framework_root()
            .context("bundled source required to reconcile native Codex resources")?;
        codex_plugin::install(&source.root, &hooks_bin)
            .context("failed native Codex reconciliation")?;
    }
    let settings_path = global_settings_path()?;
    match hooks_registered_in_settings(&settings_path)? {
        HookRegistration::Registered => {
            // #1659: a settings.json from #1660 registers `StopFailure` for
            // every user. A Claude Code too old for it ignores the whole
            // file, so the entry is drift even though every hook is present.
            // Only a Claude launch asks `claude --version`: copilot and codex
            // never read this file, and the next Claude launch repairs it.
            let Some(version) =
                harmful_limit_resume_registration(&settings_path, launched_claude_too_old)
            else {
                return Ok(());
            };
            tracing::warn!(
                "settings.json registers StopFailure, which Claude Code {version} does not know \
                 — auto-repairing"
            );
            println!(
                "⚠️  Claude Code {version} is older than {LIMIT_RESUME_MIN_CLAUDE}: removing the \
                 usage-limit resume hook (StopFailure) from settings.json, which this Claude \
                 Code would otherwise read as invalid and run no amplihack hook"
            );
        }
        HookRegistration::Missing => {}
        HookRegistration::Unparseable => {
            // Issue #1526 (SR-8): never back up or rewrite a file we cannot read.
            tracing::warn!("settings.json is not valid JSON; skipping hook auto-repair");
            eprintln!(
                "amplihack: warning: {} is not valid JSON; hook registrations were not \
                 checked or repaired. Fix the file, then run `amplihack install`.",
                paths::sanitize_path(&settings_path)
            );
            return Ok(());
        }
    }
    // Issue #1699: never write a hooks path that does not exist or is not
    // safe to run. Warn and carry on; `amplihack install` repairs it.
    if let Err(reason) = crate::commands::profile::hook_path_guard::check_hook_binary(&hooks_bin) {
        tracing::warn!(hooks_bin = ?hooks_bin, "hook auto-repair skipped");
        eprintln!(
            "amplihack: warning: hook auto-repair skipped ({reason}). Run `amplihack install`."
        );
        return Ok(());
    }
    tracing::warn!("hooks not registered in settings.json — auto-repairing");
    let timestamp = unix_timestamp();
    let (settings_ok, _events) =
        ensure_settings_json(&settings_path, &staging_dir, timestamp, &hooks_bin)?;
    if !settings_ok {
        bail!(
            "failed to configure ~/.claude/settings.json for amplihack hooks.\n\
             Run `amplihack install` to repair, or `amplihack doctor` to diagnose."
        );
    }
    // Verify the repair actually worked
    if hooks_registered_in_settings(&settings_path)? != HookRegistration::Registered
        || harmful_limit_resume_registration(&settings_path, launched_claude_too_old).is_some()
    {
        bail!(
            "hooks still not registered after auto-repair.\n\
             Run `amplihack install` manually to fix hook wiring."
        );
    }
    println!("✅ Auto-repaired missing hook registrations in settings.json");
    Ok(())
}

/// Issue #1526: an implicit re-stage does not write `settings_path`, so an
/// upgrade that adds a hook event leaves it out. Name what is missing on
/// `notice` (stderr in the binary), and say nothing when nothing is. A file
/// that is absent, unreadable or not JSON is not reported here: the launch
/// bootstrap repairs the first and warns about the last.
pub(super) fn warn_implicit_hook_drift<W: std::io::Write>(notice: &mut W, settings_path: &Path) {
    warn_implicit_hook_drift_for(
        notice,
        settings_path,
        claude_too_old_for_limit_resume().as_deref(),
    );
}

/// [`warn_implicit_hook_drift`] for a `claude` whose version is `old_claude`
/// when it is older than [`LIMIT_RESUME_MIN_CLAUDE`] (`None` otherwise).
/// With an old `claude`, an amplihack `StopFailure` registration still in
/// the file (#1660 wrote one for every user) is reported too: that Claude
/// Code ignores the whole file, so no amplihack hook runs until it is gone.
pub(super) fn warn_implicit_hook_drift_for<W: std::io::Write>(
    notice: &mut W,
    settings_path: &Path,
    old_claude: Option<&str>,
) {
    let Ok(raw) = fs::read_to_string(settings_path) else {
        return;
    };
    let Ok(settings) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return;
    };
    if let Some(version) = old_claude
        && has_limit_resume_registration(&settings)
    {
        let _ = writeln!(
            notice,
            "amplihack: warning: {} registers the usage-limit resume hook (StopFailure), and \
             Claude Code {version} is older than {LIMIT_RESUME_MIN_CLAUDE}: a Claude Code too \
             old for StopFailure ignores the whole file, so no amplihack hook runs. An \
             automatic re-stage never edits settings.json; run `amplihack install` to remove \
             it (the next launch also removes it).",
            paths::sanitize_path(settings_path),
        );
    }
    let missing = missing_hook_registrations(&settings, old_claude.is_none());
    if missing.is_empty() {
        return;
    }
    let _ = writeln!(
        notice,
        "amplihack: warning: {} lacks hook registrations this version ({}) adds or changes: {}. \
         An automatic re-stage never edits settings.json; run `amplihack install` to add them.",
        paths::sanitize_path(settings_path),
        crate::VERSION,
        missing.join(", ")
    );
}

/// #1659: the version of the `claude` a launch would run, when
/// `settings_path` holds amplihack's `StopFailure` registration and that
/// `claude` is older than [`LIMIT_RESUME_MIN_CLAUDE`]. `old_claude` is only
/// called when the registration is there, so a launch without it never
/// probes. A file that is absent, unreadable or not JSON gives `None`.
pub(super) fn harmful_limit_resume_registration(
    settings_path: &Path,
    old_claude: impl FnOnce() -> Option<String>,
) -> Option<String> {
    let raw = fs::read_to_string(settings_path).ok()?;
    let settings = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    if !has_limit_resume_registration(&settings) {
        return None;
    }
    old_claude()
}

/// #1659: [`claude_too_old_for_limit_resume`] for the launch bootstrap, which
/// runs before every launch. A launch of another tool gives `None` without
/// running `claude --version` (about 150 ms, up to the 10-second probe
/// budget): that tool never reads `~/.claude/settings.json`, and the next
/// Claude launch removes an entry its Claude Code cannot read.
fn launched_claude_too_old() -> Option<String> {
    claude_too_old_for_launch(claude_too_old_for_limit_resume)
}

/// [`launched_claude_too_old`] with the version check passed in.
pub(super) fn claude_too_old_for_launch(
    old_claude: impl FnOnce() -> Option<String>,
) -> Option<String> {
    if !crate::freshness::claude_selected() {
        return None;
    }
    old_claude()
}

/// Whether `~/.claude/settings.json` holds an `amplihack-hooks` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HookRegistration {
    Registered,
    /// The file is absent, or parses with no amplihack hook.
    Missing,
    Unparseable,
}

/// Classify the settings file. Absence is read off the failed read (no
/// `exists()` probe, issue #1123); any other read error, such as EACCES, is
/// an `Err`, because an unreadable file is not evidence that hooks are missing.
pub(super) fn hooks_registered_in_settings(settings_path: &Path) -> Result<HookRegistration> {
    let raw = match fs::read_to_string(settings_path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(HookRegistration::Missing);
        }
        Err(e) => {
            return Err(e).with_context(|| format!("failed to read {}", settings_path.display()));
        }
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Ok(HookRegistration::Unparseable);
    };
    let has_hooks = json
        .get("hooks")
        .and_then(|h| h.as_object())
        .map(|hooks_map| {
            hooks_map.values().any(|wrappers| {
                wrappers.as_array().is_some_and(|arr| {
                    arr.iter().any(|wrapper| {
                        wrapper
                            .get("hooks")
                            .and_then(|h| h.as_array())
                            .is_some_and(|entries| {
                                entries.iter().any(|entry| {
                                    entry
                                        .get("command")
                                        .and_then(|c| c.as_str())
                                        .is_some_and(|cmd| cmd.contains("amplihack-hooks"))
                                })
                            })
                    })
                })
            })
        })
        .unwrap_or(false);
    Ok(if has_hooks {
        HookRegistration::Registered
    } else {
        HookRegistration::Missing
    })
}

#[cfg(test)]
fn local_install(
    repo_root: &Path,
    wizard_config: Option<&interactive::InteractiveConfig>,
) -> Result<()> {
    local_install_scoped(repo_root, wizard_config, true, InstallMode::Explicit)
}

fn local_install_scoped(
    repo_root: &Path,
    wizard_config: Option<&interactive::InteractiveConfig>,
    reconcile_codex: bool,
    mode: InstallMode,
) -> Result<()> {
    let claude_dir = staging_claude_dir()?;
    let timestamp = unix_timestamp();

    println!();
    println!("🚀 Starting amplihack installation...");
    println!("   Source: {}", repo_root.display());
    println!("   Target: {}", claude_dir.display());
    // Phase 0: deploy binaries
    println!();
    println!("🦀 Deploying binaries:");
    // Issue #1526: read before anything below rewrites it. An implicit
    // re-stage compares each installed binary against the binaries' version.
    let stamp = match mode {
        InstallMode::Explicit => None,
        // Safe: `None` makes `implicit_deploy_allowed` refuse every occupied
        // slot, so an unreadable stamp only costs a warning.
        InstallMode::Implicit => version_stamp::read_binaries_version()
            .inspect_err(|e| {
                eprintln!(
                    "amplihack: warning: cannot read the install stamp; keeping installed \
                     binaries: {}",
                    paths::sanitize_text(&format!("{e:#}"))
                )
            })
            .unwrap_or_default(),
    };
    let (deployed_binaries, kept_binaries) = deploy_binaries(mode, stamp.as_deref())?;
    let hooks_bin = find_hooks_binary()?;
    for p in deployed_binaries
        .iter()
        .filter(|p| !kept_binaries.contains(p))
    {
        println!("  ✅ Deployed {}", p.display());
    }
    let preferred_amplihack =
        paths::preferred_user_bin_dir()?.join(crate::path_conflicts::binary_filename("amplihack"));
    if preferred_amplihack.is_file() {
        let stale_wrapper_path =
            std::env::var_os("AMPLIHACK_REPAIR_ORIGINAL_PATH").or_else(|| std::env::var_os("PATH"));
        let path_dirs = stale_wrapper_path
            .map(|value| std::env::split_paths(&value).collect())
            .unwrap_or_default();
        let repair = stale_wrappers::neutralize_shadowing_stale_wrappers(
            stale_wrappers::StaleWrapperNeutralizerConfig {
                home_dir: paths::home_dir()?,
                current_exe: std::env::current_exe()
                    .unwrap_or_else(|_| preferred_amplihack.to_path_buf()),
                preferred_rust_binary: preferred_amplihack.clone(),
                path_dirs,
                binary_name: crate::path_conflicts::binary_filename("amplihack"),
            },
        )
        .context("failed to neutralize stale Python/uvx amplihack PATH wrappers")?;
        for shim in &repair.skipped_transient_shims {
            println!(
                "  ℹ️  Leaving transient package-manager launcher {} in place; it stops shadowing {} once the launching command exits.",
                shim.display(),
                preferred_amplihack.display()
            );
        }
        for launcher in &repair.persistent_npm_launchers {
            println!(
                "  ℹ️  Leaving this package's npm launcher {} in place. It comes before {} on PATH, so `amplihack` runs through the npm wrapper (its own cached release binary).",
                launcher.display(),
                preferred_amplihack.display()
            );
            println!(
                "     To use {} instead: remove it with the package manager that installed it (e.g. npm uninstall -g @rysweet/amplihack-rs), or export PATH=\"$HOME/.local/bin:$PATH\"",
                preferred_amplihack.display()
            );
        }
        if !repair.neutralized.is_empty() {
            let manifest_path = repair
                .manifest_path
                .as_ref()
                .context("stale wrapper repair quarantined files without writing a manifest")?;
            println!(
                "  ✅ Quarantined {} stale amplihack wrapper(s); manifest: {}",
                repair.neutralized.len(),
                manifest_path.display()
            );
        }
    }

    ensure_dirs(&claude_dir)?;
    let pre_dirs = all_rel_dirs(&claude_dir)?;

    println!();
    println!("📁 Copying essential directories:");
    let (source_root, layout) = find_source_root(repo_root)?;
    println!(
        "   Source layout: {} (from {})",
        layout.marker_str(),
        source_root.display()
    );
    if layout == SourceLayout::Bundle {
        validate_framework_bundle_compatibility(&source_root).with_context(|| {
            format!(
                "source amplifier-bundle at {} is incompatible",
                source_root.display()
            )
        })?;
    }
    let copied_dirs = copytree_manifest(&source_root, layout, &claude_dir)?;
    if copied_dirs.is_empty() {
        bail!(
            "no essential directories were copied from {} (layout: {}). \
             The source repository appears to be missing all framework assets. \
             Verify the checkout is complete.",
            source_root.display(),
            layout.marker_str()
        );
    }

    // Write the .layout marker atomically so subsequent presence checks
    // (missing_framework_paths) know which mapping to consult.
    write_layout_marker(&claude_dir, layout)?;

    println!();
    println!("📦 Staging amplifier-bundle (recipes, modules, tools):");
    copy_amplifier_bundle(repo_root, &claude_dir)?;

    println!();
    println!("📝 Initializing PROJECT.md:");
    initialize_project_md(&claude_dir)?;

    println!();
    println!("📂 Creating runtime directories:");
    create_runtime_dirs(&claude_dir)?;

    println!();
    println!("🧹 Cleaning broken symlinks:");
    let broken_count = filesystem::clean_broken_symlinks(&claude_dir, true)?;
    if broken_count > 0 {
        println!("   Removed {broken_count} broken symlink(s)");
    } else {
        println!("   No broken symlinks found");
    }
    // Also clean broken symlinks in ~/.local/bin (stale gadugi-test, etc.)
    if let Ok(home) = paths::home_dir() {
        let local_bin = home.join(".local").join("bin");
        let local_broken = filesystem::clean_broken_symlinks(&local_bin, false)?;
        if local_broken > 0 {
            println!("   Removed {local_broken} broken symlink(s) from ~/.local/bin");
        }
    }

    // Honor the interactive hook-scope choice (issue #1119). Repo-local scope
    // resolves against the current working directory — where `amplihack install`
    // runs — and falls back to global if that directory is not a git repo.
    let requested_scope = wizard_config
        .map(|c| c.hook_scope)
        .unwrap_or(interactive::HookScope::Global);
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let effective_scope = interactive::resolve_hook_scope(requested_scope, &cwd);
    let settings_target = effective_scope.settings_path_for(&cwd);
    // Issue #911: every DURABLE consumer -- settings.json and the Copilot
    // plugin manifest -- records this path, so it must be the DEPLOYED binary,
    // never `hooks_bin` (the `find_hooks_binary()` source path).
    // deploy_binaries() has already run above, so the stable copy exists.
    //
    // Copilot already used the deployed path; settings.json did not, which is
    // what put a build directory into a user's hook config and broke every
    // hook when that directory was cleaned. Both now share one binding so they
    // cannot drift apart again.
    let durable_hooks_bin = deployed_binaries
        .first()
        .cloned()
        .unwrap_or_else(|| hooks_bin.clone());
    // Issue #1526: an implicit re-stage never writes (or backs up) the
    // user's settings.json; only `amplihack install` does. Its one output is
    // a stderr warning when settings.json lacks hooks this build registers,
    // so an upgrade that adds a hook event is not silently half-installed.
    let (settings_ok, registered_events) = match mode {
        InstallMode::Explicit => {
            println!();
            println!("⚙️  Configuring settings.json:");
            println!(
                "   Scope: {} → {}",
                effective_scope.display_name(),
                settings_target.display()
            );
            let configured =
                ensure_settings_json(&settings_target, &claude_dir, timestamp, &durable_hooks_bin)?;
            // Profiles copy the user's settings, not a repo's (issue #1661).
            if effective_scope == interactive::HookScope::Global {
                refresh_profile_hooks(&durable_hooks_bin);
            }
            // A rotator started before this upgrade keeps its old code (#1659).
            #[cfg(unix)]
            if !cfg!(test) {
                crate::commands::profile::warn_foreign_rotator(None);
            }
            configured
        }
        InstallMode::Implicit => {
            warn_implicit_hook_drift(&mut std::io::stderr(), &settings_target);
            (true, Vec::new())
        }
    };

    println!();
    println!("🐙 Configuring GitHub Copilot CLI plugin (if installed):");

    // Warn when Node.js is too old for Copilot CLI — don't fail the whole
    // install (it does many unrelated things), but make it visible.
    {
        use amplihack_utils::prerequisites::check_node_minimum_version;
        if let Err(err) = check_node_minimum_version(24) {
            eprintln!("  ⚠️  {err}");
            eprintln!("     Copilot CLI sessions will fail until this is resolved.");
        }
    }

    // Register the Copilot plugin against the STABLE DEPLOYED hooks binary
    // (~/.local/bin/amplihack-hooks, the first entry from deploy_binaries()),
    // NEVER the transient `find_hooks_binary()` source path (e.g.
    // target/debug/amplihack-hooks). The source path lives in a build/worktree
    // dir that gets cleaned, making every hook exit 127; Copilot CLI fails
    // closed on hook errors, denying every tool call in nested sessions (#911).
    // Issue #1526: decided once, before either Copilot writer runs below.
    let copilot_config = match mode {
        InstallMode::Explicit => crate::copilot_setup::UserConfig::Merge,
        InstallMode::Implicit => crate::copilot_setup::UserConfig::for_implicit_install(),
    };
    match copilot_plugin::register_copilot_plugin(repo_root, &durable_hooks_bin, copilot_config)
        .context("failed to register Copilot CLI plugin")?
    {
        copilot_plugin::CopilotRegistration::Registered => {
            println!("  ✅ Copilot CLI plugin amplihack@local refreshed");
        }
        copilot_plugin::CopilotRegistration::ConfigPreserved { foreign_cache_path } => {
            // Issue #1526: ~/.copilot/config.json is the user's; an implicit
            // re-stage refreshed only the plugin directory amplihack owns.
            println!(
                "  ✅ Copilot CLI plugin files refreshed; ~/.copilot/config.json not modified"
            );
            if let Some(path) = foreign_cache_path {
                eprintln!(
                    "amplihack: warning: ~/.copilot/config.json registers amplihack from {}, \
                     not this install's plugin directory; an automatic re-stage leaves it \
                     unchanged. Run `amplihack install` to point it here.",
                    paths::sanitize_text(&path)
                );
            }
        }
        copilot_plugin::CopilotRegistration::NotDetected => {
            println!("  ↩️  Copilot CLI not detected (~/.copilot missing) — skipping");
        }
    }

    println!();
    println!("🤖 Publishing assets to Claude Code (~/.claude):");
    // Issues #1344 and #1438: staging into `~/.amplihack/.claude/` reaches no
    // Claude session. Each asset type has to be published into the directories
    // Claude Code actually discovers, and both times that was missed it was
    // because the fan-out was a per-asset-type call someone had to remember.
    // `claude_publication::CLAUDE_DESTINATIONS` is the single enumerated list
    // of those destinations; this loop is the only thing that walks it, and
    // every outcome — published, skipped, failed — gets a line, because the
    // absence of a line is exactly what made both gaps invisible.
    //
    // Deliberately NOT fatal: `ensure_framework_installed` calls this whole
    // install path on every `amplihack launch`, so a permission problem under
    // `~/.claude/` — or a source that resolves to the staging target itself —
    // must cost the user those assets, not their session.
    let mut claude_features: Vec<String> = Vec::new();
    for destination in claude_publication::CLAUDE_DESTINATIONS {
        match (destination.publish)(repo_root) {
            Ok(published) => {
                println!("  {}", published.detail);
                report_preserved_entries(&published.preserved, &published.target);
                if let Some(feature) = published.feature {
                    claude_features.push(feature);
                }
            }
            Err(error) => {
                println!(
                    "  ⚠️  Could not publish {} to ~/.claude: {error:#}",
                    destination.label
                );
            }
        }
    }

    println!();
    println!("🔍 Verifying staged framework assets:");
    verify_framework_assets(&claude_dir)?;
    verify_install_completeness(&source_root, layout, &claude_dir)?;

    if reconcile_codex {
        println!();
        // Issue #1538: reconcile_codex is only set when Codex is the selected
        // client, so an implicit re-stage reconciles it as strictly as
        // `amplihack install`. Issue #1526's downgrade refusal runs earlier.
        codex_plugin::install(repo_root, &durable_hooks_bin)
            .context("failed native Codex installation")?;
    }

    println!("📝 Generating uninstall manifest:");
    let manifest_path = manifest_path()?;
    let mut tracked_roots = Vec::new();
    for dir in &copied_dirs {
        let full = claude_dir.join(dir);
        if full.exists() {
            tracked_roots.push(full);
        }
    }
    for dir in RUNTIME_DIRS {
        let full = claude_dir.join(dir);
        if full.exists() {
            tracked_roots.push(full);
        }
    }
    let (files, post_dirs) = get_all_files_and_dirs(&claude_dir, &tracked_roots)?;
    let mut new_dirs = post_dirs
        .into_iter()
        .filter(|dir| !pre_dirs.contains(dir))
        .collect::<Vec<_>>();
    // Issue #1507: `new_dirs` is "created by this run". On a reinstall every
    // directory already exists, so it would be empty and uninstall would
    // leave the earlier run's directories behind. Carry forward what previous
    // manifests recorded, as long as it still exists.
    let previous = manifest::read_manifest(&manifest_path)
        .inspect_err(|error| tracing::warn!("ignoring previous install manifest: {error:#}"))
        .unwrap_or_default();
    for dir in previous.dirs {
        if !new_dirs.contains(&dir) && claude_dir.join(&dir).is_dir() {
            new_dirs.push(dir);
        }
    }
    let mut binaries: Vec<String> = deployed_binaries
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let mut hook_registrations = registered_events;
    // Issue #1526: an implicit re-stage may keep binaries and settings.json
    // as an earlier install left them. That install's manifest entries still
    // describe them, so uninstall must keep finding them. Only binaries we
    // deploy, in the directory we deploy them to, are carried forward.
    if mode == InstallMode::Implicit {
        let bin_dir = paths::preferred_user_bin_dir()?;
        for previous_bin in previous.binaries {
            let path = Path::new(&previous_bin);
            if binary::is_deployed_binary(path, &bin_dir)
                && path.symlink_metadata().is_ok()
                && !binaries.contains(&previous_bin)
            {
                binaries.push(previous_bin);
            }
        }
        hook_registrations = previous.hook_registrations;
    }

    let mut manifest = InstallManifest {
        files,
        dirs: new_dirs,
        binaries,
        hook_registrations,
        ..InstallManifest::default()
    };

    // Apply interactive wizard configuration to the manifest if the wizard ran.
    if let Some(config) = wizard_config {
        interactive::apply_config(config, &mut manifest);
        println!();
        println!("🧙 Interactive configuration applied:");
        println!("   • Default tool: {}", config.default_tool.display_name());
        println!(
            "   • Hook scope: {} ({})",
            effective_scope.display_name(),
            settings_target.display()
        );
        println!("   • Update checks: {}", config.update_check.display_name());
    }

    write_manifest(&manifest_path, &manifest)?;
    println!("   Manifest written to {}", manifest_path.display());

    // Issue #1491: this phase can fail on an air-gapped host (no network for
    // `cargo install`) or a host that cannot bootstrap a toolchain. It stays
    // a loud failure (issue #527 install-completeness), but it runs only
    // AFTER the manifest above is written, so the staged assets are never
    // left behind without an `amplihack uninstall` path.
    println!();
    println!("🦀 Ensuring Rust recipe runner:");
    recipe_runner::ensure_recipe_runner().with_context(|| {
        format!(
            "framework assets are staged at {} and the uninstall manifest is written; \
             provision recipe-runner-rs and re-run `amplihack install` to finish, or run \
             `amplihack uninstall` to remove the staged assets",
            claude_dir.display()
        )
    })?;

    println!();
    println!("============================================================");
    if settings_ok && !copied_dirs.is_empty() {
        println!("✅ Amplihack installation completed successfully!");
        println!();
        println!("📍 Installed to: {}", claude_dir.display());
        println!();
        println!("📦 Components installed:");
        for dir in &copied_dirs {
            println!("   • {dir}");
        }
        println!("   • amplifier-bundle (recipes, modules, tools)");
        println!();
        println!("🎯 Features enabled:");
        println!("   • Session start hook");
        println!("   • Stop hook");
        println!("   • Post-tool-use hook");
        println!("   • Pre-compact hook");
        println!("   • Runtime logging and metrics");
        for feature in &claude_features {
            println!("   • {feature}");
        }
        println!("   • dev-orchestrator recipe execution");
        println!();
        println!("💡 To uninstall: amplihack uninstall");
    } else {
        println!("⚠️  Installation completed with warnings");
        if !settings_ok {
            println!("   • Settings.json configuration had issues");
        }
        if copied_dirs.is_empty() {
            println!("   • No directories were copied");
        }
        println!();
        println!("💡 You may need to manually verify the installation");
    }
    println!("============================================================");
    println!();

    // Issue #1526: the framework is now current, but kept binaries are still
    // at the version the old stamp recorded (or unknown). Record both, so the
    // next launch does not re-stage and the guard keeps failing safe. The
    // `binaries:` line is the kept binaries' version only: binaries this run
    // replaced (often `amplihack` itself) are at `crate::VERSION`, so readers
    // must not attach it to any one path (see `format_downgrade_warning`).
    if kept_binaries.is_empty() {
        version_stamp::write_installed_version(crate::VERSION)
    } else if stamp.as_deref() == Some(crate::VERSION) {
        // Kept binaries already at this version: nothing to replace, no warning.
        version_stamp::write_installed_version(crate::VERSION)
    } else {
        let kept: Vec<String> = kept_binaries
            .iter()
            .map(|p| paths::sanitize_path(p))
            .collect();
        eprintln!(
            "amplihack: warning: kept {} (installed version: {}) during an implicit re-stage \
             for v{}; run `amplihack install` to replace them.",
            kept.join(", "),
            stamp.as_deref().unwrap_or("unknown"),
            crate::VERSION,
        );
        version_stamp::write_installed_versions(crate::VERSION, stamp.as_deref())
    }
    .context("writing installed-version stamp")?;

    // Stage assets to ~/.copilot/skills/ so Copilot CLI picks them up
    // without requiring a separate `amplihack copilot` launch. Best-effort
    // and intentionally AFTER the version stamp: a failure here must not
    // block the install or leave the version stamp unwritten.
    match crate::copilot_setup::ensure_copilot_home_staged(copilot_config) {
        Ok(()) => println!("  ✅ Copilot home staged (~/.copilot/)"),
        Err(err) => {
            tracing::warn!(%err, "failed to stage copilot home during install");
            eprintln!("  ⚠️  Copilot home staging skipped: {err}");
        }
    }

    // Best-effort: provision the mermaid CLI (mmdc) so the pr-guide skill can
    // render diagrams locally for Azure DevOps instead of relying on the
    // third-party mermaid.ink service. mmdc is OPTIONAL (it pulls in puppeteer
    // + a headless Chromium download and needs Node/npm), so per the
    // install-completeness invariant a failure here must warn-and-continue and
    // never abort the install. Intentionally AFTER the version stamp + manifest
    // so this optional step can never leave required state unwritten.
    println!("Installing mermaid CLI for local diagram rendering...");
    match mermaid_cli::ensure_mermaid_cli() {
        Ok(mermaid_cli::Outcome::AlreadyPresent) => {
            println!("  ✓ mermaid CLI (mmdc) already installed; skipping");
        }
        Ok(mermaid_cli::Outcome::Installed) => {
            println!("  ✅ mermaid CLI (mmdc) installed for local diagram rendering");
        }
        Ok(mermaid_cli::Outcome::SkippedByEnv) => {
            println!("  ℹ AMPLIHACK_SKIP_MMDC set; skipping mermaid CLI install");
        }
        Ok(mermaid_cli::Outcome::SkippedNoNpm) => {
            println!(
                "  ℹ npm not available; skipping mermaid CLI install \
                 (pr-guide will fall back to mermaid.ink)"
            );
        }
        Ok(mermaid_cli::Outcome::Failed) => {
            tracing::warn!("best-effort mermaid CLI install did not complete");
            eprintln!("{}", mermaid_cli::FALLBACK_NOTICE);
        }
        Err(err) => {
            // ensure_mermaid_cli is contractually always-Ok; this arm exists
            // for defense-in-depth so a future regression still cannot abort
            // the install.
            tracing::warn!(%err, "unexpected error from best-effort mermaid CLI install");
            eprintln!("{}", mermaid_cli::FALLBACK_NOTICE);
        }
    }

    Ok(())
}

pub(crate) fn with_codex_binary<T>(binary: &Path, action: impl FnOnce() -> T) -> T {
    codex_plugin::with_binary(binary, action)
}
