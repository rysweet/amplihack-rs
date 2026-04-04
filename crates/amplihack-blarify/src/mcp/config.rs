//! MCP server configuration.
//!
//! Mirrors the Python `mcp_server/config.py`.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// Database backend type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DbType {
    #[default]
    Neo4j,
    FalkorDb,
}

/// Configuration for the MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Neo4j database URI.
    #[serde(default = "default_neo4j_uri")]
    pub neo4j_uri: String,

    /// Neo4j username.
    #[serde(default = "default_neo4j_username")]
    pub neo4j_username: String,

    /// Neo4j password.
    #[serde(default = "default_neo4j_password")]
    pub neo4j_password: String,

    /// Repository path (used as repo_id).
    pub root_path: String,

    /// Entity identifier.
    #[serde(default = "default_entity_id")]
    pub entity_id: String,

    /// Database backend type.
    #[serde(default)]
    pub db_type: DbType,

    /// FalkorDB host (optional, for FalkorDB backend).
    #[serde(default)]
    pub falkor_host: Option<String>,

    /// FalkorDB port (optional, for FalkorDB backend).
    #[serde(default)]
    pub falkor_port: Option<u16>,
}

fn default_neo4j_uri() -> String {
    "bolt://localhost:7687".into()
}
fn default_neo4j_username() -> String {
    "neo4j".into()
}
fn default_neo4j_password() -> String {
    "password".into()
}
fn default_entity_id() -> String {
    "default".into()
}

impl McpServerConfig {
    /// Validate the Neo4j URI format.
    pub fn validate_neo4j_uri(uri: &str) -> Result<()> {
        const VALID_PREFIXES: &[&str] = &["bolt://", "neo4j://", "neo4j+s://", "neo4j+ssc://"];
        if !VALID_PREFIXES.iter().any(|p| uri.starts_with(p)) {
            bail!("Invalid Neo4j URI format: {uri}");
        }
        Ok(())
    }

    /// Validate configuration based on the selected database type.
    pub fn validate_for_db_type(&self) -> Result<()> {
        match self.db_type {
            DbType::FalkorDb => {
                if self.falkor_host.is_none() || self.falkor_port.is_none() {
                    bail!("FalkorDB requires falkor_host and falkor_port to be set");
                }
            }
            DbType::Neo4j => {
                Self::validate_neo4j_uri(&self.neo4j_uri)?;
                if self.neo4j_username.is_empty() || self.neo4j_password.is_empty() {
                    bail!("Neo4j requires username and password");
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_neo4j_uris() {
        assert!(McpServerConfig::validate_neo4j_uri("bolt://localhost:7687").is_ok());
        assert!(McpServerConfig::validate_neo4j_uri("neo4j://host:7687").is_ok());
        assert!(McpServerConfig::validate_neo4j_uri("neo4j+s://host:7687").is_ok());
    }

    #[test]
    fn invalid_neo4j_uri() {
        assert!(McpServerConfig::validate_neo4j_uri("http://localhost:7687").is_err());
        assert!(McpServerConfig::validate_neo4j_uri("invalid").is_err());
    }

    #[test]
    fn config_roundtrip() {
        let config = McpServerConfig {
            neo4j_uri: "bolt://localhost:7687".into(),
            neo4j_username: "neo4j".into(),
            neo4j_password: "pass".into(),
            root_path: "/repo".into(),
            entity_id: "default".into(),
            db_type: DbType::Neo4j,
            falkor_host: None,
            falkor_port: None,
        };
        let json = serde_json::to_string(&config).unwrap();
        let deser: McpServerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.root_path, "/repo");
    }

    #[test]
    fn validate_neo4j_config() {
        let config = McpServerConfig {
            neo4j_uri: "bolt://localhost:7687".into(),
            neo4j_username: "neo4j".into(),
            neo4j_password: "pass".into(),
            root_path: "/repo".into(),
            entity_id: "default".into(),
            db_type: DbType::Neo4j,
            falkor_host: None,
            falkor_port: None,
        };
        assert!(config.validate_for_db_type().is_ok());
    }

    #[test]
    fn validate_falkordb_requires_host_port() {
        let config = McpServerConfig {
            neo4j_uri: "bolt://localhost:7687".into(),
            neo4j_username: "neo4j".into(),
            neo4j_password: "pass".into(),
            root_path: "/repo".into(),
            entity_id: "default".into(),
            db_type: DbType::FalkorDb,
            falkor_host: None,
            falkor_port: None,
        };
        assert!(config.validate_for_db_type().is_err());
    }
}
