//! Where the profile root comes from (issue #1592): a non-empty
//! `AMPLIHACK_CLAUDE_PROFILE_ROOT`, then `~/.amplihack/profile-root`, then
//! `~/.amplihack/claude-profiles`. Also reads and writes the saved root files
//! (`profile-root` and `rotate-root`) and checks the `profile root <dir>`
//! argument. See "Profile root" in docs/reference/profile-command.md.
//! Resolution and the saved-file read are `amplihack_utils::rotator_status`'s,
//! which the hooks use too.

use super::ProfileError;
use super::store::{
    ProfileStore, check_root_not_symlink, private_dir_builder, reject_root_inside_source,
};
use amplihack_utils::rotator_status::{RootError, check_root_value, locate_root, read_saved_root};
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

pub use amplihack_utils::rotator_status::{
    PROFILE_ROOT_FILE, RootSource, config_dir, home_from_env,
};

/// The root the most recent `rotate start` used, in `~/.amplihack`.
pub const ROTATE_ROOT_FILE: &str = "rotate-root";

fn saved_err(file: &Path, reason: impl Into<String>) -> ProfileError {
    ProfileError::InvalidSavedRoot {
        path: file.display().to_string(),
        reason: reason.into(),
    }
}

/// Resolves the root. A non-empty variable never opens the saved file; a
/// bad variable or a bad saved file is an error, never a fallback.
pub fn resolve_with(
    env: impl Fn(&str) -> Option<OsString>,
    home: Option<&Path>,
) -> Result<(PathBuf, RootSource), ProfileError> {
    locate_root(env, home).map_err(|e| match e {
        RootError::Env(value) => ProfileError::InvalidRoot(value),
        RootError::NoHome => ProfileError::InvalidRoot("HOME is not set".into()),
        RootError::Saved { file, reason } => saved_err(&file, reason),
    })
}

/// The store for this process's environment, and where its root came from.
pub fn store_from_env() -> Result<(ProfileStore, RootSource), ProfileError> {
    let home = home_from_env();
    let (root, source) = resolve_with(|k| std::env::var_os(k), home.as_deref())?;
    let store = ProfileStore::new(root);
    Ok(match home {
        Some(home) => (store.with_shared_assets(home.join(".claude")), source),
        None => (store, source),
    })
}

/// Reads a saved root file. `Ok(None)` when it does not exist; anything else
/// that is not a valid root is `InvalidSavedRoot`, whose reason never
/// repeats the file's contents.
pub fn read_saved(file: &Path) -> Result<Option<PathBuf>, ProfileError> {
    read_saved_root(file).map_err(|reason| saved_err(file, reason))
}

/// Writes `root` and one newline to `file`, atomically and mode 0600,
/// creating its directory (mode 0700) when missing. A value `read_saved`
/// would refuse is never written.
pub fn save(file: &Path, root: &Path) -> Result<(), ProfileError> {
    let text = root.to_str().ok_or_else(|| saved_err(file, "not UTF-8"))?;
    check_root_value(text).map_err(|r| saved_err(file, r))?;
    // `read_saved` trims, so surrounding whitespace would read back as a
    // different root.
    if text.trim() != text {
        return Err(saved_err(file, "has leading or trailing whitespace"));
    }
    let (Some(dir), Some(name)) = (file.parent(), file.file_name().and_then(|n| n.to_str())) else {
        return Err(saved_err(file, "not a file path"));
    };
    match private_dir_builder().create(dir) {
        Err(e) if e.kind() != io::ErrorKind::AlreadyExists => {
            return Err(saved_err(file, e.to_string()));
        }
        _ => {}
    }
    write_file(dir, name, format!("{text}\n").as_bytes())
        .map_err(|e| saved_err(file, e.to_string()))
}

#[cfg(unix)]
fn write_file(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    super::auto_switch::kit::write_private_atomic(dir, name, bytes)
}

#[cfg(not(unix))]
fn write_file(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    use std::fs;
    use std::io::Write;
    let target = dir.join(name);
    if fs::symlink_metadata(&target).is_ok_and(|m| !m.is_file()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a regular file",
        ));
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let tmp = dir.join(format!(".{name}.tmp.{}.{nanos}", std::process::id()));
    let result = (|| {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, &target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Checks the `profile root <dir>` argument without creating anything:
/// the content rule, one trailing `/` removed, not a symlink if it exists,
/// and not `~/.claude` or inside it.
pub fn validate_arg(raw: &str, home: &Path) -> Result<PathBuf, ProfileError> {
    check_root_value(raw).map_err(|_| ProfileError::InvalidRoot(raw.to_string()))?;
    let trimmed = match raw.strip_suffix('/') {
        Some(rest) if !rest.is_empty() => rest,
        _ => raw,
    };
    let path = PathBuf::from(trimmed);
    check_root_not_symlink(&path)?;
    reject_root_inside_source(&path, &home.join(".claude"))?;
    Ok(path)
}
