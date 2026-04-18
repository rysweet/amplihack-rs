//! First-run bootstrap for framework assets and host CLIs.

use crate::binary_finder::{BinaryFinder, BinaryInfo};
use crate::claude_plugin;
use crate::commands::install;
use crate::copilot_setup;
use crate::tool_update_check::{get_installed_version, get_latest_version, sanitize_version};
use crate::util::{is_noninteractive, run_with_timeout};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Timeout for tool installation commands (npm install, uv tool install).
/// These involve network downloads and can be legitimately slow, so we allow
/// 5 minutes before treating them as hung.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(300);

pub fn prepare_launcher(tool: &str) -> Result<()> {
    // SEC-WS2-02: Non-interactive mode (CI, pipes, AMPLIHACK_NONINTERACTIVE=1)
    // skips all interactive setup. The environment is assumed pre-provisioned.
    // This matches Python launcher behavior and prevents hangs in sandboxes.
    if is_noninteractive() {
        tracing::debug!(
            tool,
            "non-interactive mode detected — skipping interactive bootstrap"
        );
        return Ok(());
    }

    check_required_tools()?;
    install::ensure_framework_installed()?;

    match tool {
        "copilot" => copilot_setup::ensure_copilot_home_staged()?,
        "claude" => {
            // Register amplihack as a Claude Code plugin so the agents,
            // skills, and commands staged under ~/.amplihack/.claude/ are
            // discoverable through Claude Code's plugin system. A failure
            // here must not block the launch — hooks are still wired via
            // settings.json even if the plugin registration fails.
            if let Err(err) = claude_plugin::ensure_claude_plugin_installed() {
                tracing::warn!(%err, "failed to register amplihack Claude plugin");
                eprintln!("⚠️  Failed to register amplihack as a Claude Code plugin: {err}");
            }
        }
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
        let _ = maybe_upgrade_tool(tool);
        return BinaryFinder::find(tool)
            .with_context(|| format!("failed to re-locate '{tool}' after upgrade"))
            .or(Ok(binary));
    }

    install_tool(tool)?;
    BinaryFinder::find(tool)
        .with_context(|| format!("failed to locate '{tool}' after installation"))
}

/// Map a tool name to the npm package used for installation and upgrades.
///
/// This is the single source of truth — both `install_tool` and
/// `maybe_upgrade_tool` read through here so they can never disagree on
/// which package backs a given tool.
fn npm_package_for_install(tool: &str) -> Option<&'static str> {
    match tool {
        "claude" => Some("@anthropic-ai/claude-code"),
        "copilot" => Some("@github/copilot"),
        "codex" => Some("@openai/codex"),
        _ => None,
    }
}

fn install_tool(tool: &str) -> Result<()> {
    if let Some(pkg) = npm_package_for_install(tool) {
        return install_npm_package(tool, pkg);
    }
    match tool {
        "amplifier" => install_amplifier(),
        other => bail!("automatic installation is not implemented for '{other}'"),
    }
}

/// If the tool is an npm-backed CLI whose installed version is older than the
/// latest published version, reinstall the package in place. Silent no-op when
/// npm is unavailable, the tool isn't npm-backed, or versions match.
fn maybe_upgrade_tool(tool: &str) -> Result<()> {
    if is_noninteractive() {
        return Ok(());
    }
    let Some(pkg) = npm_package_for_install(tool) else {
        return Ok(());
    };
    let installed = match get_installed_version(pkg) {
        Some(v) => sanitize_version(&v),
        None => return Ok(()),
    };
    let latest = match get_latest_version(pkg) {
        Some(v) => sanitize_version(&v),
        None => return Ok(()),
    };
    if installed.is_empty() || latest.is_empty() || installed == latest {
        return Ok(());
    }

    println!("📦 Upgrading {tool} ({pkg}): {installed} → {latest}");
    if let Err(err) = install_npm_package(tool, pkg) {
        tracing::warn!(%err, tool, pkg, "tool upgrade failed; continuing with existing install");
    }
    Ok(())
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

    // Clean any stale temp dirs npm left behind from a prior failed install
    // (e.g. `@github/.copilot-YYsO5Mpa`). Left in place, these cause
    // `ENOTEMPTY: directory not empty, rename ...` on every subsequent install.
    clean_stale_npm_temp_dirs(&prefix, package);

    match run_npm_install(&npm, &prefix, package) {
        Ok(()) => {}
        Err(err) => {
            // Last-ditch: clean again and retry once. npm's own rename can fail
            // if a concurrent install (or even the first part of this one) raced.
            tracing::warn!(%err, "npm install failed; cleaning stale temp dirs and retrying once");
            clean_stale_npm_temp_dirs(&prefix, package);
            remove_package_install_dir(&prefix, package);
            run_npm_install(&npm, &prefix, package)?;
        }
    }

    persist_path_hint(&bin_dir)?;
    Ok(())
}

fn run_npm_install(npm: &Path, prefix: &Path, package: &str) -> Result<()> {
    let mut npm_cmd = Command::new(npm);
    npm_cmd
        .arg("install")
        .arg("-g")
        .arg("--prefix")
        .arg(prefix)
        .arg(package)
        .arg("--ignore-scripts");
    let status =
        run_with_timeout(npm_cmd, INSTALL_TIMEOUT).context("failed to execute npm install")?;

    if !status.success() {
        bail!("npm install failed for package {package}");
    }
    Ok(())
}

/// Remove stale `.<name>-XXXX` temp dirs that npm leaves behind in the scope
/// directory after a crashed install.
///
/// For a scoped package like `@github/copilot`, npm stages the new copy in
/// `$prefix/lib/node_modules/@github/.copilot-XXXX` and then renames over the
/// final directory. If the rename fails (or npm is killed mid-install), the
/// temp dir is left behind and every subsequent `npm install` trips ENOTEMPTY.
///
/// For an unscoped package `foo`, npm stages it as
/// `$prefix/lib/node_modules/.foo-XXXX`.
fn clean_stale_npm_temp_dirs(prefix: &Path, package: &str) {
    let node_modules = prefix.join("lib").join("node_modules");
    let (scope_dir, dot_prefix) = match split_npm_package(package) {
        Some((scope, name)) => (node_modules.join(format!("@{scope}")), format!(".{name}-")),
        None => (node_modules, format!(".{package}-")),
    };
    let Ok(entries) = fs::read_dir(&scope_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if !name_str.starts_with(&dot_prefix) {
            continue;
        }
        let path = entry.path();
        tracing::warn!(path = %path.display(), "removing stale npm temp dir");
        if let Err(err) = fs::remove_dir_all(&path) {
            tracing::warn!(%err, path = %path.display(), "failed to remove stale npm temp dir");
        } else {
            println!("  🧹 Removed stale npm temp dir: {}", path.display());
        }
    }
}

/// Remove the installed package directory (if present) so `npm install` can
/// recreate it from scratch. Used as a final fallback when the rename path is
/// still wedged.
fn remove_package_install_dir(prefix: &Path, package: &str) {
    let node_modules = prefix.join("lib").join("node_modules");
    let install_dir = match split_npm_package(package) {
        Some((scope, name)) => node_modules.join(format!("@{scope}")).join(name),
        None => node_modules.join(package),
    };
    if install_dir.exists() {
        tracing::warn!(
            path = %install_dir.display(),
            "removing existing package install dir before retry"
        );
        let _ = fs::remove_dir_all(&install_dir);
    }
}

fn split_npm_package(package: &str) -> Option<(&str, &str)> {
    let rest = package.strip_prefix('@')?;
    let (scope, name) = rest.split_once('/')?;
    if scope.is_empty() || name.is_empty() {
        return None;
    }
    Some((scope, name))
}

fn install_amplifier() -> Result<()> {
    let uv = BinaryFinder::find("uv")
        .context("uv is required to install amplifier")?
        .path;
    let bin_dir = uv_bin_dir()?;
    prepend_path(&bin_dir)?;

    println!("📦 Installing amplifier via uv tool...");
    let mut uv_cmd = Command::new(uv);
    uv_cmd
        .arg("tool")
        .arg("install")
        .arg("git+https://github.com/microsoft/amplifier");
    let status =
        run_with_timeout(uv_cmd, INSTALL_TIMEOUT).context("failed to execute uv tool install")?;

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

    // Load existing config, falling back to an empty object for any error
    // (missing file, parse error, or non-object JSON value).
    let mut value = config_path
        .exists()
        .then(|| fs::read_to_string(&config_path).ok())
        .flatten()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));

    let object = value
        .as_object_mut()
        .expect("value is guaranteed an object");
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
