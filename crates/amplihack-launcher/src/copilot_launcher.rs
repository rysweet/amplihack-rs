//! Copilot CLI integration — install, update, and launch.
//!
//! Matches Python `amplihack/launcher/copilot.py`:
//! - Copilot CLI install/update via npm
//! - Plugin registration with config.json
//! - Config validation and repair
//! - Launch orchestration

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, info, warn};

/// Required fields for installed_plugins entries.
const REQUIRED_PLUGIN_DEFAULTS: &[(&str, &str)] = &[
    ("marketplace", "local"),
    ("version", "0.0.0"),
    ("cache_path", ""),
    ("source", "unknown"),
    ("installed_at", "1970-01-01T00:00:00+00:00"),
];

/// Plugin entry for Copilot config.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginEntry {
    pub name: String,
    #[serde(default = "default_marketplace")]
    pub marketplace: String,
    #[serde(default)]
    pub version: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub cache_path: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub installed_at: String,
}

fn default_marketplace() -> String {
    "local".into()
}
fn default_true() -> bool {
    true
}

/// Check if Copilot CLI is installed and responsive.
pub fn check_copilot() -> bool {
    Command::new("copilot")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Install Copilot CLI via npm.
pub fn install_copilot() -> Result<bool> {
    let npm_prefix = copilot_npm_prefix();
    info!("Installing Copilot CLI via npm...");
    let status = Command::new("npm")
        .args([
            "install",
            "-g",
            "--prefix",
            &npm_prefix.to_string_lossy(),
            "@github/copilot",
        ])
        .status()
        .context("failed to run npm install")?;
    Ok(status.success())
}

/// Get the current installed Copilot version.
pub fn get_current_copilot_version() -> Option<String> {
    let output = Command::new("copilot").arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    text.split_whitespace()
        .find(|s| s.contains('.'))
        .map(|s| s.trim_end_matches(" version").to_string())
}

/// Detect how Copilot was installed ("npm" or "uvx").
pub fn detect_install_method() -> String {
    if let Ok(output) = Command::new("uvx").args(["list"]).output()
        && String::from_utf8_lossy(&output.stdout).contains("copilot")
    {
        return "uvx".into();
    }
    "npm".into()
}

/// Execute Copilot update via the detected install method.
pub fn execute_update(install_method: &str) -> Result<bool> {
    info!(method = install_method, "Updating Copilot CLI...");
    let status = match install_method {
        "uvx" => Command::new("uvx")
            .args(["upgrade", "@github/copilot"])
            .status()
            .context("failed to run uvx upgrade")?,
        _ => {
            let prefix = copilot_npm_prefix();
            Command::new("npm")
                .args([
                    "update",
                    "-g",
                    "--prefix",
                    &prefix.to_string_lossy(),
                    "@github/copilot",
                ])
                .status()
                .context("failed to run npm update")?
        }
    };
    Ok(status.success())
}

/// Pre-launch update gate. Respects `AMPLIHACK_SKIP_UPDATE=1`.
pub fn ensure_latest_copilot() -> Result<bool> {
    if std::env::var("AMPLIHACK_SKIP_UPDATE")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        debug!("Skipping update check (AMPLIHACK_SKIP_UPDATE=1)");
        return Ok(true);
    }
    let method = detect_install_method();
    match execute_update(&method) {
        Ok(true) => {
            info!("Copilot CLI updated successfully");
            Ok(true)
        }
        Ok(false) => {
            warn!("Copilot CLI update failed, continuing");
            Ok(true)
        }
        Err(e) => {
            warn!(err = %e, "Update check failed, continuing");
            Ok(true)
        }
    }
}

/// Register the amplihack plugin in Copilot's `config.json`.
pub fn register_copilot_plugin(source_commands: &Path, copilot_home: &Path) -> Result<bool> {
    let config_path = copilot_home.join("config.json");
    let mut config: Value = if config_path.exists() {
        let content =
            std::fs::read_to_string(&config_path).context("failed to read config.json")?;
        serde_json::from_str(&content).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };

    let plugins = config
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("config.json is not an object"))?
        .entry("installed_plugins")
        .or_insert_with(|| json!([]));
    let plugins = plugins
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("installed_plugins is not an array"))?;

    if plugins
        .iter()
        .any(|p| p.get("name").and_then(|n| n.as_str()) == Some("amplihack"))
    {
        debug!("Plugin already registered");
        return Ok(false);
    }

    let cache_path = copilot_home
        .join("installed-plugins")
        .join("amplihack@local")
        .join("commands");
    if source_commands.exists() {
        crate::copilot_staging::stage_md_files(source_commands, &cache_path, false)?;
    }

    plugins.push(json!({
        "name": "amplihack",
        "marketplace": "local",
        "version": "0.0.0",
        "enabled": true,
        "cache_path": cache_path.to_string_lossy(),
        "source": "amplihack-launcher",
        "installed_at": crate::copilot_staging::now_iso(),
    }));

    save_json(&config_path, &config)?;
    info!("Registered amplihack plugin");
    Ok(true)
}

/// Validate and repair plugin entries in config.json.
pub fn validate_and_repair_copilot_config(copilot_home: &Path) -> Result<bool> {
    let config_path = copilot_home.join("config.json");
    if !config_path.exists() {
        return Ok(false);
    }

    let content = std::fs::read_to_string(&config_path)?;
    let mut config: Value = serde_json::from_str(&content)?;
    let mut dirty = false;

    if let Some(plugins) = config
        .get_mut("installed_plugins")
        .and_then(|v| v.as_array_mut())
    {
        for plugin in plugins.iter_mut() {
            if let Some(obj) = plugin.as_object_mut() {
                for &(field, default) in REQUIRED_PLUGIN_DEFAULTS {
                    if !obj.contains_key(field) {
                        obj.insert(field.into(), json!(default));
                        dirty = true;
                    }
                }
                if !obj.contains_key("enabled") {
                    obj.insert("enabled".into(), json!(true));
                    dirty = true;
                }
            }
        }
    }

    if dirty {
        save_json(&config_path, &config)?;
        info!("Repaired config.json plugin entries");
    }
    Ok(dirty)
}

/// Resolve the Copilot home directory.
pub fn copilot_home() -> PathBuf {
    if let Ok(home) = std::env::var("COPILOT_HOME") {
        return PathBuf::from(home);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    PathBuf::from(home).join(".copilot")
}

fn copilot_npm_prefix() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    PathBuf::from(home).join(".npm-global")
}

pub(crate) fn save_json(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(value)?;
    std::fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_copilot_returns_bool() {
        let _ = check_copilot();
    }
    #[test]
    fn detect_method_valid() {
        let m = detect_install_method();
        assert!(m == "npm" || m == "uvx");
    }
    #[test]
    fn copilot_home_default() {
        assert!(
            copilot_home().to_string_lossy().contains("copilot")
                || std::env::var("COPILOT_HOME").is_ok()
        );
    }
    #[test]
    fn register_plugin_creates() {
        let home = tempfile::tempdir().unwrap();
        let cmds = tempfile::tempdir().unwrap();
        std::fs::write(cmds.path().join("dev.md"), "# dev").unwrap();
        assert!(register_copilot_plugin(cmds.path(), home.path()).unwrap());
        let c: Value = serde_json::from_str(
            &std::fs::read_to_string(home.path().join("config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(c["installed_plugins"].as_array().unwrap().len(), 1);
        assert_eq!(c["installed_plugins"][0]["name"], "amplihack");
    }
    #[test]
    fn register_plugin_idempotent() {
        let home = tempfile::tempdir().unwrap();
        let cmds = tempfile::tempdir().unwrap();
        register_copilot_plugin(cmds.path(), home.path()).unwrap();
        assert!(!register_copilot_plugin(cmds.path(), home.path()).unwrap());
    }
    #[test]
    fn validate_repair_backfills() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("config.json"),
            r#"{"installed_plugins":[{"name":"test","enabled":true}]}"#,
        )
        .unwrap();
        assert!(validate_and_repair_copilot_config(home.path()).unwrap());
        let c: Value = serde_json::from_str(
            &std::fs::read_to_string(home.path().join("config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(c["installed_plugins"][0]["marketplace"], "local");
    }
    #[test]
    fn validate_repair_noop() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("config.json"),
            r#"{"installed_plugins":[{"name":"t","marketplace":"local","version":"1","enabled":true,"cache_path":"","source":"t","installed_at":"2024-01-01T00:00:00+00:00"}]}"#).unwrap();
        assert!(!validate_and_repair_copilot_config(home.path()).unwrap());
    }
    #[test]
    fn plugin_entry_serializes() {
        let e = PluginEntry {
            name: "test".into(),
            marketplace: "local".into(),
            version: "1.0".into(),
            enabled: true,
            cache_path: "/p".into(),
            source: "m".into(),
            installed_at: "2024-01-01T00:00:00+00:00".into(),
        };
        let j = serde_json::to_value(&e).unwrap();
        assert_eq!(j["name"], "test");
        assert_eq!(j["enabled"], true);
    }
}
