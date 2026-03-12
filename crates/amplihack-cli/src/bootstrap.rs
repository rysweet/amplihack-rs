//! First-run bootstrap for framework assets and host CLIs.

use crate::binary_finder::{BinaryFinder, BinaryInfo};
use crate::commands::install;
use crate::copilot_setup;
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn prepare_launcher(tool: &str) -> Result<()> {
    check_required_tools()?;
    install::ensure_framework_installed()?;

    match tool {
        "copilot" => copilot_setup::ensure_copilot_home_staged()?,
        "codex" => configure_codex()?,
        _ => {}
    }

    Ok(())
}

/// Check that required system tools are available.
/// Prints warnings for missing tools but only fails for critical ones.
fn check_required_tools() -> Result<()> {
    // tmux is required for recipe runner workflow execution
    if which("tmux").is_none() {
        eprintln!("⚠️  tmux is not installed. Recipe workflow execution requires tmux.");
        eprintln!("   Install it:");
        eprintln!("     macOS:  brew install tmux");
        eprintln!("     Ubuntu: sudo apt install tmux");
        eprintln!("     Fedora: sudo dnf install tmux");
    }
    Ok(())
}

fn which(tool: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let full = dir.join(tool);
            if full.is_file() { Some(full) } else { None }
        })
    })
}

pub fn ensure_tool_available(tool: &str) -> Result<BinaryInfo> {
    if let Ok(binary) = BinaryFinder::find(tool) {
        return Ok(binary);
    }

    install_tool(tool)?;
    BinaryFinder::find(tool)
        .with_context(|| format!("failed to locate '{tool}' after installation"))
}

fn install_tool(tool: &str) -> Result<()> {
    match tool {
        "claude" => install_npm_package(tool, "@anthropic-ai/claude-code"),
        "copilot" => install_npm_package(tool, "@github/copilot"),
        "codex" => install_npm_package(tool, "@openai/codex-cli"),
        "amplifier" => install_amplifier(),
        other => bail!("automatic installation is not implemented for '{other}'"),
    }
}

fn install_npm_package(tool: &str, package: &str) -> Result<()> {
    let npm = BinaryFinder::find("npm")
        .context("npm is required to install Node-based host CLIs")?
        .path;

    let prefix = npm_prefix_dir()?;
    let bin_dir = prefix.join("bin");
    fs::create_dir_all(&bin_dir)
        .with_context(|| format!("failed to create {}", bin_dir.display()))?;

    prepend_path(&bin_dir)?;
    println!("📦 Installing {tool} via npm package {package}...");
    let status = Command::new(npm)
        .arg("install")
        .arg("-g")
        .arg("--prefix")
        .arg(&prefix)
        .arg(package)
        .arg("--ignore-scripts")
        .status()
        .context("failed to execute npm install")?;

    if !status.success() {
        bail!("npm install failed for package {package}");
    }

    persist_path_hint(&bin_dir)?;
    Ok(())
}

fn install_amplifier() -> Result<()> {
    let uv = BinaryFinder::find("uv")
        .context("uv is required to install amplifier")?
        .path;
    let bin_dir = uv_bin_dir()?;
    prepend_path(&bin_dir)?;

    println!("📦 Installing amplifier via uv tool...");
    let status = Command::new(uv)
        .arg("tool")
        .arg("install")
        .arg("git+https://github.com/microsoft/amplifier")
        .status()
        .context("failed to execute uv tool install")?;

    if !status.success() {
        bail!("uv tool install failed for amplifier");
    }

    persist_path_hint(&bin_dir)?;
    Ok(())
}

fn configure_codex() -> Result<()> {
    let config_dir = home_dir()?.join(".openai").join("codex");
    fs::create_dir_all(&config_dir)
        .with_context(|| format!("failed to create {}", config_dir.display()))?;
    let config_path = config_dir.join("config.json");

    let mut value = if config_path.exists() {
        fs::read_to_string(&config_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .unwrap_or_else(|| json!({}))
    } else {
        json!({})
    };

    let Some(object) = value.as_object_mut() else {
        value = json!({});
        value
            .as_object_mut()
            .expect("json object just initialized")
            .insert(
                "approval_mode".to_string(),
                Value::String("auto".to_string()),
            );
        fs::write(&config_path, serde_json::to_string_pretty(&value)? + "\n")
            .with_context(|| format!("failed to write {}", config_path.display()))?;
        return Ok(());
    };

    if object.get("approval_mode").and_then(Value::as_str) != Some("auto") {
        object.insert(
            "approval_mode".to_string(),
            Value::String("auto".to_string()),
        );
        fs::write(&config_path, serde_json::to_string_pretty(&value)? + "\n")
            .with_context(|| format!("failed to write {}", config_path.display()))?;
    }

    Ok(())
}

fn prepend_path(dir: &Path) -> Result<()> {
    let current = std::env::var_os("PATH").unwrap_or_default();
    let paths = std::env::split_paths(&current).collect::<Vec<_>>();
    if paths.iter().any(|existing| existing == dir) {
        return Ok(());
    }

    let mut updated = vec![dir.to_path_buf()];
    updated.extend(paths);
    let joined = std::env::join_paths(updated).context("failed to rebuild PATH")?;
    // SAFETY: This CLI is single-process during bootstrap and updates PATH intentionally.
    unsafe {
        std::env::set_var("PATH", joined);
    }
    Ok(())
}

fn persist_path_hint(bin_dir: &Path) -> Result<()> {
    let shell = std::env::var("SHELL").unwrap_or_default();
    let profile = if shell.ends_with("/zsh") || shell.ends_with("/zsh5") {
        home_dir()?.join(".zshrc")
    } else {
        home_dir()?.join(".bashrc")
    };
    let export_line = format!("export PATH=\"{}:$PATH\"", bin_dir.display());

    let existing = fs::read_to_string(&profile).unwrap_or_default();
    if existing.contains(&export_line) {
        return Ok(());
    }

    let mut content = existing;
    if !content.ends_with('\n') && !content.is_empty() {
        content.push('\n');
    }
    content.push_str("# Added by amplihack\n");
    content.push_str(&export_line);
    content.push('\n');

    fs::write(&profile, content).with_context(|| format!("failed to update {}", profile.display()))
}

fn npm_prefix_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(".npm-global"))
}

fn uv_bin_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("UV_TOOL_BIN_DIR") {
        let path = PathBuf::from(dir);
        if !path.as_os_str().is_empty() {
            fs::create_dir_all(&path)
                .with_context(|| format!("failed to create {}", path.display()))?;
            return Ok(path);
        }
    }

    let path = home_dir()?.join(".local").join("bin");
    fs::create_dir_all(&path).with_context(|| format!("failed to create {}", path.display()))?;
    Ok(path)
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| anyhow!("HOME is not set"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configure_codex_sets_auto_mode() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous_home = crate::test_support::set_home(temp.path());
        configure_codex().unwrap();

        let config = fs::read_to_string(temp.path().join(".openai/codex/config.json")).unwrap();
        let value: Value = serde_json::from_str(&config).unwrap();
        assert_eq!(value["approval_mode"], "auto");

        crate::test_support::restore_home(previous_home);
    }

    #[test]
    fn persist_path_hint_is_idempotent() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous_home = crate::test_support::set_home(temp.path());
        // SAFETY: Test-only shell override.
        unsafe {
            std::env::set_var("SHELL", "/bin/bash");
        }

        let bin_dir = temp.path().join(".npm-global/bin");
        fs::create_dir_all(&bin_dir).unwrap();
        persist_path_hint(&bin_dir).unwrap();
        persist_path_hint(&bin_dir).unwrap();

        let profile = fs::read_to_string(temp.path().join(".bashrc")).unwrap();
        assert_eq!(profile.matches("Added by amplihack").count(), 1);

        crate::test_support::restore_home(previous_home);
    }
}
