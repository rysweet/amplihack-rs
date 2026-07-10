//! Common path helpers and binary lookup utilities.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn find_binary(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() && is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// Returns `true` if the path has at least one executable bit set.
/// On non-Unix platforms every file is considered executable.
pub(super) fn is_executable(path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

pub(super) fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .context("HOME is not set")
}

pub(super) fn preferred_user_bin_dir() -> Result<PathBuf> {
    Ok(crate::path_conflicts::preferred_user_bin(&home_dir()?))
}

pub(super) fn global_claude_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(".claude"))
}

pub(super) fn staging_claude_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(".amplihack").join(".claude"))
}

/// Staged location for the `amplifier-bundle/` tree.
///
/// The dev-orchestrator skill, recipe runner, and parse-decomposition tooling
/// all expect `AMPLIHACK_HOME/amplifier-bundle/` to exist (where
/// `AMPLIHACK_HOME` defaults to `~/.amplihack`). The bundle ships the
/// `smart-orchestrator`, `default-workflow`, and `investigation-workflow`
/// recipes; without them, the dev-orchestrator's "REQUIRED" execution path is
/// unreachable on a fresh install.
pub(super) fn staging_amplifier_bundle_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(".amplihack").join("amplifier-bundle"))
}

pub(super) fn global_settings_path() -> Result<PathBuf> {
    Ok(global_claude_dir()?.join("settings.json"))
}

/// Optional XPIA hook asset directory under the staged install.
///
/// Fresh native installs use unified `amplihack-hooks <subcmd>` entries for the
/// live hook path, but the presence of staged XPIA assets is still used to
/// verify optional installation state and to upgrade legacy hook settings
/// entries in place during reinstall.
pub(super) fn xpia_hooks_dir() -> Result<PathBuf> {
    Ok(home_dir()?
        .join(".amplihack")
        .join(".claude")
        .join("tools")
        .join("xpia")
        .join("hooks"))
}

pub(super) fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// Determine the shell rc file where PATH exports should be appended.
///
/// Checks `$SHELL` for the login shell and returns the corresponding profile
/// file (e.g. `.bashrc`, `.zshrc`).  Returns `None` when the shell cannot be
/// detected or is unsupported.
pub(crate) fn shell_profile_path() -> Option<PathBuf> {
    let home = home_dir().ok()?;
    let shell = std::env::var("SHELL").ok()?;
    let name = Path::new(&shell).file_name()?.to_str()?;
    let rc = match name {
        "bash" => ".bashrc",
        "zsh" => ".zshrc",
        "fish" => ".config/fish/config.fish",
        "ksh" => ".kshrc",
        _ => return None,
    };
    Some(home.join(rc))
}

const PATH_BLOCK_START: &str = "# >>> amplihack managed PATH >>>";
const PATH_BLOCK_END: &str = "# <<< amplihack managed PATH <<<";

/// Ensure `~/.local/bin` is prepended in the user's shell profile.
///
/// A later PATH mention is not sufficient: stale Python/uvx wrappers earlier
/// on PATH can still win. The managed block is idempotent and always prepends
/// `$HOME/.local/bin` for future shells.
pub(crate) fn ensure_local_bin_on_shell_path() -> Result<()> {
    let profile = match shell_profile_path() {
        Some(p) => p,
        None => {
            tracing::debug!("could not detect shell profile; skipping PATH auto-persist");
            return Ok(());
        }
    };

    let existing = std::fs::read_to_string(&profile).unwrap_or_default();
    let without_old_block = remove_managed_path_block(&existing);
    let next_content = format!("{}{}", without_old_block.trim_end(), managed_path_block());
    if existing == next_content {
        return Ok(());
    }

    atomic_write(&profile, next_content.as_bytes())?;
    println!(
        "  ✅ Ensured ~/.local/bin is prepended to PATH in {}",
        profile.display()
    );
    Ok(())
}

fn managed_path_block() -> String {
    format!(
        "\n{}\n# Added by amplihack install\nexport PATH=\"$HOME/.local/bin:$PATH\"\n{}\n",
        PATH_BLOCK_START, PATH_BLOCK_END
    )
}

fn remove_managed_path_block(input: &str) -> String {
    let Some(start) = input.find(PATH_BLOCK_START) else {
        return input.to_string();
    };
    let Some(end_relative) = input[start..].find(PATH_BLOCK_END) else {
        return input.to_string();
    };
    let end = start + end_relative + PATH_BLOCK_END.len();
    let mut output = String::with_capacity(input.len());
    output.push_str(input[..start].trim_end());
    output.push('\n');
    output.push_str(input[end..].trim_start_matches(['\r', '\n']));
    output
}

fn atomic_write(path: &Path, body: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut tmp_name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    std::fs::write(&tmp, body).with_context(|| format!("failed to write {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("failed to rename {} to {}", tmp.display(), path.display()))?;
    Ok(())
}
