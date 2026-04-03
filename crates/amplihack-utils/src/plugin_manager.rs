//! Plugin manager: install, uninstall, list, and path resolution.
//!
//! Ported from `amplihack/plugin_manager/manager.py`.
//!
//! The [`PluginManager`] struct manages the full plugin lifecycle:
//!
//! - **Install** from a git URL or local directory path.
//! - **Uninstall** by plugin name.
//! - **List** all installed plugins.
//! - **Resolve paths** in manifest dictionaries (with path-traversal detection).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::plugin_manifest::{is_valid_plugin_name, validate_manifest};
use crate::plugin_manager_paths::{
    copy_dir_recursive, extract_plugin_name_from_url, home_dir, resolve_paths_inner,
    validate_path_safety,
};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors produced by plugin manager operations.
#[derive(Debug, Error)]
pub enum PluginManagerError {
    /// An I/O error occurred during plugin operations.
    #[error("plugin I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization / deserialization failure.
    #[error("plugin JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// A git clone command failed.
    #[error("git clone failed (exit {code:?}): {stderr}")]
    GitCloneFailed {
        /// Exit code from git, if available.
        code: Option<i32>,
        /// Captured stderr.
        stderr: String,
    },

    /// Path traversal was detected.
    #[error("path traversal detected: {path} escapes {base}")]
    PathTraversal {
        /// The offending path.
        path: String,
        /// The base directory.
        base: String,
    },
}

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

/// Result of a plugin installation attempt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstallResult {
    /// Whether installation succeeded.
    pub success: bool,
    /// Name of the plugin (extracted from URL or directory).
    pub plugin_name: String,
    /// Where the plugin was installed (empty path on failure).
    pub installed_path: PathBuf,
    /// Human-readable status message.
    pub message: String,
}

// ---------------------------------------------------------------------------
// PluginManager
// ---------------------------------------------------------------------------

/// Manages plugin installation, validation, and removal.
///
/// # Example
///
/// ```no_run
/// use amplihack_utils::plugin_manager::PluginManager;
/// use std::path::PathBuf;
///
/// let mgr = PluginManager::new(Some(PathBuf::from("/my/plugins")));
/// let result = mgr.install("/path/to/local-plugin", false);
/// println!("{}", result.message);
/// ```
pub struct PluginManager {
    plugin_root: PathBuf,
    settings_path: PathBuf,
}

impl PluginManager {
    /// Create a new `PluginManager`.
    ///
    /// If `plugin_root` is `None`, defaults to `~/.amplihack/.claude/plugins`.
    pub fn new(plugin_root: Option<PathBuf>) -> Self {
        let home = home_dir();
        let plugin_root = plugin_root
            .unwrap_or_else(|| home.join(".amplihack").join(".claude").join("plugins"));
        let settings_path = home
            .join(".config")
            .join("claude-code")
            .join("plugins.json");
        Self { plugin_root, settings_path }
    }

    /// Create a manager with explicit paths (useful for testing).
    pub fn with_paths(plugin_root: PathBuf, settings_path: PathBuf) -> Self {
        Self { plugin_root, settings_path }
    }

    /// Root directory for installed plugins.
    pub fn plugin_root(&self) -> &Path {
        &self.plugin_root
    }

    /// Install a plugin from a git URL or local directory path.
    ///
    /// Git URLs start with `http://`, `https://`, or `git@`.
    /// Set `force` to overwrite an existing installation.
    pub fn install(&self, source: &str, force: bool) -> InstallResult {
        if source.is_empty() {
            return fail_result("", "Empty source provided");
        }

        let is_git = source.starts_with("http://")
            || source.starts_with("https://")
            || source.starts_with("git@");

        if is_git {
            self.install_from_git(source, force)
        } else {
            self.install_from_local(source, force)
        }
    }

    fn install_from_git(&self, url: &str, force: bool) -> InstallResult {
        let plugin_name = extract_plugin_name_from_url(url);

        if !is_valid_plugin_name(&plugin_name) {
            return fail_result(&plugin_name, "Invalid plugin name from URL");
        }

        let staging_root = self.plugin_root.join(".staging");
        if let Err(e) = std::fs::create_dir_all(&staging_root) {
            return fail_result(&plugin_name, &format!("Failed to create staging dir: {e}"));
        }

        let clone_target = staging_root.join(&plugin_name);
        if clone_target.exists() {
            let _ = std::fs::remove_dir_all(&clone_target);
        }

        let git_result = std::process::Command::new("git")
            .args(["clone", url, &clone_target.to_string_lossy()])
            .output();

        let output = match git_result {
            Ok(o) => o,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&staging_root);
                return fail_result(&plugin_name, &format!("Failed to run git clone: {e}"));
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let _ = std::fs::remove_dir_all(&staging_root);
            return fail_result(&plugin_name, &format!("Git clone failed: {stderr}"));
        }

        let result = self.finalize_install(&clone_target, &plugin_name, force);
        let _ = std::fs::remove_dir_all(&staging_root);
        result
    }

    fn install_from_local(&self, source: &str, force: bool) -> InstallResult {
        let source_path = PathBuf::from(source);

        if !source_path.is_dir() {
            return fail_result("", &format!("Source must be a directory: {source}"));
        }

        let plugin_name = source_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        self.finalize_install(&source_path, &plugin_name, force)
    }

    fn finalize_install(
        &self,
        source_path: &Path,
        plugin_name: &str,
        force: bool,
    ) -> InstallResult {
        let manifest_path = source_path.join(".claude-plugin").join("plugin.json");

        if !validate_path_safety(&manifest_path, source_path) {
            return fail_result(plugin_name, "Manifest path traversal detected");
        }

        let validation = validate_manifest(&manifest_path);
        if !validation.valid {
            let msg = format!("Invalid manifest: {}", validation.errors.join(", "));
            return fail_result(plugin_name, &msg);
        }

        let target_path = self.plugin_root.join(plugin_name);

        if target_path.exists() && !force {
            return InstallResult {
                success: false,
                plugin_name: plugin_name.to_string(),
                installed_path: target_path,
                message: format!(
                    "Plugin already installed: {plugin_name} (use force=true to overwrite)"
                ),
            };
        }

        if target_path.exists()
            && let Err(e) = std::fs::remove_dir_all(&target_path)
        {
            let msg = format!("Failed to remove existing plugin: {e}");
            return fail_result(plugin_name, &msg);
        }

        if let Err(e) = std::fs::create_dir_all(&self.plugin_root) {
            return fail_result(plugin_name, &format!("Failed to create plugin dir: {e}"));
        }

        if let Err(e) = copy_dir_recursive(source_path, &target_path) {
            return fail_result(plugin_name, &format!("Failed to copy plugin files: {e}"));
        }

        if let Err(e) = self.register_plugin(plugin_name) {
            return InstallResult {
                success: false,
                plugin_name: plugin_name.to_string(),
                installed_path: target_path,
                message: format!("Plugin copied but registration failed: {e}"),
            };
        }

        InstallResult {
            success: true,
            plugin_name: plugin_name.to_string(),
            installed_path: target_path,
            message: format!("Plugin installed successfully: {plugin_name}"),
        }
    }

    /// Remove an installed plugin by name.
    ///
    /// Returns `true` if the plugin was found and removed.
    pub fn uninstall(&self, plugin_name: &str) -> bool {
        let plugin_path = self.plugin_root.join(plugin_name);
        if !plugin_path.exists() {
            return false;
        }
        std::fs::remove_dir_all(&plugin_path).is_ok()
    }

    /// List all installed plugin names.
    ///
    /// Scans the plugin root for directories containing a manifest.
    pub fn list_installed(&self) -> Vec<String> {
        let mut plugins = Vec::new();
        let entries = match std::fs::read_dir(&self.plugin_root) {
            Ok(e) => e,
            Err(_) => return plugins,
        };
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let manifest = entry.path().join(".claude-plugin").join("plugin.json");
            if manifest.exists()
                && let Some(name) = entry.file_name().to_str()
            {
                plugins.push(name.to_string());
            }
        }
        plugins.sort();
        plugins
    }

    /// Resolve relative paths in a manifest JSON map to absolute paths.
    ///
    /// Fields listed in [`PATH_FIELDS`](crate::plugin_manifest::PATH_FIELDS)
    /// are resolved relative to `plugin_path` (or the plugin root if `None`).
    pub fn resolve_paths(
        &self,
        manifest: &serde_json::Map<String, serde_json::Value>,
        plugin_path: Option<&Path>,
    ) -> Result<serde_json::Map<String, serde_json::Value>, PluginManagerError> {
        let base = plugin_path.unwrap_or(&self.plugin_root);
        resolve_paths_inner(manifest, base)
    }

    /// Register plugin name in the Claude Code settings file.
    fn register_plugin(&self, plugin_name: &str) -> Result<(), PluginManagerError> {
        if let Some(parent) = self.settings_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut settings: serde_json::Value = if self.settings_path.exists() {
            let text = std::fs::read_to_string(&self.settings_path)?;
            if text.trim().is_empty() {
                serde_json::json!({})
            } else {
                serde_json::from_str(&text)?
            }
        } else {
            serde_json::json!({})
        };

        let enabled = settings.as_object_mut().and_then(|o| {
            o.entry("enabledPlugins")
                .or_insert(serde_json::json!([]))
                .as_array_mut()
        });

        if let Some(arr) = enabled {
            let name_val = serde_json::Value::String(plugin_name.to_string());
            if !arr.contains(&name_val) {
                arr.push(name_val);
            }
        }

        let text = serde_json::to_string_pretty(&settings)?;
        std::fs::write(&self.settings_path, text)?;
        Ok(())
    }
}

/// Helper to construct a failure `InstallResult`.
fn fail_result(plugin_name: &str, message: &str) -> InstallResult {
    InstallResult {
        success: false,
        plugin_name: plugin_name.to_string(),
        installed_path: PathBuf::new(),
        message: message.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "tests/plugin_manager_tests.rs"]
mod tests;
