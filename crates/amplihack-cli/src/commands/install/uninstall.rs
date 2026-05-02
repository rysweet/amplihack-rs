//! Native uninstall command.

use super::hooks::ensure_object;
use super::manifest::{manifest_path, read_manifest};
use super::paths::{global_settings_path, staging_amplifier_bundle_dir, staging_claude_dir};
use super::settings::read_settings_json;
use anyhow::{Context, Result};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

pub fn run_uninstall() -> Result<()> {
    let claude_dir = staging_claude_dir()?;
    let manifest_path = manifest_path()?;
    let manifest = read_manifest(&manifest_path)?;

    let mut removed_any = false;
    let mut removed_files = 0usize;

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

    let mut dirs_sorted = manifest.dirs.clone();
    dirs_sorted.sort_unstable();
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

    if let Ok(bundle) = staging_amplifier_bundle_dir()
        && bundle.exists()
    {
        match fs::remove_dir_all(&bundle) {
            Ok(()) => {
                removed_any = true;
                removed_dirs += 1;
                println!("  🗑️  Removed amplifier-bundle at {}", bundle.display());
            }
            Err(error) => {
                println!(
                    "  ⚠️  Could not remove amplifier-bundle at {}: {}",
                    bundle.display(),
                    error
                );
            }
        }
    }

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

pub(crate) fn remove_hook_registrations(settings_path: &Path) -> Result<()> {
    let mut settings = read_settings_json(settings_path)?;
    let root = ensure_object(&mut settings);
    if let Some(hooks_val) = root.get_mut("hooks")
        && let Some(hooks_map) = hooks_val.as_object_mut()
    {
        for (_event, wrappers_val) in hooks_map.iter_mut() {
            if let Some(wrappers) = wrappers_val.as_array_mut() {
                wrappers.retain(|wrapper| {
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
