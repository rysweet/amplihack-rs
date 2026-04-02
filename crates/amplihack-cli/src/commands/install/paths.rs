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

pub(super) fn global_claude_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(".claude"))
}

pub(super) fn staging_claude_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(".amplihack").join(".claude"))
}

pub(super) fn global_settings_path() -> Result<PathBuf> {
    Ok(global_claude_dir()?.join("settings.json"))
}

// Retained for symmetry with `xpia_hooks_dir()` and future install-asset
// verification cleanup; current install code only reads the optional XPIA path.
#[allow(dead_code)]
pub(super) fn amplihack_hooks_dir() -> Result<PathBuf> {
    Ok(home_dir()?
        .join(".amplihack")
        .join(".claude")
        .join("tools")
        .join("amplihack")
        .join("hooks"))
}

/// Optional XPIA hook asset directory under the staged install.
///
/// Fresh native installs use unified `amplihack-hooks <subcmd>` entries for the
/// live hook path, but the presence of staged XPIA assets is still used to
/// verify optional installation state and to upgrade legacy `tools/xpia/hooks/*.py`
/// settings entries in place during reinstall.
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

/// Ensure `~/.local/bin` is exported in the user's shell profile.
///
/// If `~/.local/bin` is already mentioned in the rc file (via literal
/// `.local/bin` substring), this is a no-op.  Otherwise it appends an
/// `export PATH` line with a timestamped comment.
pub(crate) fn ensure_local_bin_on_shell_path() -> Result<()> {
    let profile = match shell_profile_path() {
        Some(p) => p,
        None => {
            tracing::debug!("could not detect shell profile; skipping PATH auto-persist");
            return Ok(());
        }
    };

    let existing = std::fs::read_to_string(&profile).unwrap_or_default();
    if existing.contains(".local/bin") {
        return Ok(());
    }

    let line = format!(
        "\n# Added by amplihack install ({})\nexport PATH=\"$HOME/.local/bin:$PATH\"\n",
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
    );

    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&profile)
        .with_context(|| format!("failed to open {} for appending", profile.display()))?;
    file.write_all(line.as_bytes())
        .with_context(|| format!("failed to write PATH export to {}", profile.display()))?;
    println!("  ✅ Added ~/.local/bin to PATH in {}", profile.display());
    Ok(())
}
