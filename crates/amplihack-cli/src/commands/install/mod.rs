//! Native install and uninstall commands.

mod binary;
mod clone;
mod directories;
mod filesystem;
mod hooks;
mod manifest;
mod paths;
mod settings;
mod types;

#[cfg(test)]
mod tests;

use binary::{deploy_binaries, find_hooks_binary};
use clone::download_and_extract_framework_repo;
use directories::*;
use filesystem::{all_rel_dirs, get_all_files_and_dirs};
use hooks::ensure_object;
use manifest::{manifest_path, read_manifest, write_manifest};
use paths::*;
use settings::*;
use types::*;

use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

pub fn run_install(local: Option<PathBuf>) -> Result<()> {
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
        return local_install(&canonical);
    }

    let temp_dir = tempfile::tempdir().context("failed to create temp dir for install")?;
    let extracted_root = download_and_extract_framework_repo(temp_dir.path())?;
    local_install(&extracted_root)
}

pub(crate) fn ensure_framework_installed() -> Result<()> {
    let staging_dir = staging_claude_dir()?;
    let needs_bootstrap =
        !staging_dir.exists() || !missing_framework_paths(&staging_dir)?.is_empty();
    if needs_bootstrap {
        println!("🔧 Bootstrapping amplihack framework assets...");
        run_install(None)?;
        return Ok(());
    }

    let hooks_bin = find_hooks_binary()?;
    let timestamp = unix_timestamp();
    let (settings_ok, _events) = ensure_settings_json(&staging_dir, timestamp, &hooks_bin)?;
    if !settings_ok {
        bail!("failed to configure ~/.claude/settings.json for staged amplihack hooks");
    }
    Ok(())
}

pub fn run_uninstall() -> Result<()> {
    let claude_dir = staging_claude_dir()?;
    let manifest_path = manifest_path()?;
    let manifest = read_manifest(&manifest_path)?;

    let mut removed_any = false;
    let mut removed_files = 0usize;

    // Phase 1: remove files tracked in manifest
    for file in &manifest.files {
        let target = claude_dir.join(file);
        if target.is_file() {
            match fs::remove_file(&target) {
                Ok(()) => {
                    removed_any = true;
                    removed_files += 1;
                }
                Err(error) => {
                    println!("  ⚠️  Could not remove file {file}: {error}");
                }
            }
        }
    }

    // Phase 2: remove dirs tracked in manifest (deepest-first to avoid removing a parent
    // before its children, which would cause remove_dir_all to fail on the children).
    let mut dirs_sorted = manifest.dirs.clone();
    dirs_sorted.sort_unstable(); // NOTE: dedup() only removes adjacent duplicates — sort must precede it
    dirs_sorted.dedup();
    for dir in dirs_sorted.iter().rev() {
        let target = claude_dir.join(dir);
        if target.is_dir() && fs::remove_dir_all(&target).is_ok() {
            removed_any = true;
        }
    }

    let mut removed_dirs = 0usize;
    for dir in ["agents/amplihack", "commands/amplihack", "tools/amplihack"] {
        let target = claude_dir.join(dir);
        if target.exists() {
            match fs::remove_dir_all(&target) {
                Ok(()) => {
                    removed_any = true;
                    removed_dirs += 1;
                }
                Err(error) => {
                    println!("  ⚠️  Could not remove {}: {}", target.display(), error);
                }
            }
        }
    }

    // Phase 3: remove binaries listed in manifest
    for binary_path in &manifest.binaries {
        let p = PathBuf::from(binary_path);
        if p.is_file() {
            match fs::remove_file(&p) {
                Ok(()) => {
                    removed_any = true;
                    println!("  🗑️  Removed binary {}", p.display());
                }
                Err(error) => {
                    println!("  ⚠️  Could not remove binary {}: {error}", p.display());
                }
            }
        }
    }

    // Phase 4: remove hook registrations from ~/.claude/settings.json
    let global_settings = global_settings_path()?;
    if global_settings.exists() && !manifest.hook_registrations.is_empty() {
        if let Err(e) = remove_hook_registrations(&global_settings) {
            println!("  ⚠️  Could not clean hook registrations: {e}");
        } else {
            println!("  ✅ Hook registrations removed from settings.json");
        }
    }

    let _ = fs::remove_file(&manifest_path);

    if removed_any {
        println!("✅ Uninstalled amplihack from {}", claude_dir.display());
        if removed_files > 0 {
            println!("   • Removed {removed_files} files");
        }
        if removed_dirs > 0 {
            println!("   • Removed {removed_dirs} amplihack directories");
        }
    } else {
        println!("Nothing to uninstall.");
    }

    Ok(())
}

/// Remove amplihack hook registrations from settings.json.
/// Removes wrappers whose command contains `amplihack-hooks` or `tools/amplihack/`.
/// Preserves XPIA and all other non-amplihack entries.
fn remove_hook_registrations(settings_path: &Path) -> Result<()> {
    let mut settings = read_settings_json(settings_path)?;
    let root = ensure_object(&mut settings);
    if let Some(hooks_val) = root.get_mut("hooks")
        && let Some(hooks_map) = hooks_val.as_object_mut()
    {
        for (_event, wrappers_val) in hooks_map.iter_mut() {
            if let Some(wrappers) = wrappers_val.as_array_mut() {
                wrappers.retain(|wrapper| {
                    // Keep wrapper if none of its hooks reference amplihack
                    let hooks = wrapper.get("hooks").and_then(Value::as_array);
                    let Some(hooks) = hooks else {
                        return true;
                    };
                    let is_amplihack = hooks.iter().any(|hook| {
                        hook.get("command")
                            .and_then(Value::as_str)
                            .map(|cmd| {
                                cmd.contains("amplihack-hooks") || cmd.contains("tools/amplihack/")
                            })
                            .unwrap_or(false)
                    });
                    !is_amplihack
                });
            }
        }
        // Phase 2: prune event-type keys where every amplihack wrapper was removed,
        // leaving no empty arrays in settings.json (fixes issue #38).
        // Non-array values (unlikely but possible) are kept via the unwrap_or(true) guard.
        hooks_map.retain(|_event, wrappers_val| {
            wrappers_val
                .as_array()
                .map(|a| !a.is_empty())
                .unwrap_or(true)
        });
    }

    fs::write(
        settings_path,
        serde_json::to_string_pretty(&settings)? + "\n",
    )
    .with_context(|| format!("failed to write {}", settings_path.display()))
}

fn local_install(repo_root: &Path) -> Result<()> {
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
    let source_claude = find_source_claude_dir(repo_root)?;
    let copied_dirs = copytree_manifest(&source_claude, &claude_dir)?;
    if copied_dirs.is_empty() {
        println!();
        println!("❌ No directories were copied. Installation may be incomplete.");
        println!("   Please check that the source repository is valid.");
        println!();
        return Ok(());
    }

    println!();
    println!("📝 Initializing PROJECT.md:");
    initialize_project_md(&claude_dir)?;

    println!();
    println!("📂 Creating runtime directories:");
    create_runtime_dirs(&claude_dir)?;

    println!();
    println!("⚙️  Configuring settings.json:");
    let (settings_ok, registered_events) =
        ensure_settings_json(&claude_dir, timestamp, &hooks_bin)?;

    println!();
    println!("🔍 Verifying staged framework assets:");
    let hooks_ok = verify_framework_assets(&claude_dir)?;

    println!();
    println!("🦀 Ensuring Rust recipe runner:");
    if paths::find_binary("recipe-runner-rs").is_some() {
        println!("   ✅ recipe-runner-rs is available");
    } else {
        println!("   ❌ recipe-runner-rs not installed (recipe execution will fail without it)");
        println!(
            "   Install: cargo install --git https://github.com/rysweet/amplihack-recipe-runner"
        );
    }

    println!();
    println!("📝 Generating uninstall manifest:");
    let manifest_path = manifest_path()?;
    let mut tracked_roots = Vec::new();
    for dir in ESSENTIAL_DIRS {
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

    let manifest = InstallManifest {
        files,
        dirs: new_dirs,
        binaries: deployed_binaries
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
        hook_registrations: registered_events,
    };
    write_manifest(&manifest_path, &manifest)?;
    println!("   Manifest written to {}", manifest_path.display());

    println!();
    println!("============================================================");
    if settings_ok && hooks_ok && !copied_dirs.is_empty() {
        println!("✅ Amplihack installation completed successfully!");
        println!();
        println!("📍 Installed to: {}", claude_dir.display());
        println!();
        println!("📦 Components installed:");
        for dir in &copied_dirs {
            println!("   • {dir}");
        }
        println!();
        println!("🎯 Features enabled:");
        println!("   • Session start hook");
        println!("   • Stop hook");
        println!("   • Post-tool-use hook");
        println!("   • Pre-compact hook");
        println!("   • Runtime logging and metrics");
        println!();
        println!("💡 To uninstall: amplihack uninstall");
    } else {
        println!("⚠️  Installation completed with warnings");
        if !settings_ok {
            println!("   • Settings.json configuration had issues");
        }
        if !hooks_ok {
            println!("   • Some staged framework assets are missing");
        }
        if copied_dirs.is_empty() {
            println!("   • No directories were copied");
        }
        println!();
        println!("💡 You may need to manually verify the installation");
    }
    println!("============================================================");
    println!();

    Ok(())
}
