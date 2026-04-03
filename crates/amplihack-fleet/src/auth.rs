//! Credential propagation for remote fleet VMs.
//!
//! Matches Python `amplihack/fleet/fleet_auth.py`:
//! - Propagate GitHub, Azure, and Claude credentials to remote VMs
//! - SSH-based secure credential transfer
//! - Credential validation before propagation

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::process::{Command, Stdio};
use tracing::{debug, info, warn};

/// Types of credentials that can be propagated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialType {
    GitHub,
    Azure,
    Claude,
    Anthropic,
    OpenAi,
}

impl CredentialType {
    pub fn env_var_name(&self) -> &'static str {
        match self {
            Self::GitHub => "GITHUB_TOKEN",
            Self::Azure => "AZURE_CREDENTIALS",
            Self::Claude => "CLAUDE_API_KEY",
            Self::Anthropic => "ANTHROPIC_API_KEY",
            Self::OpenAi => "OPENAI_API_KEY",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::GitHub => "GitHub Token",
            Self::Azure => "Azure Credentials",
            Self::Claude => "Claude API Key",
            Self::Anthropic => "Anthropic API Key",
            Self::OpenAi => "OpenAI API Key",
        }
    }
}

/// Result of checking which credentials are available locally.
#[derive(Debug, Clone, Serialize)]
pub struct CredentialInventory {
    pub available: Vec<CredentialType>,
    pub missing: Vec<CredentialType>,
}

/// Check which credentials are available in the local environment.
pub fn check_local_credentials() -> CredentialInventory {
    let all_types = [
        CredentialType::GitHub,
        CredentialType::Azure,
        CredentialType::Claude,
        CredentialType::Anthropic,
        CredentialType::OpenAi,
    ];

    let mut available = Vec::new();
    let mut missing = Vec::new();

    for cred_type in all_types {
        if std::env::var(cred_type.env_var_name()).is_ok() {
            available.push(cred_type);
        } else {
            missing.push(cred_type);
        }
    }

    CredentialInventory { available, missing }
}

/// Validate that an SSH target does not contain shell metacharacters.
fn validate_ssh_target(target: &str) -> Result<()> {
    if target.is_empty() {
        bail!("SSH target must not be empty");
    }
    // Allow user@host, user@host:port, IPv4, IPv6 in brackets, hostnames
    let forbidden = ['\'', '"', '`', '$', '(', ')', ';', '&', '|', '\\', '\n', '\r', ' ', '\t'];
    if target.chars().any(|c| forbidden.contains(&c)) {
        bail!("SSH target contains forbidden characters: {target:?}");
    }
    Ok(())
}

/// Propagate a specific credential to a remote VM via SSH.
///
/// The credential value is passed through stdin to avoid shell injection.
/// The remote script reads the value from stdin and writes it to ~/.bashrc.
pub fn propagate_credential(ssh_target: &str, cred_type: CredentialType) -> Result<bool> {
    validate_ssh_target(ssh_target)?;

    let env_name = cred_type.env_var_name();
    let value = match std::env::var(env_name) {
        Ok(v) if !v.is_empty() => v,
        _ => {
            warn!(
                credential = cred_type.display_name(),
                "Credential not available locally, skipping"
            );
            return Ok(false);
        }
    };

    // Reject credential values containing newlines (would break the stdin protocol).
    if value.contains('\n') || value.contains('\r') {
        bail!(
            "Credential value for {} contains newline characters",
            cred_type.display_name()
        );
    }

    // Pass the credential value via stdin to avoid shell injection.
    // The remote script reads one line from stdin and uses it as the value.
    let remote_cmd = format!(
        "read -r VALUE && \
         grep -q '^export {env_name}=' ~/.bashrc 2>/dev/null && \
         sed -i \"s|^export {env_name}=.*|export {env_name}=$VALUE|\" ~/.bashrc || \
         echo \"export {env_name}=$VALUE\" >> ~/.bashrc"
    );

    let mut child = Command::new("ssh")
        .args([
            "-o",
            "ConnectTimeout=10",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            "BatchMode=yes",
            ssh_target,
            &remote_cmd,
        ])
        .stdin(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "failed to propagate {} to {ssh_target}",
                cred_type.display_name()
            )
        })?;

    // Write credential value to stdin, then close the pipe.
    if let Some(ref mut stdin) = child.stdin {
        writeln!(stdin, "{value}")?;
    }
    drop(child.stdin.take());

    let status = child.wait().with_context(|| {
        format!(
            "failed waiting for credential propagation to {ssh_target}",
        )
    })?;

    if status.success() {
        info!(
            credential = cred_type.display_name(),
            target = ssh_target,
            "Credential propagated"
        );
        Ok(true)
    } else {
        warn!(
            credential = cred_type.display_name(),
            target = ssh_target,
            "Credential propagation failed"
        );
        Ok(false)
    }
}

/// Propagate all available credentials to a remote VM.
pub fn propagate_all(ssh_target: &str) -> Result<PropagationResult> {
    let inventory = check_local_credentials();
    let mut propagated = Vec::new();
    let mut failed = Vec::new();

    for cred_type in &inventory.available {
        match propagate_credential(ssh_target, *cred_type) {
            Ok(true) => propagated.push(*cred_type),
            Ok(false) => failed.push(*cred_type),
            Err(e) => {
                debug!(error = %e, "Propagation error for {}", cred_type.display_name());
                failed.push(*cred_type);
            }
        }
    }

    Ok(PropagationResult {
        target: ssh_target.to_string(),
        propagated,
        failed,
        skipped: inventory.missing,
    })
}

/// Result of credential propagation.
#[derive(Debug, Clone, Serialize)]
pub struct PropagationResult {
    pub target: String,
    pub propagated: Vec<CredentialType>,
    pub failed: Vec<CredentialType>,
    pub skipped: Vec<CredentialType>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_env_var_names() {
        assert_eq!(CredentialType::GitHub.env_var_name(), "GITHUB_TOKEN");
        assert_eq!(CredentialType::Claude.env_var_name(), "CLAUDE_API_KEY");
        assert_eq!(
            CredentialType::Anthropic.env_var_name(),
            "ANTHROPIC_API_KEY"
        );
        assert_eq!(CredentialType::OpenAi.env_var_name(), "OPENAI_API_KEY");
    }

    #[test]
    fn credential_display_names() {
        assert_eq!(CredentialType::GitHub.display_name(), "GitHub Token");
        assert_eq!(CredentialType::Azure.display_name(), "Azure Credentials");
    }

    #[test]
    fn credential_inventory_serializes() {
        let inv = CredentialInventory {
            available: vec![CredentialType::GitHub],
            missing: vec![CredentialType::Claude],
        };
        let json = serde_json::to_value(&inv).unwrap();
        assert!(json["available"].is_array());
        assert!(json["missing"].is_array());
    }

    #[test]
    fn propagation_result_serializes() {
        let result = PropagationResult {
            target: "user@vm-1".into(),
            propagated: vec![CredentialType::GitHub],
            failed: vec![],
            skipped: vec![CredentialType::Azure],
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["target"], "user@vm-1");
    }

    #[test]
    fn check_credentials_runs() {
        // This just verifies the function runs without panicking;
        // actual credential availability depends on the environment
        let inv = check_local_credentials();
        assert_eq!(
            inv.available.len() + inv.missing.len(),
            5,
            "should account for all credential types"
        );
    }

    #[test]
    fn validate_ssh_target_accepts_valid() {
        assert!(validate_ssh_target("user@vm-1.example.com").is_ok());
        assert!(validate_ssh_target("root@10.0.0.1").is_ok());
        assert!(validate_ssh_target("deploy@host:22").is_ok());
    }

    #[test]
    fn validate_ssh_target_rejects_empty() {
        assert!(validate_ssh_target("").is_err());
    }

    #[test]
    fn validate_ssh_target_rejects_shell_metacharacters() {
        assert!(validate_ssh_target("user@host; rm -rf /").is_err());
        assert!(validate_ssh_target("user@host$(whoami)").is_err());
        assert!(validate_ssh_target("user@host`id`").is_err());
        assert!(validate_ssh_target("user@host | cat /etc/passwd").is_err());
        assert!(validate_ssh_target("user@host\nmalicious").is_err());
        assert!(validate_ssh_target("user@host'injection").is_err());
    }
}
