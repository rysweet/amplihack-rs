//! CLI command handlers for plugin management.
//!
//! Ported from `amplihack/plugin_cli/cli_handlers.py` and
//! `amplihack/plugin_cli/parser_setup.py`.
//!
//! Provides thin wrappers around [`PluginManager`] that print user-facing
//! messages and return standard exit codes (0 = success, 1 = failure).

use std::path::PathBuf;

use crate::plugin_manager::PluginManager;
use crate::plugin_verifier::PluginVerifier;

// ---------------------------------------------------------------------------
// Emoji / platform-aware indicators
// ---------------------------------------------------------------------------

/// Success indicator (emoji on Unix, text on Windows).
const SUCCESS: &str = if cfg!(windows) { "[OK]" } else { "✅" };

/// Failure indicator (emoji on Unix, text on Windows).
const FAILURE: &str = if cfg!(windows) { "[ERROR]" } else { "❌" };

// ---------------------------------------------------------------------------
// CLI command handlers
// ---------------------------------------------------------------------------

/// Install a plugin from a git URL or local path.
///
/// - `source`: Git URL (`https://…`, `git@…`) or local directory path.
/// - `force`: If `true`, overwrite an existing installation.
/// - `plugin_root`: Optional override for the plugin install directory.
///
/// Returns `0` on success, `1` on failure.
pub fn plugin_install_command(
    source: &str,
    force: bool,
    plugin_root: Option<PathBuf>,
) -> i32 {
    let manager = PluginManager::new(plugin_root);
    let result = manager.install(source, force);

    if result.success {
        println!("{SUCCESS} Plugin installed: {}", result.plugin_name);
        println!("   Location: {}", result.installed_path.display());
        println!("   {}", result.message);
        0
    } else {
        println!("{FAILURE} Installation failed: {}", result.message);
        1
    }
}

/// Uninstall a plugin by name.
///
/// Returns `0` on success, `1` on failure.
pub fn plugin_uninstall_command(
    plugin_name: &str,
    plugin_root: Option<PathBuf>,
) -> i32 {
    let manager = PluginManager::new(plugin_root);
    let success = manager.uninstall(plugin_name);

    if success {
        println!("{SUCCESS} Plugin removed: {plugin_name}");
        0
    } else {
        println!("{FAILURE} Failed to remove plugin: {plugin_name}");
        println!("   Plugin may not be installed or removal failed");
        1
    }
}

/// Verify a plugin installation and discoverability.
///
/// Returns `0` if fully verified, `1` if any check fails.
pub fn plugin_verify_command(plugin_name: &str) -> i32 {
    let verifier = PluginVerifier::new(plugin_name);
    let result = verifier.verify();

    let status = |ok: bool| if ok { SUCCESS } else { FAILURE };

    println!("Plugin: {plugin_name}");
    println!("  Installed: {}", status(result.installed));
    println!("  Discoverable: {}", status(result.discoverable));
    println!("  Hooks loaded: {}", status(result.hooks_loaded));

    if !result.success {
        println!("\nDiagnostics:");
        for issue in &result.issues {
            println!("  - {issue}");
        }
    }

    if result.success { 0 } else { 1 }
}

/// List all installed plugins and print them.
///
/// Returns `0` always (listing is informational).
pub fn plugin_list_command(plugin_root: Option<PathBuf>) -> i32 {
    let manager = PluginManager::new(plugin_root);
    let plugins = manager.list_installed();

    if plugins.is_empty() {
        println!("No plugins installed.");
    } else {
        println!("Installed plugins:");
        for name in &plugins {
            println!("  - {name}");
        }
    }

    0
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_plugin(root: &std::path::Path, name: &str) -> PathBuf {
        let dir = root.join(name);
        let manifest_dir = dir.join(".claude-plugin");
        std::fs::create_dir_all(&manifest_dir).unwrap();
        std::fs::write(
            manifest_dir.join("plugin.json"),
            r#"{"name":"test-plugin","version":"1.0.0","entry_point":"main.py"}"#,
        )
        .unwrap();
        std::fs::write(dir.join("main.py"), "# entry").unwrap();
        dir
    }

    #[test]
    fn install_command_succeeds_with_valid_source() {
        let tmp = TempDir::new().unwrap();
        let source = create_test_plugin(tmp.path(), "test-plugin");
        let install_dir = tmp.path().join("installed");

        let code = plugin_install_command(
            source.to_str().unwrap(),
            false,
            Some(install_dir.clone()),
        );
        assert_eq!(code, 0);
        assert!(install_dir.join("test-plugin").exists());
    }

    #[test]
    fn install_command_fails_with_bad_source() {
        let code = plugin_install_command("/nonexistent", false, None);
        assert_eq!(code, 1);
    }

    #[test]
    fn uninstall_command_succeeds() {
        let tmp = TempDir::new().unwrap();
        let source = create_test_plugin(tmp.path(), "test-plugin");
        let install_dir = tmp.path().join("installed");

        plugin_install_command(
            source.to_str().unwrap(),
            false,
            Some(install_dir.clone()),
        );

        let code = plugin_uninstall_command("test-plugin", Some(install_dir));
        assert_eq!(code, 0);
    }

    #[test]
    fn uninstall_command_fails_for_missing_plugin() {
        let tmp = TempDir::new().unwrap();
        let code = plugin_uninstall_command(
            "ghost-plugin",
            Some(tmp.path().join("plugins")),
        );
        assert_eq!(code, 1);
    }

    #[test]
    fn list_command_returns_zero() {
        let tmp = TempDir::new().unwrap();
        let code = plugin_list_command(Some(tmp.path().join("plugins")));
        assert_eq!(code, 0);
    }
}
