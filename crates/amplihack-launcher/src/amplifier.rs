//! Amplifier CLI launcher with auto-install, update, and bundle management.
//!
//! Matches Python `amplihack/launcher/amplifier.py`:
//! - Version detection via `amplifier --version`
//! - Auto-install via `uv tool install`
//! - Auto-update via `amplifier update`
//! - Bundle registration and AGENTS.md sync
//! - Launch with managed environment

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, info, warn};

/// Amplifier binary detection result.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AmplifierInfo {
    pub installed: bool,
    pub version: Option<String>,
    pub path: Option<PathBuf>,
}

/// Check if Amplifier is installed and get version info.
pub fn check_amplifier() -> AmplifierInfo {
    match Command::new("amplifier").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let path = which_binary("amplifier");
            info!(version = %version, "Amplifier detected");
            AmplifierInfo {
                installed: true,
                version: Some(version),
                path,
            }
        }
        _ => {
            debug!("Amplifier not found");
            AmplifierInfo {
                installed: false,
                version: None,
                path: None,
            }
        }
    }
}

/// Install Amplifier via uv tool install.
pub fn install_amplifier() -> Result<()> {
    info!("Installing Amplifier via uv...");
    let status = Command::new("uv")
        .args(["tool", "install", "amplifier-cli"])
        .status()
        .context("failed to run uv tool install")?;
    if !status.success() {
        anyhow::bail!("uv tool install amplifier-cli failed with status {status}");
    }
    info!("Amplifier installed successfully");
    Ok(())
}

/// Update Amplifier to latest version.
pub fn upgrade_amplifier() -> Result<()> {
    info!("Updating Amplifier...");
    let status = Command::new("amplifier")
        .args(["update"])
        .status()
        .context("failed to run amplifier update")?;
    if !status.success() {
        warn!("amplifier update failed, continuing with current version");
    }
    Ok(())
}

/// Ensure the amplifier bundle is registered for the project.
pub fn ensure_bundle_registered(project_path: &Path) -> Result<()> {
    let bundle_dir = project_path.join("amplifier-bundle");
    if !bundle_dir.exists() {
        debug!("No amplifier-bundle directory found, skipping registration");
        return Ok(());
    }
    info!("Registering amplifier bundle...");
    let status = Command::new("amplifier")
        .args(["bundle", "register", "--path"])
        .arg(&bundle_dir)
        .current_dir(project_path)
        .status()
        .context("failed to register bundle")?;
    if !status.success() {
        warn!("Bundle registration failed, continuing anyway");
    }
    Ok(())
}

/// Sync AGENTS.md with CLAUDE.md for amplifier compatibility.
pub fn sync_agents_md(project_path: &Path) -> Result<()> {
    let agents_md = project_path.join("AGENTS.md");
    let claude_md = project_path.join("CLAUDE.md");

    if agents_md.exists() && !claude_md.exists() {
        info!("Syncing AGENTS.md → CLAUDE.md for amplifier compatibility");
        std::fs::copy(&agents_md, &claude_md).context("failed to copy AGENTS.md to CLAUDE.md")?;
    } else if claude_md.exists() && !agents_md.exists() {
        info!("Syncing CLAUDE.md → AGENTS.md");
        std::fs::copy(&claude_md, &agents_md).context("failed to copy CLAUDE.md to AGENTS.md")?;
    }
    Ok(())
}

/// Build the command to launch Amplifier.
pub fn build_amplifier_command(
    prompt: &str,
    project_path: &Path,
    extra_args: &[String],
) -> Command {
    let mut cmd = Command::new("amplifier");
    cmd.current_dir(project_path);
    cmd.arg("run");
    if !prompt.is_empty() {
        cmd.args(["--prompt", prompt]);
    }
    for arg in extra_args {
        cmd.arg(arg);
    }
    cmd.env("AMPLIHACK_AGENT_BINARY", "amplifier");
    cmd
}

/// Full launch: ensure installed, register bundle, sync docs, build command.
pub fn ensure_and_build(
    prompt: &str,
    project_path: &Path,
    extra_args: &[String],
    auto_install: bool,
) -> Result<Command> {
    let info = check_amplifier();
    if !info.installed {
        if auto_install {
            install_amplifier()?;
        } else {
            anyhow::bail!("Amplifier is not installed. Run `uv tool install amplifier-cli`.");
        }
    }
    ensure_bundle_registered(project_path)?;
    sync_agents_md(project_path)?;
    Ok(build_amplifier_command(prompt, project_path, extra_args))
}

fn which_binary(name: &str) -> Option<PathBuf> {
    Command::new("which")
        .arg(name)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amplifier_info_serializes() {
        let info = AmplifierInfo {
            installed: true,
            version: Some("0.5.0".into()),
            path: None,
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["installed"], true);
        assert_eq!(json["version"], "0.5.0");
    }

    #[test]
    fn build_amplifier_command_sets_env() {
        let cmd = build_amplifier_command("test", Path::new("/tmp"), &[]);
        let envs: Vec<_> = cmd.get_envs().collect();
        assert!(envs.iter().any(|(k, v)| *k == "AMPLIHACK_AGENT_BINARY"
            && v == &Some(std::ffi::OsStr::new("amplifier"))));
    }

    #[test]
    fn build_amplifier_command_includes_run_subcommand() {
        let cmd = build_amplifier_command("hello", Path::new("/tmp"), &[]);
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args[0], "run");
    }

    #[test]
    fn sync_agents_md_copies_to_claude() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# Agents\nContent").unwrap();
        sync_agents_md(dir.path()).unwrap();
        let claude = std::fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap();
        assert!(claude.contains("# Agents"));
    }

    #[test]
    fn sync_agents_md_copies_from_claude() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("CLAUDE.md"), "# Claude\nInstructions").unwrap();
        sync_agents_md(dir.path()).unwrap();
        let agents = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
        assert!(agents.contains("# Claude"));
    }

    #[test]
    fn sync_agents_md_noop_when_both_exist() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "agents").unwrap();
        std::fs::write(dir.path().join("CLAUDE.md"), "claude").unwrap();
        sync_agents_md(dir.path()).unwrap();
        // Neither should be overwritten
        assert_eq!(
            std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap(),
            "agents"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap(),
            "claude"
        );
    }
}
