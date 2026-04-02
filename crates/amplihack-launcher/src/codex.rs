//! OpenAI Codex launcher with auto-install and auto-update.
//!
//! Matches Python `amplihack/launcher/codex.py`:
//! - Version detection via `codex --version`
//! - Auto-install via npm
//! - Auto-update to latest version
//! - Configuration (approval_mode: auto)
//! - Launch with managed environment

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Maximum time to wait for a version-check subprocess.
const VERSION_CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// Codex binary detection result.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CodexInfo {
    pub installed: bool,
    pub version: Option<String>,
    pub path: Option<PathBuf>,
}

/// Check if Codex is installed and get version info.
pub fn check_codex() -> CodexInfo {
    let child = Command::new("codex")
        .arg("--version")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    match child.and_then(|c| wait_with_timeout(c, VERSION_CHECK_TIMEOUT)) {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let path = which_binary("codex");
            info!(version = %version, "Codex detected");
            CodexInfo {
                installed: true,
                version: Some(version),
                path,
            }
        }
        _ => {
            debug!("Codex not found or timed out");
            CodexInfo {
                installed: false,
                version: None,
                path: None,
            }
        }
    }
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

/// Configure Codex for autonomous mode (approval_mode: auto).
pub fn configure_codex(project_path: &Path) -> Result<()> {
    let config_dir = project_path.join(".codex");
    std::fs::create_dir_all(&config_dir).context("failed to create .codex directory")?;
    let config_file = config_dir.join("config.yaml");
    if !config_file.exists() {
        std::fs::write(&config_file, "approval_mode: auto\n")
            .context("failed to write codex config")?;
        info!("Created Codex config with approval_mode: auto");
    }
    Ok(())
}

/// Build the command to launch Codex with the given prompt.
pub fn build_codex_command(prompt: &str, project_path: &Path, extra_args: &[String]) -> Command {
    let mut cmd = Command::new("codex");
    cmd.current_dir(project_path);
    if !prompt.is_empty() {
        cmd.args(["--prompt", prompt]);
    }
    for arg in extra_args {
        cmd.arg(arg);
    }
    // Set agent binary env var for nested sessions
    cmd.env("AMPLIHACK_AGENT_BINARY", "codex");
    cmd
}

/// Ensure Codex is installed and up-to-date, then return a ready command.
pub fn ensure_and_build(
    prompt: &str,
    project_path: &Path,
    extra_args: &[String],
    auto_install: bool,
) -> Result<Command> {
    let info = check_codex();
    if !info.installed {
        if auto_install {
            install_codex()?;
        } else {
            anyhow::bail!("Codex is not installed. Run `npm install -g @openai/codex` to install.");
        }
    }
    Ok(build_codex_command(prompt, project_path, extra_args))
}

fn which_binary(name: &str) -> Option<PathBuf> {
    Command::new("which")
        .arg(name)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
}

/// Wait for a child process with a timeout, killing it if it exceeds the limit.
fn wait_with_timeout(
    mut child: std::process::Child,
    timeout: Duration,
) -> std::io::Result<std::process::Output> {
    use std::io;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait()? {
            Some(status) => {
                let stdout = child.stdout.map_or_else(Vec::new, |mut s| {
                    let mut buf = Vec::new();
                    let _ = io::Read::read_to_end(&mut s, &mut buf);
                    buf
                });
                let stderr = child.stderr.map_or_else(Vec::new, |mut s| {
                    let mut buf = Vec::new();
                    let _ = io::Read::read_to_end(&mut s, &mut buf);
                    buf
                });
                return Ok(std::process::Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            None if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(io::ErrorKind::TimedOut, "process timed out"));
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }
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
            &["--verbose".into(), "--dry-run".into()],
        );
        let args: Vec<_> = cmd.get_args().collect();
        assert!(args.contains(&std::ffi::OsStr::new("--verbose")));
        assert!(args.contains(&std::ffi::OsStr::new("--dry-run")));
    }

    #[test]
    fn configure_codex_creates_config() {
        let dir = tempfile::tempdir().unwrap();
        configure_codex(dir.path()).unwrap();
        let config = dir.path().join(".codex/config.yaml");
        assert!(config.exists());
        let content = std::fs::read_to_string(config).unwrap();
        assert!(content.contains("approval_mode: auto"));
    }

    #[test]
    fn configure_codex_does_not_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join(".codex");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("config.yaml"), "custom: true\n").unwrap();
        configure_codex(dir.path()).unwrap();
        let content = std::fs::read_to_string(config_dir.join("config.yaml")).unwrap();
        assert!(content.contains("custom: true"));
    }
}
