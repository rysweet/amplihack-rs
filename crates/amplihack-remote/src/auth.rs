//! Azure Service Principal authentication for remote execution.
//!
//! Reads credentials from environment variables (with optional `.env`
//! file fallback) and exposes them as [`AzureCredentials`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::debug;

/// Container for Azure Service Principal credentials.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AzureCredentials {
    pub tenant_id: String,
    pub client_id: String,
    #[serde(skip_serializing)]
    pub client_secret: String,
    pub subscription_id: String,
    pub resource_group: Option<String>,
}

impl AzureCredentials {
    /// Validate that all required fields are non-empty.
    pub fn validate(&self) -> Result<(), String> {
        let mut missing = Vec::new();
        if self.tenant_id.is_empty() {
            missing.push("tenant_id");
        }
        if self.client_id.is_empty() {
            missing.push("client_id");
        }
        if self.client_secret.is_empty() {
            missing.push("client_secret");
        }
        if self.subscription_id.is_empty() {
            missing.push("subscription_id");
        }
        if missing.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "Missing required credentials: {}",
                missing.join(", ")
            ))
        }
    }
}

/// Handles Azure authentication via environment / `.env` files.
pub struct AzureAuthenticator {
    env_file: Option<PathBuf>,
    credentials: Option<AzureCredentials>,
}

impl AzureAuthenticator {
    pub fn new(env_file: Option<PathBuf>) -> Self {
        Self {
            env_file,
            credentials: None,
        }
    }

    /// Load and return credentials, caching after first call.
    pub fn get_credentials(&mut self) -> anyhow::Result<&AzureCredentials> {
        if self.credentials.is_none() {
            self.credentials = Some(self.load_credentials()?);
        }
        Ok(self.credentials.as_ref().unwrap())
    }

    /// Return the subscription id.
    pub fn get_subscription_id(&mut self) -> anyhow::Result<String> {
        Ok(self.get_credentials()?.subscription_id.clone())
    }

    /// Return the optional resource group.
    pub fn get_resource_group(&mut self) -> anyhow::Result<Option<String>> {
        Ok(self.get_credentials()?.resource_group.clone())
    }

    // ------------------------------------------------------------------

    fn load_credentials(&self) -> anyhow::Result<AzureCredentials> {
        // Optionally load .env
        if let Some(ref p) = self.env_file {
            if p.exists() {
                Self::load_env_file(p)?;
            } else {
                anyhow::bail!(".env file not found: {}", p.display());
            }
        } else if let Some(p) = Self::find_env_file() {
            Self::load_env_file(&p)?;
        }

        let creds = AzureCredentials {
            tenant_id: std::env::var("AZURE_TENANT_ID").unwrap_or_default(),
            client_id: std::env::var("AZURE_CLIENT_ID").unwrap_or_default(),
            client_secret: std::env::var("AZURE_CLIENT_SECRET").unwrap_or_default(),
            subscription_id: std::env::var("AZURE_SUBSCRIPTION_ID").unwrap_or_default(),
            resource_group: std::env::var("AZURE_RESOURCE_GROUP").ok(),
        };

        debug!(
            tenant = !creds.tenant_id.is_empty(),
            client = !creds.client_id.is_empty(),
            secret = !creds.client_secret.is_empty(),
            subscription = !creds.subscription_id.is_empty(),
            "loaded azure credentials"
        );

        creds.validate().map_err(|e| anyhow::anyhow!(e))?;

        Ok(creds)
    }

    fn find_env_file() -> Option<PathBuf> {
        let cwd = std::env::current_dir().ok()?;
        let candidate = cwd.join(".env");
        if candidate.exists() {
            return Some(candidate);
        }
        None
    }

    fn load_env_file(path: &Path) -> anyhow::Result<()> {
        debug!(path = %path.display(), "loading .env file");
        let content = std::fs::read_to_string(path)?;
        let vars = parse_env_content(&content);
        for (key, value) in vars {
            if std::env::var(&key).is_err() {
                // SAFETY: This is called during single-threaded
                // initialization before any threads are spawned.
                unsafe { std::env::set_var(&key, &value) };
                debug!(key = %key, "set env var from .env");
            }
        }
        Ok(())
    }
}

/// Convenience function matching the Python `get_azure_auth()`.
pub fn get_azure_auth(env_file: Option<PathBuf>) -> anyhow::Result<AzureCredentials> {
    let mut auth = AzureAuthenticator::new(env_file);
    auth.get_credentials().cloned()
}

/// Parse simple `KEY=VALUE` lines (ignoring comments / blanks).
fn parse_env_content(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = trimmed.split_once('=') {
            let key = key.trim().to_string();
            let value = value
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
            map.insert(key, value);
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_env_ignores_comments_and_blanks() {
        let content = "# comment\n\nFOO=bar\nBAZ=\"qux\"\n";
        let vars = parse_env_content(content);
        assert_eq!(vars.get("FOO").unwrap(), "bar");
        assert_eq!(vars.get("BAZ").unwrap(), "qux");
        assert_eq!(vars.len(), 2);
    }

    #[test]
    fn credentials_validate_missing() {
        let creds = AzureCredentials {
            tenant_id: String::new(),
            client_id: "cid".into(),
            client_secret: "sec".into(),
            subscription_id: "sub".into(),
            resource_group: None,
        };
        let err = creds.validate().unwrap_err();
        assert!(err.contains("tenant_id"));
    }

    #[test]
    fn credentials_validate_ok() {
        let creds = AzureCredentials {
            tenant_id: "t".into(),
            client_id: "c".into(),
            client_secret: "s".into(),
            subscription_id: "sub".into(),
            resource_group: Some("rg".into()),
        };
        assert!(creds.validate().is_ok());
    }

    #[test]
    fn parse_env_strips_quotes() {
        let content = "A='hello'\nB=\"world\"";
        let vars = parse_env_content(content);
        assert_eq!(vars["A"], "hello");
        assert_eq!(vars["B"], "world");
    }
}
