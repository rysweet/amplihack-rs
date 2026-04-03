//! Plugin verification module.
//!
//! Ported from `amplihack/plugin_cli/verifier.py`.
//!
//! Three-layer verification:
//! 1. **Installed** – plugin directory and manifest exist.
//! 2. **Discoverable** – plugin is listed in the Claude Code settings file.
//! 3. **Hooks loaded** – `hooks.json` exists and contains at least one hook.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Result of plugin verification.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationResult {
    /// Overall success (all three layers passed).
    pub success: bool,
    /// Plugin directory and manifest exist.
    pub installed: bool,
    /// Plugin found in Claude Code settings.
    pub discoverable: bool,
    /// `hooks.json` exists with at least one hook.
    pub hooks_loaded: bool,
    /// Human-readable diagnostics for any failures.
    pub issues: Vec<String>,
}

/// Verify plugin installation and discoverability.
///
/// # Example
///
/// ```no_run
/// use amplihack_utils::plugin_verifier::PluginVerifier;
///
/// let verifier = PluginVerifier::new("my-plugin");
/// let result = verifier.verify();
/// if !result.success {
///     for issue in &result.issues {
///         eprintln!("  ⚠ {issue}");
///     }
/// }
/// ```
pub struct PluginVerifier {
    plugin_name: String,
    plugin_root: PathBuf,
    settings_path: PathBuf,
}

impl PluginVerifier {
    /// Create a verifier for the given plugin name using default paths.
    pub fn new(plugin_name: &str) -> Self {
        let home = home_dir();
        let plugin_root = home
            .join(".amplihack")
            .join(".claude")
            .join("plugins")
            .join(plugin_name);
        let settings_path = home.join(".claude").join("settings.json");
        Self {
            plugin_name: plugin_name.to_owned(),
            plugin_root,
            settings_path,
        }
    }

    /// Create a verifier with explicit paths (useful for testing).
    pub fn with_paths(
        plugin_name: &str,
        plugin_root: PathBuf,
        settings_path: PathBuf,
    ) -> Self {
        Self {
            plugin_name: plugin_name.to_owned(),
            plugin_root,
            settings_path,
        }
    }

    /// Run all three verification layers.
    pub fn verify(&self) -> VerificationResult {
        let mut issues = Vec::new();

        let installed = self.check_installed();
        if !installed {
            issues.push(format!(
                "Plugin directory not found: {}",
                self.plugin_root.display()
            ));
        }

        let discoverable = self.check_discoverable();
        if !discoverable {
            issues.push(format!(
                "Plugin not found in {}",
                self.settings_path.display()
            ));
        }

        let hooks_loaded = self.check_hooks_loaded();
        if !hooks_loaded {
            issues.push("Hooks not registered or hooks.json missing".into());
        }

        let success = installed && discoverable && hooks_loaded;

        VerificationResult {
            success,
            installed,
            discoverable,
            hooks_loaded,
            issues,
        }
    }

    /// Check whether the plugin directory and its manifest exist.
    pub fn check_installed(&self) -> bool {
        let manifest = self
            .plugin_root
            .join(".claude-plugin")
            .join("plugin.json");
        self.plugin_root.exists() && manifest.exists()
    }

    /// Check whether the plugin appears in the Claude Code settings file.
    pub fn check_discoverable(&self) -> bool {
        read_enabled_plugins(&self.settings_path)
            .map(|list| list.contains(&self.plugin_name))
            .unwrap_or(false)
    }

    /// Check whether `hooks.json` exists and contains at least one hook.
    pub fn check_hooks_loaded(&self) -> bool {
        let hooks_json = self
            .plugin_root
            .join(".claude")
            .join("tools")
            .join("amplihack")
            .join("hooks")
            .join("hooks.json");

        read_json_array_len(&hooks_json)
            .map(|len| len > 0)
            .unwrap_or(false)
    }
}

/// Read the `enabledPlugins` array from a Claude Code settings file.
fn read_enabled_plugins(path: &Path) -> Option<Vec<String>> {
    let content = std::fs::read_to_string(path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&content).ok()?;
    let arr = val.get("enabledPlugins")?.as_array()?;
    Some(
        arr.iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
    )
}

/// Read a JSON file and return the top-level array length.
fn read_json_array_len(path: &Path) -> Option<usize> {
    let content = std::fs::read_to_string(path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&content).ok()?;
    val.as_array().map(|a| a.len()).or_else(|| {
        // If it's an object, count the keys (hooks can be an object map).
        val.as_object().map(|o| o.len())
    })
}

/// Resolve the user's home directory.
fn home_dir() -> PathBuf {
    #[cfg(unix)]
    {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"))
    }
    #[cfg(not(unix))]
    {
        std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_plugin(tmp: &TempDir, name: &str) -> (PathBuf, PathBuf) {
        let plugin_root = tmp.path().join("plugins").join(name);

        // Create manifest
        std::fs::create_dir_all(plugin_root.join(".claude-plugin")).unwrap();
        std::fs::write(
            plugin_root.join(".claude-plugin").join("plugin.json"),
            "{}",
        )
        .unwrap();

        // Create hooks
        let hooks_dir = plugin_root
            .join(".claude")
            .join("tools")
            .join("amplihack")
            .join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        std::fs::write(
            hooks_dir.join("hooks.json"),
            r#"[{"name":"pre-commit"}]"#,
        )
        .unwrap();

        // Create settings
        let settings_path = tmp.path().join("settings.json");
        std::fs::write(
            &settings_path,
            serde_json::json!({"enabledPlugins": [name]}).to_string(),
        )
        .unwrap();

        (plugin_root, settings_path)
    }

    #[test]
    fn verify_fully_installed_plugin() {
        let tmp = TempDir::new().unwrap();
        let (plugin_root, settings_path) = setup_plugin(&tmp, "test-plugin");
        let v = PluginVerifier::with_paths("test-plugin", plugin_root, settings_path);
        let r = v.verify();
        assert!(r.success);
        assert!(r.installed);
        assert!(r.discoverable);
        assert!(r.hooks_loaded);
        assert!(r.issues.is_empty());
    }

    #[test]
    fn verify_missing_manifest() {
        let tmp = TempDir::new().unwrap();
        let plugin_root = tmp.path().join("plugins").join("bad");
        std::fs::create_dir_all(&plugin_root).unwrap();
        // No .claude-plugin/plugin.json
        let settings_path = tmp.path().join("settings.json");
        std::fs::write(&settings_path, r#"{"enabledPlugins":["bad"]}"#).unwrap();

        let v = PluginVerifier::with_paths("bad", plugin_root, settings_path);
        let r = v.verify();
        assert!(!r.success);
        assert!(!r.installed);
    }

    #[test]
    fn verify_not_in_settings() {
        let tmp = TempDir::new().unwrap();
        let (plugin_root, settings_path) = setup_plugin(&tmp, "test-plugin");
        // Overwrite settings to remove plugin.
        std::fs::write(&settings_path, r#"{"enabledPlugins":[]}"#).unwrap();
        let v = PluginVerifier::with_paths("test-plugin", plugin_root, settings_path);
        let r = v.verify();
        assert!(!r.success);
        assert!(!r.discoverable);
    }

    #[test]
    fn verify_empty_hooks() {
        let tmp = TempDir::new().unwrap();
        let (plugin_root, settings_path) = setup_plugin(&tmp, "test-plugin");
        // Overwrite hooks with empty array.
        let hooks_path = plugin_root
            .join(".claude")
            .join("tools")
            .join("amplihack")
            .join("hooks")
            .join("hooks.json");
        std::fs::write(&hooks_path, "[]").unwrap();

        let v = PluginVerifier::with_paths("test-plugin", plugin_root, settings_path);
        let r = v.verify();
        assert!(!r.success);
        assert!(!r.hooks_loaded);
    }

    #[test]
    fn verify_missing_settings_file() {
        let tmp = TempDir::new().unwrap();
        let plugin_root = tmp.path().join("plugins").join("x");
        std::fs::create_dir_all(plugin_root.join(".claude-plugin")).unwrap();
        std::fs::write(
            plugin_root.join(".claude-plugin").join("plugin.json"),
            "{}",
        )
        .unwrap();
        let settings_path = tmp.path().join("nonexistent.json");

        let v = PluginVerifier::with_paths("x", plugin_root, settings_path);
        assert!(!v.check_discoverable());
    }

    #[test]
    fn verification_result_serializable() {
        let r = VerificationResult {
            success: true,
            installed: true,
            discoverable: true,
            hooks_loaded: true,
            issues: vec![],
        };
        let json = serde_json::to_string(&r).unwrap();
        let r2: VerificationResult = serde_json::from_str(&json).unwrap();
        assert_eq!(r, r2);
    }
}
