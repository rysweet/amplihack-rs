//! Native install and uninstall commands.

mod binary;
pub(crate) mod bundle_compat;
mod clone;
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

#[cfg(test)]
mod tests;

use binary::{deploy_binaries, find_hooks_binary};
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

pub fn run_install(local: Option<PathBuf>, interactive: bool, force_refresh: bool) -> Result<()> {
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
        return local_install(&canonical, wizard_config.as_ref());
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
        if let Some(bundled_root) = find_bundled_framework_root() {
            println!(
                "📦 Using bundled framework assets from {}",
                bundled_root.display()
            );
            return local_install(&bundled_root, wizard_config.as_ref());
        }
    } else {
        println!("📦 Forcing fresh framework download from upstream...");
    }

    println!("⚠️  Bundled framework source not found, falling back to network download...");
    let temp_dir = tempfile::tempdir().context("failed to create temp dir for install")?;
    let extracted_root = download_and_extract_framework_repo(temp_dir.path())?;
    local_install(&extracted_root, wizard_config.as_ref())?;

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

pub(crate) fn ensure_framework_installed() -> Result<()> {
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
        run_install(None, false, false)?;
    }

    // Verify hooks are registered in settings.json — even after a fresh install.
    // This catches the case where `run_install` completed but hooks were not
    // wired into settings.json (issue #202: silent unwiring on fresh env).
    let hooks_bin = find_hooks_binary().context(
        "amplihack-hooks binary not found. Run `amplihack install` to set up hooks, \
         or set AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH to the binary location.",
    )?;
    let settings_path = global_settings_path()?;
    if !hooks_registered_in_settings(&settings_path)? {
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
        if !hooks_registered_in_settings(&settings_path)? {
            bail!(
                "hooks still not registered after auto-repair.\n\
                 Run `amplihack install` manually to fix hook wiring."
            );
        }
        println!("✅ Auto-repaired missing hook registrations in settings.json");
    }
    Ok(())
}

/// Check whether amplihack hooks are registered in `~/.claude/settings.json`.
///
/// Returns `true` if the settings file exists and its `hooks` section contains
/// at least one entry referencing `amplihack-hooks` (the native binary).
///
/// The absent case is read off the failed read rather than a preceding
/// `exists()` probe, matching the four sites collapsed in issue #1123. Two
/// reasons, and the second is the one that matters: it drops a `stat` from a
/// path that runs on every `amplihack claude` launch, and it closes the TOCTOU
/// window where the file is created or removed between the probe and the read
/// (a probe-then-read reports the state of the file at probe time, which is not
/// the state it then reads). `NotFound` maps to the same `Ok(false)` the probe
/// produced; every other error keeps the existing context message, so a
/// present-but-unreadable settings file is still a hard error and is not
/// silently reported as "no hooks registered".
///
/// One behaviour change is deliberate: `EACCES` while traversing a parent
/// directory used to reach `exists() == false` and so `Ok(false)`, and now
/// returns `Err`, which `ensure_framework_installed`'s `?` propagates and which
/// fails the launch. Fail-closed is the right default for a security-relevant
/// config we cannot read — "unreadable" is not evidence that no hooks are
/// registered — but it is a real delta, so it is written down rather than
/// discovered.
fn hooks_registered_in_settings(settings_path: &Path) -> Result<bool> {
    let raw = match fs::read_to_string(settings_path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => {
            return Err(e).with_context(|| format!("failed to read {}", settings_path.display()));
        }
    };
    let json: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return Ok(false),
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
    Ok(has_hooks)
}

fn local_install(
    repo_root: &Path,
    wizard_config: Option<&interactive::InteractiveConfig>,
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
    let deployed_binaries = deploy_binaries()?;
    let hooks_bin = find_hooks_binary()?;
    for p in &deployed_binaries {
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
                preferred_rust_binary: preferred_amplihack,
                path_dirs,
                binary_name: crate::path_conflicts::binary_filename("amplihack"),
            },
        )
        .context("failed to neutralize stale Python/uvx amplihack PATH wrappers")?;
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

    println!();
    println!("⚙️  Configuring settings.json:");
    // Honor the interactive hook-scope choice (issue #1119). Repo-local scope
    // resolves against the current working directory — where `amplihack install`
    // runs — and falls back to global if that directory is not a git repo.
    let requested_scope = wizard_config
        .map(|c| c.hook_scope)
        .unwrap_or(interactive::HookScope::Global);
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let effective_scope = interactive::resolve_hook_scope(requested_scope, &cwd);
    let settings_target = effective_scope.settings_path_for(&cwd);
    println!(
        "   Scope: {} → {}",
        effective_scope.display_name(),
        settings_target.display()
    );
    let (settings_ok, registered_events) =
        ensure_settings_json(&settings_target, &claude_dir, timestamp, &hooks_bin)?;

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
    let deployed_hooks_bin = deployed_binaries
        .first()
        .cloned()
        .unwrap_or_else(|| hooks_bin.clone());
    match copilot_plugin::register_copilot_plugin(repo_root, &deployed_hooks_bin)
        .context("failed to register Copilot CLI plugin")?
    {
        true => {
            println!("  ✅ Copilot CLI plugin amplihack@local refreshed");
        }
        false => {
            println!("  ↩️  Copilot CLI not detected (~/.copilot missing) — skipping");
        }
    }

    println!();
    println!("🔍 Verifying staged framework assets:");
    verify_framework_assets(&claude_dir)?;
    verify_install_completeness(&source_root, layout, &claude_dir)?;

    println!();
    println!("🦀 Ensuring Rust recipe runner:");
    recipe_runner::ensure_recipe_runner()?;

    println!();
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
    let new_dirs = post_dirs
        .into_iter()
        .filter(|dir| !pre_dirs.contains(dir))
        .collect::<Vec<_>>();

    let mut manifest = InstallManifest {
        files,
        dirs: new_dirs,
        binaries: deployed_binaries
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
        hook_registrations: registered_events,
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

    version_stamp::write_installed_version(crate::VERSION)
        .context("writing installed-version stamp")?;

    // Stage assets to ~/.copilot/skills/ so Copilot CLI picks them up
    // without requiring a separate `amplihack copilot` launch. Best-effort
    // and intentionally AFTER the version stamp: a failure here must not
    // block the install or leave the version stamp unwritten.
    match crate::copilot_setup::ensure_copilot_home_staged() {
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
