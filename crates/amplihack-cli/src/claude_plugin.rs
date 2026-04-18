//! Claude Code plugin registration for amplihack.
//!
//! When the user launches `amplihack claude`, we install amplihack as a
//! first-class Claude Code plugin at `~/.claude/plugins/amplihack/` and
//! register it in `~/.config/claude-code/plugins.json`. Without this,
//! Claude Code never sees amplihack's agents, skills, or commands — only
//! the hooks registered in `~/.claude/settings.json` would fire.
//!
//! The plugin directory is built from the staged framework under
//! `~/.amplihack/.claude/` (populated by `amplihack install`). We use
//! symlinks when possible so subsequent framework updates are picked up
//! automatically, and fall back to copies when symlinks fail (e.g. on
//! Windows without developer mode, or across filesystem boundaries).

use amplihack_state::AtomicJsonFile;
use anyhow::{Context, Result};
use serde_json::{Map, Value, json};
use std::fs;
use std::path::{Path, PathBuf};

const PLUGIN_NAME: &str = "amplihack";
const PLUGIN_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Top-level plugin assets mirrored from the staged framework dir into the
/// Claude Code plugin dir. Only asset types Claude Code discovers need to
/// be listed here.
const MIRRORED_ASSETS: &[&str] = &["agents", "skills", "commands", "context", "workflow"];

/// Ensure amplihack is installed as a Claude Code plugin.
///
/// Idempotent: safe to call on every launcher start. Errors are converted
/// to warnings and do not abort the launch — a failed plugin install should
/// not block the user from running Claude.
pub fn ensure_claude_plugin_installed() -> Result<()> {
    let staged = staged_framework_dir()?;
    if !staged.is_dir() {
        tracing::debug!(
            path = %staged.display(),
            "staged amplihack framework not found; skipping Claude plugin install"
        );
        return Ok(());
    }
    let plugin_dir = plugin_install_dir()?;
    fs::create_dir_all(&plugin_dir)
        .with_context(|| format!("failed to create {}", plugin_dir.display()))?;

    write_plugin_manifest(&plugin_dir)?;
    mirror_assets(&staged, &plugin_dir)?;
    register_plugin_in_settings()?;
    Ok(())
}

fn staged_framework_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(".amplihack").join(".claude"))
}

fn plugin_install_dir() -> Result<PathBuf> {
    Ok(home_dir()?
        .join(".claude")
        .join("plugins")
        .join(PLUGIN_NAME))
}

fn plugins_json_path() -> Result<PathBuf> {
    Ok(home_dir()?
        .join(".config")
        .join("claude-code")
        .join("plugins.json"))
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .context("HOME is not set")
}

/// Write `.claude-plugin/plugin.json`. Rewritten every launch so the version
/// stays in sync with the installed amplihack binary.
fn write_plugin_manifest(plugin_dir: &Path) -> Result<()> {
    let manifest_dir = plugin_dir.join(".claude-plugin");
    fs::create_dir_all(&manifest_dir)
        .with_context(|| format!("failed to create {}", manifest_dir.display()))?;
    let manifest_path = manifest_dir.join("plugin.json");
    let manifest = json!({
        "name": PLUGIN_NAME,
        "version": PLUGIN_VERSION,
        "description": "Amplihack AI development framework — agents, skills, and commands.",
        "author": "Microsoft",
    });
    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest)? + "\n",
    )
    .with_context(|| format!("failed to write {}", manifest_path.display()))?;
    Ok(())
}

/// Mirror each asset directory from the staged framework into the plugin
/// dir. Symlink when possible, fall back to a recursive copy otherwise.
///
/// The agents layout needs a special note: Claude Code expects flat files
/// under `agents/` (one markdown per agent). The staged framework puts
/// them under `agents/amplihack/<category>/...`. We mirror the `amplihack`
/// subdirectory directly so `~/.claude/plugins/amplihack/agents/<...>`
/// matches Claude Code's discovery pattern.
fn mirror_assets(staged: &Path, plugin_dir: &Path) -> Result<()> {
    for asset in MIRRORED_ASSETS {
        let source = resolve_asset_source(staged, asset);
        let target = plugin_dir.join(asset);
        if !source.is_dir() {
            continue;
        }

        // Remove any existing target (stale symlink, stale copy, or a
        // leftover from an older amplihack version). remove_dir_all also
        // removes symlinks in both stdlib implementations.
        if target.exists() || target.symlink_metadata().is_ok() {
            let _ = fs::remove_file(&target);
            let _ = fs::remove_dir_all(&target);
        }

        if try_symlink(&source, &target).is_ok() {
            continue;
        }
        copy_dir_recursive(&source, &target)?;
    }
    Ok(())
}

/// Resolve the staged source directory for a given asset type, accounting
/// for the amplihack-specific subdirectory layout inside agents/commands.
fn resolve_asset_source(staged: &Path, asset: &str) -> PathBuf {
    // Staged framework nests its own content under `<asset>/amplihack/`.
    // Claude Code plugin dirs want the content directly under `<asset>/`,
    // so we prefer the amplihack-scoped subdir when it exists.
    let scoped = staged.join(asset).join("amplihack");
    if scoped.is_dir() {
        return scoped;
    }
    staged.join(asset)
}

#[cfg(unix)]
fn try_symlink(source: &Path, target: &Path) -> Result<()> {
    std::os::unix::fs::symlink(source, target)
        .with_context(|| format!("symlink {} -> {}", target.display(), source.display()))
}

#[cfg(windows)]
fn try_symlink(source: &Path, target: &Path) -> Result<()> {
    std::os::windows::fs::symlink_dir(source, target)
        .with_context(|| format!("symlink {} -> {}", target.display(), source.display()))
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst).with_context(|| format!("failed to create {}", dst.display()))?;
    for entry in fs::read_dir(src).with_context(|| format!("failed to read {}", src.display()))? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let source = entry.path();
        let target = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&source, &target)?;
        } else if file_type.is_file() {
            fs::copy(&source, &target).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    source.display(),
                    target.display()
                )
            })?;
        }
        // Symlinks inside the staged framework are skipped deliberately —
        // they point back into the framework source and don't need to be
        // re-exposed through the plugin dir.
    }
    Ok(())
}

/// Add `"amplihack"` to `enabledPlugins` in `~/.config/claude-code/plugins.json`.
/// Uses the shared atomic-JSON writer so concurrent claude launches do not
/// clobber each other.
fn register_plugin_in_settings() -> Result<()> {
    let settings_path = plugins_json_path()?;
    if let Some(parent) = settings_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let file = AtomicJsonFile::new(&settings_path);
    file.update(|settings: &mut Value| {
        if !settings.is_object() {
            *settings = Value::Object(Map::new());
        }
        let object = settings.as_object_mut().expect("object just created");
        let plugins = object
            .entry("enabledPlugins")
            .or_insert_with(|| Value::Array(vec![]));
        if !plugins.is_array() {
            *plugins = Value::Array(vec![]);
        }
        let list = plugins.as_array_mut().expect("array just created");
        if !list.iter().any(|value| value.as_str() == Some(PLUGIN_NAME)) {
            list.push(Value::String(PLUGIN_NAME.to_string()));
        }
    })
    .with_context(|| format!("failed to update {}", settings_path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_asset_source_prefers_scoped_subdir() {
        let temp = tempfile::tempdir().unwrap();
        let staged = temp.path();
        fs::create_dir_all(staged.join("agents/amplihack")).unwrap();
        let src = resolve_asset_source(staged, "agents");
        assert_eq!(src, staged.join("agents/amplihack"));
    }

    #[test]
    fn resolve_asset_source_falls_back_to_flat_dir() {
        let temp = tempfile::tempdir().unwrap();
        let staged = temp.path();
        fs::create_dir_all(staged.join("skills")).unwrap();
        let src = resolve_asset_source(staged, "skills");
        assert_eq!(src, staged.join("skills"));
    }

    #[test]
    fn ensure_claude_plugin_installed_is_noop_without_staging() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous_home = crate::test_support::set_home(temp.path());
        ensure_claude_plugin_installed().unwrap();
        // No plugin dir should be created when staging is absent.
        assert!(!temp.path().join(".claude/plugins/amplihack").exists());
        crate::test_support::restore_home(previous_home);
    }

    #[test]
    fn ensure_claude_plugin_installed_registers_and_mirrors_assets() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous_home = crate::test_support::set_home(temp.path());

        let staged = temp.path().join(".amplihack/.claude");
        fs::create_dir_all(staged.join("agents/amplihack/core")).unwrap();
        fs::create_dir_all(staged.join("skills/dev-orchestrator")).unwrap();
        fs::write(staged.join("agents/amplihack/core/architect.md"), "agent").unwrap();
        fs::write(staged.join("skills/dev-orchestrator/SKILL.md"), "skill").unwrap();

        ensure_claude_plugin_installed().unwrap();

        let plugin_dir = temp.path().join(".claude/plugins/amplihack");
        assert!(plugin_dir.join(".claude-plugin/plugin.json").is_file());
        // agents mirror the scoped subdir
        assert!(plugin_dir.join("agents/core/architect.md").exists());
        // skills mirror the flat dir
        assert!(plugin_dir.join("skills/dev-orchestrator/SKILL.md").exists());

        let plugins_json = temp.path().join(".config/claude-code/plugins.json");
        let settings: Value =
            serde_json::from_str(&fs::read_to_string(&plugins_json).unwrap()).unwrap();
        let enabled = settings
            .get("enabledPlugins")
            .and_then(Value::as_array)
            .unwrap();
        assert!(enabled.iter().any(|v| v.as_str() == Some("amplihack")));

        crate::test_support::restore_home(previous_home);
    }
}
