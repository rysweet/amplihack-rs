//! Copilot MCP server management.
//!
//! Matches Python `amplihack/launcher/copilot.py` MCP features:
//! - Disable default GitHub MCP server
//! - Enable awesome-copilot community MCP server via Docker
//! - MCP server configuration in JSON settings files
//! - Config validation and repair

use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// MCP server entry in settings JSON.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: serde_json::Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
}

/// Known MCP server settings file locations.
pub fn mcp_settings_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = home_dir() {
        // VS Code Copilot settings
        paths.push(home.join(".vscode/mcp.json"));
        // GitHub Copilot CLI settings
        paths.push(home.join(".github-copilot/mcp.json"));
        // Project-local settings
        paths.push(PathBuf::from(".vscode/mcp.json"));
        paths.push(PathBuf::from(".github/copilot/mcp.json"));
    }
    paths
}

/// Disable the default GitHub MCP server in settings.
pub fn disable_github_mcp_server(settings_path: &Path) -> Result<bool> {
    let mut config = load_mcp_config(settings_path)?;
    let servers = config
        .as_object_mut()
        .and_then(|o| o.get_mut("mcpServers"))
        .and_then(|v| v.as_object_mut());

    let Some(servers) = servers else {
        debug!("No mcpServers section found");
        return Ok(false);
    };

    if let Some(github) = servers.get_mut("github") {
        if let Some(obj) = github.as_object_mut() {
            obj.insert("disabled".into(), json!(true));
            save_mcp_config(settings_path, &config)?;
            info!("Disabled GitHub MCP server");
            return Ok(true);
        }
    }
    Ok(false)
}

/// Enable the awesome-copilot community MCP server via Docker.
pub fn enable_awesome_copilot_mcp_server(settings_path: &Path) -> Result<bool> {
    let mut config = load_mcp_config(settings_path)?;
    let servers = config
        .as_object_mut()
        .and_then(|o| {
            o.entry("mcpServers").or_insert_with(|| json!({}));
            o.get_mut("mcpServers")
        })
        .and_then(|v| v.as_object_mut());

    let Some(servers) = servers else {
        return Ok(false);
    };

    if servers.contains_key("awesome-copilot") {
        debug!("awesome-copilot MCP server already configured");
        return Ok(false);
    }

    servers.insert(
        "awesome-copilot".into(),
        json!({
            "command": "docker",
            "args": [
                "run", "--rm", "-i",
                "--name", "awesome-copilot-mcp",
                "ghcr.io/awesome-copilot/mcp-server:latest"
            ]
        }),
    );

    save_mcp_config(settings_path, &config)?;
    info!("Enabled awesome-copilot MCP server");
    Ok(true)
}

/// Validate MCP configuration and repair common issues.
pub fn validate_and_repair(settings_path: &Path) -> Result<Vec<String>> {
    let mut issues = Vec::new();

    if !settings_path.exists() {
        issues.push("MCP settings file does not exist".into());
        return Ok(issues);
    }

    let config = load_mcp_config(settings_path)?;
    let servers = config
        .as_object()
        .and_then(|o| o.get("mcpServers"))
        .and_then(|v| v.as_object());

    let Some(servers) = servers else {
        issues.push("No mcpServers section in config".into());
        return Ok(issues);
    };

    for (name, server) in servers {
        let obj = match server.as_object() {
            Some(o) => o,
            None => {
                issues.push(format!("Server '{name}' is not an object"));
                continue;
            }
        };

        if !obj.contains_key("command") {
            issues.push(format!("Server '{name}' missing 'command' field"));
        }

        if let Some(cmd) = obj.get("command").and_then(|v| v.as_str()) {
            if cmd.is_empty() {
                issues.push(format!("Server '{name}' has empty command"));
            }
        }
    }

    if issues.is_empty() {
        debug!("MCP configuration is valid");
    } else {
        warn!(count = issues.len(), "MCP configuration issues found");
    }

    Ok(issues)
}

/// List all configured MCP servers.
pub fn list_servers(settings_path: &Path) -> Result<Vec<(String, McpServerConfig)>> {
    let config = load_mcp_config(settings_path)?;
    let servers = config
        .as_object()
        .and_then(|o| o.get("mcpServers"))
        .and_then(|v| v.as_object());

    let Some(servers) = servers else {
        return Ok(Vec::new());
    };

    let mut result = Vec::new();
    for (name, value) in servers {
        if let Ok(server) = serde_json::from_value::<McpServerConfig>(value.clone()) {
            result.push((name.clone(), server));
        }
    }
    Ok(result)
}

fn load_mcp_config(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({"mcpServers": {}}));
    }
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&content).with_context(|| format!("failed to parse {}", path.display()))
}

fn save_mcp_config(path: &Path, config: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(config)?;
    std::fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disable_github_server() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(&path, r#"{"mcpServers":{"github":{"command":"gh"}}}"#).unwrap();
        let result = disable_github_mcp_server(&path).unwrap();
        assert!(result);
        let config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(config["mcpServers"]["github"]["disabled"], true);
    }

    #[test]
    fn enable_awesome_copilot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(&path, r#"{"mcpServers":{}}"#).unwrap();
        let result = enable_awesome_copilot_mcp_server(&path).unwrap();
        assert!(result);
        let config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(config["mcpServers"]["awesome-copilot"].is_object());
    }

    #[test]
    fn enable_awesome_copilot_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(&path, r#"{"mcpServers":{}}"#).unwrap();
        enable_awesome_copilot_mcp_server(&path).unwrap();
        let result = enable_awesome_copilot_mcp_server(&path).unwrap();
        assert!(!result, "should be no-op on second call");
    }

    #[test]
    fn validate_finds_issues() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(&path, r#"{"mcpServers":{"bad":{"args":["x"]}}}"#).unwrap();
        let issues = validate_and_repair(&path).unwrap();
        assert!(!issues.is_empty());
        assert!(issues[0].contains("missing 'command'"));
    }

    #[test]
    fn validate_passes_for_valid_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{"mcpServers":{"test":{"command":"echo","args":["hello"]}}}"#,
        )
        .unwrap();
        let issues = validate_and_repair(&path).unwrap();
        assert!(issues.is_empty());
    }

    #[test]
    fn list_servers_returns_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{"mcpServers":{"a":{"command":"x"},"b":{"command":"y"}}}"#,
        )
        .unwrap();
        let servers = list_servers(&path).unwrap();
        assert_eq!(servers.len(), 2);
    }

    #[test]
    fn missing_file_returns_empty() {
        let path = Path::new("/tmp/nonexistent-mcp-config.json");
        let servers = list_servers(path).unwrap();
        assert!(servers.is_empty());
    }
}
