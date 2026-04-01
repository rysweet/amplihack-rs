//! Native plugin management commands.

use crate::command_error::exit_error;
use crate::util::run_with_timeout;
use amplihack_state::AtomicJsonFile;
use anyhow::{Context, Result};
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use tempfile::TempDir;

mod helpers;
mod manager;
mod verifier;

use helpers::{default_plugin_root, home_dir, plugins_json_path};
use manager::PluginManager;
use verifier::PluginVerifier;

/// Timeout for git clone operations.
const GIT_CLONE_TIMEOUT: Duration = Duration::from_secs(120);

const SUCCESS_ICON: &str = "✅";
const FAILURE_ICON: &str = "❌";
const LINK_SUCCESS_ICON: &str = "✓";

#[derive(Debug, Clone, PartialEq, Eq)]
struct InstallResult {
    success: bool,
    plugin_name: String,
    installed_path: PathBuf,
    message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ValidationResult {
    valid: bool,
    errors: Vec<String>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VerificationResult {
    success: bool,
    installed: bool,
    discoverable: bool,
    hooks_loaded: bool,
    issues: Vec<String>,
}

pub fn run_install(source: &str, force: bool) -> Result<()> {
    let manager = PluginManager::new(None)?;
    let result = manager.install(source, force)?;
    if result.success {
        println!("{SUCCESS_ICON} Plugin installed: {}", result.plugin_name);
        println!("   Location: {}", result.installed_path.display());
        println!("   {}", result.message);
        return Ok(());
    }
    println!("{FAILURE_ICON} Installation failed: {}", result.message);
    Err(exit_error(1))
}

pub fn run_uninstall(plugin_name: &str) -> Result<()> {
    let manager = PluginManager::new(None)?;
    if manager.uninstall(plugin_name)? {
        println!("{SUCCESS_ICON} Plugin removed: {plugin_name}");
        return Ok(());
    }
    println!("{FAILURE_ICON} Failed to remove plugin: {plugin_name}");
    println!("   Plugin may not be installed or removal failed");
    Err(exit_error(1))
}

pub fn run_link(plugin_name: &str) -> Result<()> {
    let plugin_root = home_dir()?.join(".amplihack").join("plugins");
    let plugin_path = plugin_root.join(plugin_name);
    if !plugin_path.exists() {
        println!("Error: Plugin not found at {}", plugin_path.display());
        println!("Install the plugin first with: amplihack install");
        return Err(exit_error(1));
    }

    let manager = PluginManager::new(Some(plugin_root))?;
    if manager.register_plugin(plugin_name)? {
        println!("{LINK_SUCCESS_ICON} Plugin linked successfully: {plugin_name}");
        println!("  Settings updated in: ~/.claude/settings.json");
        println!("  Plugin should now appear in /plugin command");
        return Ok(());
    }

    println!("Error: Failed to link plugin: {plugin_name}");
    Err(exit_error(1))
}

pub fn run_verify(plugin_name: &str) -> Result<()> {
    let verifier = PluginVerifier::new(plugin_name)?;
    let result = verifier.verify()?;
    println!("Plugin: {plugin_name}");
    println!(
        "  Installed: {}",
        if result.installed {
            SUCCESS_ICON
        } else {
            FAILURE_ICON
        }
    );
    println!(
        "  Discoverable: {}",
        if result.discoverable {
            SUCCESS_ICON
        } else {
            FAILURE_ICON
        }
    );
    println!(
        "  Hooks loaded: {}",
        if result.hooks_loaded {
            SUCCESS_ICON
        } else {
            FAILURE_ICON
        }
    );

    if !result.success {
        println!();
        println!("Diagnostics:");
        for issue in &result.issues {
            println!("  - {issue}");
        }
        return Err(exit_error(1));
    }

    Ok(())
}

#[cfg(test)]
mod tests;
