//! Native install and uninstall commands.

mod binary;
mod clone;
mod directories;
mod filesystem;
mod hooks;
pub(crate) mod interactive;
mod manifest;
pub(crate) mod paths;
mod recipe_runner;
mod settings;
mod types;
mod uninstall;
mod verification;
pub(crate) mod version_stamp;

#[cfg(test)]
mod tests;

use binary::{deploy_binaries, find_hooks_binary};
use clone::{download_and_extract_framework_repo, find_bundled_framework_root};
use directories::*;
use filesystem::{all_rel_dirs, get_all_files_and_dirs};
use manifest::{manifest_path, write_manifest};
use paths::*;
use settings::*;
use types::*;
#[cfg(test)]
pub(crate) use uninstall::remove_hook_registrations;
pub use uninstall::run_uninstall;
use verification::verify_install_completeness;

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};

pub fn run_install(local: Option<PathBuf>, interactive: bool) -> Result<()> {
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

pub(crate) fn ensure_framework_installed() -> Result<()> {
    let staging_dir = staging_claude_dir()?;
    let presence_bootstrap_needed =
        !staging_dir.exists() || !missing_framework_paths(&staging_dir)?.is_empty();
    // Issue #254: framework assets are now bundled in the amplihack-rs source
    // tree.  The legacy upstream freshness check is removed;
    // framework updates are delivered via amplihack-rs binary updates instead.
    if presence_bootstrap_needed {
        println!("🔧 Bootstrapping amplihack framework assets...");
        run_install(None, false)?;
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
        let (settings_ok, _events) = ensure_settings_json(&staging_dir, timestamp, &hooks_bin)?;
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
fn hooks_registered_in_settings(settings_path: &Path) -> Result<bool> {
    if !settings_path.exists() {
        return Ok(false);
    }
    let raw = fs::read_to_string(settings_path)
        .with_context(|| format!("failed to read {}", settings_path.display()))?;
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
    println!();
    println!(
        "ℹ️  Profile management unavailable (No module named 'profile_management'), using full installation"
    );
    println!();

    // Phase 0: deploy binaries
    println!();
    println!("🦀 Deploying binaries:");
    let deployed_binaries = deploy_binaries()?;
    let hooks_bin = find_hooks_binary()?;
    for p in &deployed_binaries {
        println!("  ✅ Deployed {}", p.display());
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
    let (settings_ok, registered_events) =
        ensure_settings_json(&claude_dir, timestamp, &hooks_bin)?;

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
        println!("   • Hook scope: {}", config.hook_scope.display_name());
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

    Ok(())
}
