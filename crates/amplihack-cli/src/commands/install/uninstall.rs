//! Native uninstall command.

use super::claude_commands::{claude_commands_dir, claude_commands_scratch_dir};
use super::hooks::ensure_object;
use super::manifest::{manifest_path, read_manifest};
use super::paths::{global_settings_path, staging_amplifier_bundle_dir, staging_claude_dir};
use super::release_layout;
use super::settings::read_settings_json;
use crate::claude_plugin::CommandRemoval;
use crate::commands::profile::is_amplihack_statusline;
use anyhow::{Context, Result};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// Remove `~/.claude/commands/amplihack/` if — and only if — it still matches
/// the ownership digest recorded at install time.
fn remove_claude_slash_commands() -> Result<(CommandRemoval, PathBuf)> {
    let target = claude_commands_dir()?;
    let manifest = crate::claude_plugin::ownership_manifest_path(&staging_claude_dir()?);
    let outcome = crate::claude_plugin::remove_managed_claude_commands(&manifest, &target)?;
    Ok((outcome, target))
}

pub fn run_uninstall() -> Result<()> {
    // Issue #1954 D14: uninstall changes the same paths an install does.
    let _install_lock = crate::install_lock::acquire()?;
    super::codex_plugin::uninstall()?;
    let claude_dir = staging_claude_dir()?;
    let manifest_path = manifest_path()?;
    let manifest = read_manifest(&manifest_path)?;

    crate::claude_plugin::remove_managed_claude_plugin()
        .context("failed to clean verified Claude plugin destinations before ownership state")?;

    let mut removed_any = false;
    let mut removed_files = 0usize;
    let mut removed_dirs = 0usize;

    // Issue #1344: install stages the slash commands into Claude's own
    // discovery directory, so uninstall has to take them back out — otherwise
    // `/amplihack:lock` keeps resolving after the framework is gone.
    //
    // Unlike the `claude_dir`-rooted loops below — `~/.amplihack/.claude` is
    // amplihack-private, so blind removal there is fine — this target lives
    // under the *user's* `~/.claude`, where a `my-thing.md` of their own is
    // perfectly legitimate. So it goes through the same verified-removal
    // contract as the managed Claude plugin destinations: remove only while
    // the tree still hashes to what install recorded, otherwise leave it alone
    // and say so. It runs first because the ownership record it consults lives
    // under `~/.amplihack/.claude`, which those loops are about to delete.
    match remove_claude_slash_commands() {
        Ok((CommandRemoval::Removed, path)) => {
            removed_any = true;
            removed_dirs += 1;
            println!("  🗑️  Removed Claude slash commands at {}", path.display());
        }
        Ok((CommandRemoval::Preserved, path)) => {
            println!(
                "  ⚠️  Kept {} — its contents no longer match what amplihack staged, \
                 so it may hold commands of your own",
                path.display()
            );
        }
        Ok((CommandRemoval::Absent, _)) => {}
        Err(error) => {
            println!("  ⚠️  Could not remove Claude slash commands: {error:#}");
        }
    }
    if let Ok(scratch) = claude_commands_scratch_dir()
        && scratch.exists()
    {
        // amplihack-private scratch root; an orphan here is left by an
        // interrupted install and is never anything but ours.
        let _ = fs::remove_dir_all(&scratch);
    }

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
        && bundle.symlink_metadata().is_ok()
    {
        match remove_published_bundle(&bundle) {
            Ok(()) => {
                removed_any = true;
                removed_dirs += 1;
                println!("  🗑️  Removed amplifier-bundle at {}", bundle.display());
            }
            Err(error) => {
                println!(
                    "  ⚠️  Could not remove amplifier-bundle at {}: {error:#}",
                    bundle.display(),
                );
            }
        }
    }

    for binary_path in &manifest.binaries {
        let p = PathBuf::from(binary_path);
        // A link into a release (issue #1954) is removed as a link, even when
        // its release is already gone; a legacy regular file is unlinked,
        // which a process still running it does not notice.
        let is_link = p
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink());
        if is_link || p.is_file() {
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

    // Not tied to the hook registrations: an install can write this entry
    // with no hooks to record, and the binary it names has just gone.
    if global_settings.exists() {
        match remove_status_line(&global_settings) {
            Ok(true) => {
                removed_any = true;
                println!("  ✅ statusLine entry removed from settings.json");
            }
            Ok(false) => {}
            Err(e) => println!("  ⚠️  Could not clean the statusLine entry: {e}"),
        }
    }

    let _ = fs::remove_file(&manifest_path);

    // Issue #1954: with every link gone, every release and retired tree is
    // removed unless a running process still uses it.
    if let Some(amplihack_dir) = claude_dir.parent() {
        match remove_unused_releases(amplihack_dir) {
            Ok(removed) => removed_any |= removed,
            Err(error) => println!("  ⚠️  Could not clean up amplihack releases: {error:#}"),
        }
    }

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

/// Issue #1954 S18: the published bundle is removed only when it is a link.
/// A real directory there (a pre-#1954 install) is retired, not deleted, so
/// a run working inside it keeps its files until cleanup finds it unused.
fn remove_published_bundle(bundle: &Path) -> Result<()> {
    let meta = bundle
        .symlink_metadata()
        .with_context(|| format!("failed to inspect {}", bundle.display()))?;
    if meta.file_type().is_symlink() {
        return fs::remove_file(bundle)
            .with_context(|| format!("failed to remove link {}", bundle.display()));
    }
    let amplihack_dir = bundle
        .parent()
        .with_context(|| format!("{} has no parent directory", bundle.display()))?;
    match release_layout::retire(amplihack_dir, bundle, "amplifier-bundle")? {
        release_layout::Retired::Moved(_) => Ok(()),
        release_layout::Retired::LeftInPlace(at) => anyhow::bail!(
            "{} is on another filesystem than the releases directory; left in place",
            at.display()
        ),
    }
}

/// Runs release cleanup with nothing kept but what processes use, prints the
/// releases kept, and removes `releases/` once it is empty. Returns whether
/// anything was removed.
fn remove_unused_releases(amplihack_dir: &Path) -> Result<bool> {
    let releases = release_layout::releases_dir(amplihack_dir);
    if releases.symlink_metadata().is_err() {
        return Ok(false);
    }
    let report = release_layout::cleanup_unused(amplihack_dir, &[])?;
    for path in &report.removed {
        println!("  🗑️  Removed {}", path.display());
    }
    for (path, reason) in &report.failed {
        println!("  ⚠️  Could not remove {}: {reason}", path.display());
    }
    if report.unknown {
        println!("  {}", release_layout::unknown_scan_notice());
    }
    if report.unsupported {
        println!("  {}", release_layout::unsupported_scan_notice());
    }
    if let Ok(home) = super::paths::home_dir()
        && let Some(kept) = release_layout::format_kept_in_use(&home, &report.kept_in_use)
    {
        println!("{kept}");
    }
    // Only empty directories go; anything kept stays with its parents.
    for dir in [releases.join(release_layout::RETIRED_DIR), releases] {
        match fs::remove_dir(&dir) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                ) => {}
            Err(error) => println!("  ⚠️  Could not remove {}: {error}", dir.display()),
        }
    }
    Ok(!report.removed.is_empty())
}

/// Removes `statusLine` from `settings_path` when it is amplihack's (issue
/// #1745), and says whether it did. Any other entry is the user's: the file
/// is left as it was.
pub(crate) fn remove_status_line(settings_path: &Path) -> Result<bool> {
    let mut settings = read_settings_json(settings_path)?;
    let root = ensure_object(&mut settings);
    let ours = root
        .get("statusLine")
        .and_then(|sl| sl.get("command"))
        .and_then(Value::as_str)
        .is_some_and(is_amplihack_statusline);
    if !ours {
        return Ok(false);
    }
    root.remove("statusLine");
    fs::write(
        settings_path,
        serde_json::to_string_pretty(&settings)? + "\n",
    )
    .with_context(|| format!("failed to write {}", settings_path.display()))?;
    Ok(true)
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
