//! Where the profile root comes from (issue #1592): a non-empty
//! `AMPLIHACK_CLAUDE_PROFILE_ROOT`, then `~/.amplihack/profile-root`, then
//! `~/.amplihack/claude-profiles`. Also reads and writes the saved root files
//! (`profile-root` and `rotate-root`) and checks the `profile root <dir>`
//! argument. See "Profile root" in docs/reference/profile-command.md.

use super::ProfileError;
use super::store::{
    ProfileStore, ROOT_ENV, check_root_not_symlink, private_dir_builder, reject_root_inside_source,
};
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

/// The saved profile root, in `~/.amplihack`.
pub const PROFILE_ROOT_FILE: &str = "profile-root";
/// The root the most recent `rotate start` used, in `~/.amplihack`.
pub const ROTATE_ROOT_FILE: &str = "rotate-root";
const MAX_SAVED_BYTES: u64 = 4096;

/// Where the root in use came from, as shown in `root: <path> (source: …)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootSource {
    Env,
    Config,
    Default,
}

impl std::fmt::Display for RootSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Env => "env",
            Self::Config => "config",
            Self::Default => "default",
        })
    }
}

pub fn config_dir(home: &Path) -> PathBuf {
    home.join(".amplihack")
}

/// `HOME`, or `USERPROFILE` on Windows; `None` when unset or empty.
pub fn home_from_env() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// The content rule shared by the variable, the argument and the saved
/// files. Returns the reason a value is refused.
fn check_value(raw: &str) -> Result<(), &'static str> {
    if raw.is_empty() {
        Err("empty")
    } else if raw.chars().any(char::is_control) {
        Err("contains control characters")
    } else if !Path::new(raw).is_absolute() {
        Err("not an absolute path")
    } else if Path::new(raw)
        .components()
        .any(|c| c == Component::ParentDir)
    {
        Err("contains a .. component")
    } else {
        Ok(())
    }
}

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
    if let Some(raw) = env(ROOT_ENV).filter(|v| !v.is_empty()) {
        let raw = raw
            .into_string()
            .map_err(|_| ProfileError::InvalidRoot("non-UTF-8 path".into()))?;
        check_value(&raw).map_err(|_| ProfileError::InvalidRoot(raw.clone()))?;
        return Ok((PathBuf::from(raw), RootSource::Env));
    }
    let home = home.ok_or_else(|| ProfileError::InvalidRoot("HOME is not set".into()))?;
    match read_saved(&config_dir(home).join(PROFILE_ROOT_FILE))? {
        Some(root) => Ok((root, RootSource::Config)),
        None => Ok((
            config_dir(home).join("claude-profiles"),
            RootSource::Default,
        )),
    }
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
    let Some(bytes) = read_limited(file).map_err(|e| saved_err(file, e.to_string()))? else {
        return Ok(None);
    };
    if bytes.len() as u64 > MAX_SAVED_BYTES {
        return Err(saved_err(file, "larger than 4096 bytes"));
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| saved_err(file, "not UTF-8"))?;
    let raw = text.trim();
    check_value(raw).map_err(|r| saved_err(file, r))?;
    Ok(Some(PathBuf::from(raw)))
}

/// At most `MAX_SAVED_BYTES + 1` bytes of a regular file that others cannot
/// write, opened without following a final symlink. A refusal is an
/// `io::Error` whose text is the reason.
#[cfg(unix)]
fn read_limited(file: &Path) -> io::Result<Option<Vec<u8>>> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let mut opts = fs::OpenOptions::new();
    opts.read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    let f = match opts.open(file) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
            return Err(io::Error::other("is a symlink"));
        }
        Err(e) => return Err(e),
    };
    let meta = f.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::other("not a regular file"));
    }
    if meta.mode() & 0o022 != 0 {
        return Err(io::Error::other("writable by others"));
    }
    let mut buf = Vec::new();
    f.take(MAX_SAVED_BYTES + 1).read_to_end(&mut buf)?;
    Ok(Some(buf))
}

/// Best-effort portable read: the symlink check and the open are separate
/// steps, and there is no mode check (Windows ACLs are out of scope).
#[cfg(not(unix))]
fn read_limited(file: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(file) {
        Ok(m) if m.file_type().is_symlink() => return Err(io::Error::other("is a symlink")),
        Ok(m) if !m.is_file() => return Err(io::Error::other("not a regular file")),
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    }
    let mut buf = Vec::new();
    fs::File::open(file)?
        .take(MAX_SAVED_BYTES + 1)
        .read_to_end(&mut buf)?;
    Ok(Some(buf))
}

/// Writes `root` and one newline to `file`, atomically and mode 0600,
/// creating its directory (mode 0700) when missing. A value `read_saved`
/// would refuse is never written.
pub fn save(file: &Path, root: &Path) -> Result<(), ProfileError> {
    let text = root.to_str().ok_or_else(|| saved_err(file, "not UTF-8"))?;
    check_value(text).map_err(|r| saved_err(file, r))?;
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
    check_value(raw).map_err(|_| ProfileError::InvalidRoot(raw.to_string()))?;
    let trimmed = match raw.strip_suffix('/') {
        Some(rest) if !rest.is_empty() => rest,
        _ => raw,
    };
    let path = PathBuf::from(trimmed);
    check_root_not_symlink(&path)?;
    reject_root_inside_source(&path, &home.join(".claude"))?;
    Ok(path)
}
