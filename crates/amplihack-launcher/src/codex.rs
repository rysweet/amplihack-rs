//! OpenAI Codex launcher with auto-install and auto-update.
//!
//! Matches Python `amplihack/launcher/codex.py`:
//! - Shared discovery and version-specific native/helper validation
//! - Auto-install via npm
//! - Auto-update to latest version
//! - Preserving TOML configuration
//! - Launch with managed environment

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{info, warn};

use crate::flag_matrix::AgentBinary;
use crate::prompt_delivery::{DeliveredCommand, build_tool_command_from_env};

/// Codex binary detection result.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CodexInfo {
    pub installed: bool,
    pub version: Option<String>,
    pub path: Option<PathBuf>,
}

/// Check if Codex is installed and get version info.
pub fn check_codex() -> CodexInfo {
    let resolution = codex_resolution();
    match resolution.target {
        Some(target) => CodexInfo {
            installed: true,
            version: Some(target.version),
            path: Some(target.path),
        },
        None => {
            warn!("{}", resolution.rejection_report("codex", "@openai/codex"));
            CodexInfo {
                installed: false,
                version: None,
                path: None,
            }
        }
    }
}

fn codex_resolution() -> amplihack_utils::launch_target::Resolution {
    use amplihack_utils::launch_target::{OverrideOrigin, resolve_uncached};
    resolve_uncached("codex", OverrideOrigin::User)
}

/// Install Codex via npm.
pub fn install_codex() -> Result<()> {
    info!("Installing Codex via npm...");
    let status = Command::new("npm")
        .args(["install", "-g", "@openai/codex"])
        .status()
        .context("failed to run npm install")?;
    if !status.success() {
        anyhow::bail!("npm install @openai/codex failed with status {status}");
    }
    info!("Codex installed successfully");
    Ok(())
}

/// Ensure Codex is at the latest version.
pub fn ensure_latest_codex() -> Result<()> {
    info!("Updating Codex to latest version...");
    let status = Command::new("npm")
        .args(["update", "-g", "@openai/codex"])
        .status()
        .context("failed to run npm update")?;
    if !status.success() {
        warn!("npm update codex failed, continuing with current version");
    }
    Ok(())
}

/// Validate project TOML without adding approval overrides.
pub fn configure_codex(project_path: &Path) -> Result<()> {
    crate::codex_config::configure(&project_path.join(".codex"), false)
}

/// Build the command to launch Codex with the given prompt.
pub fn build_codex_command(prompt: &str, project_path: &Path, extra_args: &[String]) -> Command {
    let delivered = build_codex_command_with_prompt_delivery(prompt, project_path, extra_args)
        .expect("invalid Codex invocation; use the fallible delivery-aware builder");
    assert!(
        delivered.stdin_payload.is_none(),
        "Codex exec requires the delivery-aware builder and its complete stdin payload"
    );
    delivered.command
}

pub fn build_codex_command_with_prompt_delivery(
    prompt: &str,
    project_path: &Path,
    extra_args: &[String],
) -> std::io::Result<DeliveredCommand> {
    build_tool_command_from_env(AgentBinary::Codex, project_path, extra_args, prompt)
}

/// Ensure Codex is installed and up-to-date, then return a ready command.
pub fn ensure_and_build(
    prompt: &str,
    project_path: &Path,
    extra_args: &[String],
    auto_install: bool,
) -> Result<Command> {
    use amplihack_utils::launch_target::InstallDecision;
    let mut resolution = codex_resolution();
    if resolution.target.is_none() {
        let original = resolution.rejection_report("codex", "@openai/codex");
        let decision =
            amplihack_utils::launch_target::decide_install("codex", &resolution, None, None);
        if decision == InstallDecision::Abstain {
            anyhow::bail!(
                "{original}\nValidation is inconclusive; automatic repair was not attempted. Select a supported complete installation or address the reported inspection failure."
            );
        }
        if !auto_install || decision == InstallDecision::BrokenOverride {
            anyhow::bail!(
                "{original}\nRepair the installation with npm install -g @openai/codex, or change/unset AMPLIHACK_CODEX_BINARY_PATH and CODEX_BINARY_PATH."
            );
        }
        eprintln!("{original}");
        install_codex()
            .map_err(|error| anyhow::anyhow!("{original}\nCodex repair failed: {error:#}"))?;
        resolution = codex_resolution();
        if resolution.target.is_none() {
            anyhow::bail!(
                "{original}\nCodex remains unusable after npm repair:\n{}",
                resolution.rejection_report("codex", "@openai/codex")
            );
        }
    }
    let target = resolution.target.expect("validated above");
    let delivered = crate::prompt_delivery::build_codex_command_at(
        &target.path,
        project_path,
        extra_args,
        prompt,
    )?;
    anyhow::ensure!(
        delivered.stdin_payload.is_none(),
        "Codex exec requires build_codex_command_with_prompt_delivery and explicit stdin ownership"
    );
    Ok(delivered.command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_info_serializes() {
        let info = CodexInfo {
            installed: true,
            version: Some("1.0.0".into()),
            path: Some(PathBuf::from("/usr/bin/codex")),
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["installed"], true);
        assert_eq!(json["version"], "1.0.0");
    }

    #[test]
    fn build_codex_command_sets_env() {
        let cmd = build_codex_command("test prompt", Path::new("/tmp"), &[]);
        let envs: Vec<_> = cmd.get_envs().collect();
        assert!(
            envs.iter().any(|(k, v)| *k == "AMPLIHACK_AGENT_BINARY"
                && v == &Some(std::ffi::OsStr::new("codex")))
        );
    }

    #[test]
    fn build_codex_command_with_extra_args() {
        let cmd = build_codex_command(
            "hello",
            Path::new("/tmp"),
            &["--search".into(), "--no-alt-screen".into()],
        );
        let args: Vec<_> = cmd.get_args().collect();
        assert!(args.contains(&std::ffi::OsStr::new("--search")));
        assert!(args.contains(&std::ffi::OsStr::new("--no-alt-screen")));
    }

    #[test]
    fn configure_codex_does_not_create_legacy_config() {
        let dir = tempfile::tempdir().unwrap();
        configure_codex(dir.path()).unwrap();
        assert!(!dir.path().join(".codex/config.yaml").exists());
    }

    #[test]
    fn configure_codex_preserves_legacy_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join(".codex");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("config.yaml"), "custom: true\n").unwrap();
        configure_codex(dir.path()).unwrap();
        let content = std::fs::read_to_string(config_dir.join("config.yaml")).unwrap();
        assert_eq!(content, "custom: true\n");
    }
}
