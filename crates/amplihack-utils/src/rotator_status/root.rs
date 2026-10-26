//! The profile root, resolved like the CLI's `root_config::resolve_with`: a
//! non-empty `AMPLIHACK_CLAUDE_PROFILE_ROOT`, then `~/.amplihack/profile-root`,
//! then `~/.amplihack/claude-profiles`. A bad variable or a bad saved file is
//! `None` (the hook stays off), never a fallback to the next source.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

/// The variable that names the profile root.
pub const ROOT_ENV: &str = "AMPLIHACK_CLAUDE_PROFILE_ROOT";
const POINTER_FILE: &str = "profile-root";
const MAX_POINTER_BYTES: u64 = 4096;

/// The root for this process's environment, or `None` when it cannot be
/// resolved safely.
pub fn resolve_root() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|h| !h.is_empty())
        .map(PathBuf::from);
    resolve_root_from(|k| std::env::var_os(k), home.as_deref())
}

/// [`resolve_root`] with the environment and home directory passed in.
pub fn resolve_root_from(
    env: impl Fn(&str) -> Option<OsString>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(raw) = env(ROOT_ENV).filter(|v| !v.is_empty()) {
        let raw = raw.into_string().ok()?;
        return valid_root(&raw).then(|| PathBuf::from(raw));
    }
    let dir = home?.join(".amplihack");
    match read_pointer(&dir.join(POINTER_FILE)) {
        Ok(None) => Some(dir.join("claude-profiles")),
        Ok(Some(bytes)) => {
            let raw = std::str::from_utf8(&bytes).ok()?.trim();
            valid_root(raw).then(|| PathBuf::from(raw))
        }
        Err(()) => None,
    }
}

/// Non-empty, no control characters, absolute, no `..` component.
fn valid_root(raw: &str) -> bool {
    !raw.is_empty()
        && !raw.chars().any(char::is_control)
        && Path::new(raw).is_absolute()
        && !Path::new(raw)
            .components()
            .any(|c| c == Component::ParentDir)
}

/// At most `MAX_POINTER_BYTES` bytes of a regular file that group and
/// others cannot write, opened without following a final symlink.
/// `Ok(None)` when missing; `Err` for anything unsafe or unreadable.
#[cfg(unix)]
fn read_pointer(file: &Path) -> Result<Option<Vec<u8>>, ()> {
    use crate::secure_files::read_bounded_nofollow;
    use std::os::unix::fs::MetadataExt;
    match read_bounded_nofollow(file, MAX_POINTER_BYTES) {
        Ok(None) => Ok(None),
        Ok(Some((bytes, meta))) if meta.mode() & 0o022 == 0 => Ok(Some(bytes)),
        // Writable by others, a symlink, not a regular file, too large or
        // unreadable.
        _ => Err(()),
    }
}

/// Off Unix the hook never runs; refuse rather than read without checks.
#[cfg(not(unix))]
fn read_pointer(_file: &Path) -> Result<Option<Vec<u8>>, ()> {
    Err(())
}
