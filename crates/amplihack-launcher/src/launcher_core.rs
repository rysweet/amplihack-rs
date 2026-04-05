//! Core launcher orchestrator for Claude CLI.
//!
//! Matches Python `amplihack/launcher/core.py`:
//! - Pre-launch preparation (prerequisites, repo checkout, directory detection)
//! - Claude command building with managed environment
//! - Runtime directory creation and settings patching
//! - Environment detection (non-interactive, sandboxed)

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, info, warn};

/// Launcher configuration, created once at startup.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LauncherConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub append_system_prompt: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkout_repo: Option<String>,
    #[serde(default)]
    pub claude_args: Vec<String>,
    pub verbose: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_directory: Option<PathBuf>,
}

const RUNTIME_DIRS: &[&str] = &["locks", "reflection", "logs", "metrics"];
const PROJECT_DIR_PLACEHOLDER: &str = "$CLAUDE_PROJECT_DIR";

/// Core launcher orchestrator.
pub struct ClaudeLauncher {
    config: LauncherConfig,
    path_cache: HashMap<PathBuf, PathBuf>,
}

impl ClaudeLauncher {
    pub fn new(config: LauncherConfig) -> Self {
        Self {
            config,
            path_cache: HashMap::new(),
        }
    }

    /// Full pre-launch preparation. Returns `true` if launch can proceed.
    pub fn prepare_launch(&mut self) -> Result<bool> {
        if is_noninteractive() && !self.check_prerequisites_noninteractive()? {
            return Ok(false);
        }
        if !self.handle_repo_checkout()? {
            return Ok(false);
        }
        let target_dir = self.find_target_directory();
        if let Some(ref dir) = target_dir {
            self.ensure_runtime_directories(dir)?;
            self.fix_hook_paths_in_settings(dir)?;
        }
        self.config.target_directory = target_dir;
        info!("Pre-launch preparation complete");
        Ok(true)
    }

    /// Build the full Claude CLI command.
    pub fn build_claude_command(&self) -> Result<Command> {
        let cli_path = get_claude_cli_path()?;
        let mut cmd = Command::new(&cli_path);
        if let Some(ref pf) = self.config.append_system_prompt {
            if pf.exists() {
                cmd.args(["--append-system-prompt", &pf.to_string_lossy()]);
            } else {
                warn!(path = %pf.display(), "System prompt file not found");
            }
        }
        if let Some(ref target) = self.config.target_directory
            && !paths_are_same(target, &std::env::current_dir().unwrap_or_default())
        {
            cmd.args(["--add-dir", &target.to_string_lossy()]);
        }
        for arg in &self.config.claude_args {
            cmd.arg(arg);
        }
        cmd.env("AMPLIHACK_MANAGED", "1");
        if self.config.verbose {
            cmd.env("AMPLIHACK_VERBOSE", "1");
        }
        debug!(cli = %cli_path.display(), "Built Claude command");
        Ok(cmd)
    }

    pub fn launch(&mut self) -> Result<i32> {
        if !self.prepare_launch()? {
            return Ok(1);
        }
        let mut cmd = self.build_claude_command()?;
        info!("Launching Claude...");
        let status = cmd.status().context("failed to launch Claude process")?;
        let code = status.code().unwrap_or(1);
        info!(exit_code = code, "Claude process exited");
        Ok(code)
    }

    pub fn launch_interactive(&mut self) -> Result<i32> {
        if !self.prepare_launch()? {
            return Ok(1);
        }
        let mut cmd = self.build_claude_command()?;
        cmd.stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit());
        info!("Launching Claude (interactive)...");
        Ok(cmd
            .status()
            .context("failed to launch")?
            .code()
            .unwrap_or(1))
    }

    pub fn invalidate_path_cache(&mut self) {
        self.path_cache.clear();
    }
    pub fn config(&self) -> &LauncherConfig {
        &self.config
    }

    fn check_prerequisites_noninteractive(&self) -> Result<bool> {
        if get_claude_cli_path().is_err() {
            warn!("Claude CLI not found in non-interactive mode");
            return Ok(false);
        }
        Ok(true)
    }

    fn handle_repo_checkout(&self) -> Result<bool> {
        let Some(ref repo) = self.config.checkout_repo else {
            return Ok(true);
        };
        info!(repo = %repo, "Checking out repository...");
        let status = Command::new("git")
            .args(["clone", "--depth", "1", repo])
            .status()
            .context("failed to run git clone")?;
        if !status.success() {
            warn!(repo = %repo, "Repository checkout failed");
            return Ok(false);
        }
        Ok(true)
    }

    fn find_target_directory(&self) -> Option<PathBuf> {
        if let Some(ref dir) = self.config.target_directory
            && dir.exists()
        {
            return Some(dir.clone());
        }
        let mut current = std::env::current_dir().ok()?;
        loop {
            if current.join(".claude").exists() {
                return Some(current);
            }
            if !current.pop() {
                break;
            }
        }
        // PR #3916 port: fall back to git repo root, then cwd — never empty.
        detect_repo_root().or_else(|| std::env::current_dir().ok())
    }

    fn ensure_runtime_directories(&self, target_dir: &Path) -> Result<()> {
        let base = target_dir.join(".claude").join("runtime");
        for name in RUNTIME_DIRS {
            let path = base.join(name);
            if !path.exists() {
                std::fs::create_dir_all(&path)
                    .with_context(|| format!("create {}", path.display()))?;
                debug!(dir = %path.display(), "Created runtime directory");
            }
        }
        Ok(())
    }

    fn fix_hook_paths_in_settings(&self, target_dir: &Path) -> Result<()> {
        let sp = target_dir.join(".claude").join("settings.json");
        if !sp.exists() {
            return Ok(());
        }
        let content =
            std::fs::read_to_string(&sp).with_context(|| format!("read {}", sp.display()))?;
        if !content.contains(PROJECT_DIR_PLACEHOLDER) {
            return Ok(());
        }
        let updated = content.replace(PROJECT_DIR_PLACEHOLDER, &target_dir.to_string_lossy());
        std::fs::write(&sp, &updated).with_context(|| format!("write {}", sp.display()))?;
        info!("Replaced {} in settings.json", PROJECT_DIR_PLACEHOLDER);
        Ok(())
    }
}

pub fn is_noninteractive() -> bool {
    std::env::var("AMPLIHACK_NONINTERACTIVE").is_ok() || std::env::var("CI").is_ok() || !is_tty()
}

pub fn is_sandboxed() -> bool {
    [
        "CI",
        "GITHUB_ACTIONS",
        "DOCKER_CONTAINER",
        "KUBERNETES_SERVICE_HOST",
    ]
    .iter()
    .any(|v| std::env::var(v).is_ok())
        || std::env::var("HOME").is_err()
        || std::env::var("PATH").is_err()
}

fn get_claude_cli_path() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("CLAUDE_CLI_PATH") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Ok(pb);
        }
    }
    which_binary("claude").ok_or_else(|| anyhow::anyhow!("Claude CLI not found"))
}

fn paths_are_same(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => a == b,
    }
}

/// Detect the repository root via `git rev-parse --show-toplevel`.
///
/// Falls back to the current working directory when git is unavailable or when
/// the working directory is not inside a git repository (PR #3916 parity).
pub fn detect_repo_root() -> Option<PathBuf> {
    match Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
    {
        Ok(output) if output.status.success() => {
            let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if root.is_empty() {
                debug!("git rev-parse --show-toplevel returned empty; falling back to cwd");
                std::env::current_dir().ok()
            } else {
                Some(PathBuf::from(root))
            }
        }
        _ => {
            debug!("git rev-parse --show-toplevel failed; falling back to cwd");
            std::env::current_dir().ok()
        }
    }
}

fn which_binary(name: &str) -> Option<PathBuf> {
    Command::new("which")
        .arg(name)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
}

#[cfg(unix)]
fn is_tty() -> bool {
    unsafe { libc::isatty(0) != 0 }
}
#[cfg(not(unix))]
fn is_tty() -> bool {
    false
}

pub fn launch_claude(args: &[String]) -> Result<i32> {
    ClaudeLauncher::new(LauncherConfig {
        claude_args: args.to_vec(),
        ..Default::default()
    })
    .launch()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let c = LauncherConfig::default();
        assert!(!c.verbose);
        assert!(c.claude_args.is_empty());
    }
    #[test]
    fn config_serializes() {
        let c = LauncherConfig {
            verbose: true,
            claude_args: vec!["--model".into(), "opus".into()],
            ..Default::default()
        };
        let j = serde_json::to_value(&c).unwrap();
        assert_eq!(j["verbose"], true);
        assert_eq!(j["claude_args"][0], "--model");
    }
    #[test]
    fn launcher_stores_config() {
        assert!(
            ClaudeLauncher::new(LauncherConfig {
                verbose: true,
                ..Default::default()
            })
            .config()
            .verbose
        );
    }
    #[test]
    fn is_sandboxed_basic() {
        let in_ci = std::env::var("CI").is_ok();
        assert_eq!(is_sandboxed(), in_ci || std::env::var("HOME").is_err());
    }
    #[test]
    fn paths_same_identical() {
        assert!(paths_are_same(Path::new("/usr"), Path::new("/usr")));
    }
    #[test]
    fn paths_same_different() {
        assert!(!paths_are_same(Path::new("/usr"), Path::new("/var")));
    }
    #[test]
    fn ensure_runtime_dirs() {
        let dir = tempfile::tempdir().unwrap();
        ClaudeLauncher::new(LauncherConfig::default())
            .ensure_runtime_directories(dir.path())
            .unwrap();
        for n in RUNTIME_DIRS {
            assert!(
                dir.path().join(".claude/runtime").join(n).exists(),
                "Missing: {n}"
            );
        }
    }
    #[test]
    fn fix_hook_paths_replaces() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        let sp = dir.path().join(".claude/settings.json");
        std::fs::write(
            &sp,
            r#"{"hooks": {"path": "$CLAUDE_PROJECT_DIR/hooks/pre.sh"}}"#,
        )
        .unwrap();
        ClaudeLauncher::new(LauncherConfig::default())
            .fix_hook_paths_in_settings(dir.path())
            .unwrap();
        let c = std::fs::read_to_string(&sp).unwrap();
        assert!(!c.contains("$CLAUDE_PROJECT_DIR"));
        assert!(c.contains(&dir.path().to_string_lossy().to_string()));
    }
    #[test]
    fn fix_hook_paths_noop() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        let sp = dir.path().join(".claude/settings.json");
        let orig = r#"{"hooks": {"path": "/absolute/hooks/pre.sh"}}"#;
        std::fs::write(&sp, orig).unwrap();
        ClaudeLauncher::new(LauncherConfig::default())
            .fix_hook_paths_in_settings(dir.path())
            .unwrap();
        assert_eq!(std::fs::read_to_string(&sp).unwrap(), orig);
    }
    #[test]
    fn find_target_from_claude_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        let l = ClaudeLauncher::new(LauncherConfig {
            target_directory: Some(dir.path().into()),
            ..Default::default()
        });
        assert_eq!(l.find_target_directory().unwrap(), dir.path());
    }
    #[test]
    fn detect_repo_root_never_empty() {
        // Even outside a git repo, detect_repo_root falls back to cwd.
        let result = detect_repo_root();
        assert!(result.is_some(), "detect_repo_root must never return None");
    }
}
