//! Where the profile root comes from (issue #1592): a non-empty
//! `AMPLIHACK_CLAUDE_PROFILE_ROOT`, then `~/.amplihack/profile-root`, then
//! `~/.amplihack/claude-profiles`. A bad variable or a bad saved file is an
//! error, never a fallback to the next source. The CLI's `root_config`
//! calls [`locate_root`] and [`read_saved_root`]; the hooks use
//! [`resolve_root`], which turns every error into `None`.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

/// The variable that names the profile root.
pub const ROOT_ENV: &str = "AMPLIHACK_CLAUDE_PROFILE_ROOT";
/// The saved profile root, in `~/.amplihack`.
pub const PROFILE_ROOT_FILE: &str = "profile-root";
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

/// Why the root could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootError {
    /// The variable is not UTF-8 (`non-UTF-8 path`) or fails the content
    /// rule (the value itself).
    Env(String),
    /// Neither the variable nor a home directory is set.
    NoHome,
    /// The saved file is unsafe or invalid; `reason` never quotes it.
    Saved { file: PathBuf, reason: String },
}

/// `~/.amplihack`.
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

/// The root for this process's environment, or `None` when it cannot be
/// resolved safely.
pub fn resolve_root() -> Option<PathBuf> {
    resolve_root_from(|k| std::env::var_os(k), home_from_env().as_deref())
}

/// [`resolve_root`] with the environment and home directory passed in.
pub fn resolve_root_from(
    env: impl Fn(&str) -> Option<OsString>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    locate_root(env, home).ok().map(|(root, _)| root)
}

/// Resolves the root and where it came from. A non-empty variable never
/// opens the saved file.
pub fn locate_root(
    env: impl Fn(&str) -> Option<OsString>,
    home: Option<&Path>,
) -> Result<(PathBuf, RootSource), RootError> {
    if let Some(raw) = env(ROOT_ENV).filter(|v| !v.is_empty()) {
        let raw = raw
            .into_string()
            .map_err(|_| RootError::Env("non-UTF-8 path".into()))?;
        check_root_value(&raw).map_err(|_| RootError::Env(raw.clone()))?;
        return Ok((PathBuf::from(raw), RootSource::Env));
    }
    let dir = config_dir(home.ok_or(RootError::NoHome)?);
    let file = dir.join(PROFILE_ROOT_FILE);
    match read_saved_root(&file) {
        Ok(Some(root)) => Ok((root, RootSource::Config)),
        Ok(None) => Ok((dir.join("claude-profiles"), RootSource::Default)),
        Err(reason) => Err(RootError::Saved { file, reason }),
    }
}

/// The content rule for the variable, the `profile root` argument and the
/// saved files: non-empty, no control characters, absolute, no `..`
/// component. Returns the reason a value is refused.
pub fn check_root_value(raw: &str) -> Result<(), &'static str> {
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

/// Reads a saved root file (`profile-root` or `rotate-root`). `Ok(None)`
/// when it does not exist; otherwise the trimmed root, or the reason it is
/// refused, which never repeats the file's contents.
pub fn read_saved_root(file: &Path) -> Result<Option<PathBuf>, String> {
    let Some(bytes) = read_limited(file).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    if bytes.len() as u64 > MAX_SAVED_BYTES {
        return Err("larger than 4096 bytes".into());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| "not UTF-8".to_string())?;
    let raw = text.trim();
    check_root_value(raw)?;
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
