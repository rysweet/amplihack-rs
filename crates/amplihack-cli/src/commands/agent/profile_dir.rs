//! Which config directory a profile means (D12): the directory a resume
//! with a recorded profile uses, and the profile a found config directory
//! belongs to, so that what is recorded, what the pre-check reads and what
//! the pane's launch uses agree.

use super::sessions::root_error;
use super::validate;
use amplihack_utils::rotator_status::locate_root;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

const PROFILE_MISSING: &str = "--profile: the profile's directory must exist under the profile \
                               root and must not be a symlink";

/// [`profile_config_dir_from`] for this process's environment.
pub fn profile_config_dir(home: &Path, profile: Option<&str>) -> Result<PathBuf, String> {
    profile_config_dir_from(home, profile, |k| std::env::var_os(k))
}

/// The config directory a resume with `profile` uses (D12): `~/.claude`
/// without a profile, otherwise `root/<profile>`, which must be a real
/// directory. Errors are whole messages for the `--profile` flag.
pub fn profile_config_dir_from(
    home: &Path,
    profile: Option<&str>,
    env: impl Fn(&str) -> Option<OsString>,
) -> Result<PathBuf, String> {
    let Some(p) = profile else {
        return Ok(home.join(".claude"));
    };
    validate::profile_name(p).map_err(|rule| format!("--profile: {rule}"))?;
    let (root, _) = locate_root(env, Some(home))
        .map_err(|e| format!("cannot list profile directories: {}", root_error(&e)))?;
    let dir = root.join(p);
    match fs::symlink_metadata(&dir) {
        Ok(m) if m.is_dir() => Ok(dir),
        _ => Err(PROFILE_MISSING.to_string()),
    }
}

/// The profile whose directory is `config_dir`, one of
/// [`config_dirs`](super::sessions::config_dirs):
/// `None` for `~/.claude`, otherwise the directory's name when it passes the
/// profile rule. This is the profile a launch actually ran under, however it
/// was chosen (`--profile`, an inherited `AMPLIHACK_CLAUDE_PROFILE` after
/// auto-switch, or the default pointer).
pub fn profile_of_config_dir(home: &Path, config_dir: &Path) -> Option<String> {
    if config_dir == home.join(".claude") {
        return None;
    }
    let name = config_dir.file_name()?.to_str()?;
    validate::profile_name(name).ok()?;
    Some(name.to_string())
}

/// The config directory the pane command's launch will use. The pane passes
/// `--profile` only when one is recorded and removes
/// `AMPLIHACK_CLAUDE_PROFILE`, so with no recorded profile the launch reads
/// the default pointer; this resolves the same way, so the transcript is
/// checked where `claude --resume` will look for it. A recorded profile whose
/// directory is gone is an error, as it is for the launch.
pub(super) fn resume_config_dir(home: &Path, profile: Option<&str>) -> Result<PathBuf, String> {
    if let Some(p) = profile {
        profile_config_dir(home, Some(p))?;
    }
    let dir = crate::commands::profile::resume_profile_dir(profile, Some(home))?;
    Ok(dir.unwrap_or_else(|| home.join(".claude")))
}
