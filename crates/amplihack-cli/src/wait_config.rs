//! Settings for the usage-limit wait (issue #1618).
//!
//! Each field is resolved separately: the environment variable when it is
//! set and valid, else the `usage_limit_wait` key of
//! `$HOME/.amplihack/config`, else the default (enabled, no maximum). See
//! docs/reference/profile-command.md#usage-limit-wait-settings.
//!
//! The file is read without following symlinks, at most 64 KiB, and only if
//! it is a regular file owned by the current user that nobody else can
//! write. Warnings name the variable or key, never a value or the file's
//! content, and each is printed at most once per process.

use crate::commands::profile::fsx;
use serde_json::{Map, Value};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const ENABLED_ENV: &str = "AMPLIHACK_USAGE_LIMIT_WAIT";
pub const MAX_WAIT_ENV: &str = "AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS";
/// The key in `$HOME/.amplihack/config`.
pub const KEY: &str = "usage_limit_wait";
const MAX_FILE: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitSettings {
    pub enabled: bool,
    pub max_wait_secs: Option<u64>,
}

impl Default for WaitSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            max_wait_secs: None,
        }
    }
}

/// What reading the config file gave.
#[derive(Debug)]
pub enum FileRead {
    Missing,
    /// A symlink, not a regular file, too large, owned by another user or
    /// writable by others, or unreadable.
    Untrusted,
    Content(Vec<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Warn {
    BadEnabledVar,
    BadMaxVar,
    UntrustedFile,
    BadKey,
}

impl Warn {
    pub fn message(self) -> String {
        match self {
            Warn::BadEnabledVar => format!(
                "warning: {ENABLED_ENV} must be 1, true, on, yes, 0, false, off or no; ignoring it"
            ),
            Warn::BadMaxVar => format!(
                "warning: {MAX_WAIT_ENV} must be a whole number of seconds with no sign; ignoring it"
            ),
            Warn::UntrustedFile => format!(
                "warning: ~/.amplihack/config is not a private regular file of at most 64 KiB \
                 owned by you; {KEY} uses its defaults (fix with `chmod go-w ~/.amplihack/config`)"
            ),
            Warn::BadKey => format!(
                "warning: {KEY} in ~/.amplihack/config is not an object of \
                 {{\"enabled\": bool, \"max_wait_secs\": seconds}}; using its defaults"
            ),
        }
    }

    fn printed(self) -> &'static AtomicBool {
        static FLAGS: [AtomicBool; 4] = [const { AtomicBool::new(false) }; 4];
        &FLAGS[self as usize]
    }
}

/// The settings for this launch, from the process environment and
/// `$HOME/.amplihack/config`. Prints each warning at most once per process.
pub fn resolve() -> WaitSettings {
    let var = |name: &str| std::env::var_os(name).map(|v| v.to_string_lossy().into_owned());
    let (enabled, max) = (var(ENABLED_ENV), var(MAX_WAIT_ENV));
    let file = match home() {
        Some(home) => read_file(&home),
        None => FileRead::Missing,
    };
    let (settings, warns) = resolve_from(enabled.as_deref(), max.as_deref(), file);
    for w in warns {
        if !w.printed().swap(true, Ordering::Relaxed) {
            eprintln!("{}", w.message());
        }
    }
    settings
}

fn home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(Into::into)
}

/// Pure resolution from the two variables' raw values and the file.
pub fn resolve_from(
    enabled_var: Option<&str>,
    max_var: Option<&str>,
    file: FileRead,
) -> (WaitSettings, Vec<Warn>) {
    let mut warns = Vec::new();
    let enabled_env = match enabled_var.filter(|v| !v.is_empty()) {
        None => None,
        Some(v) => {
            let parsed = parse_bool(v);
            if parsed.is_none() {
                warns.push(Warn::BadEnabledVar);
            }
            parsed
        }
    };
    let max_env = match max_var.filter(|v| !v.is_empty()) {
        None => None,
        Some(v) => {
            let parsed = parse_secs(v);
            if parsed.is_none() {
                warns.push(Warn::BadMaxVar);
            }
            parsed
        }
    };
    let from_file = match file {
        FileRead::Missing => FileKey::default(),
        FileRead::Untrusted => {
            warns.push(Warn::UntrustedFile);
            FileKey::default()
        }
        FileRead::Content(raw) => file_key(&raw).unwrap_or_else(|| {
            warns.push(Warn::BadKey);
            FileKey::default()
        }),
    };
    let defaults = WaitSettings::default();
    let settings = WaitSettings {
        enabled: enabled_env
            .or(from_file.enabled)
            .unwrap_or(defaults.enabled),
        max_wait_secs: max_env.or(from_file.max_wait_secs),
    };
    (settings, warns)
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Some(true),
        "0" | "false" | "off" | "no" => Some(false),
        _ => None,
    }
}

/// Non-empty ASCII digits only: `u64::from_str` would also take a `+`.
fn parse_secs(v: &str) -> Option<u64> {
    if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    v.parse().ok()
}

#[derive(Default)]
struct FileKey {
    enabled: Option<bool>,
    max_wait_secs: Option<u64>,
}

/// The key's fields; `None` when the content or the key is malformed. A
/// missing key or field is not malformed.
fn file_key(raw: &[u8]) -> Option<FileKey> {
    let Value::Object(top) = serde_json::from_slice::<Value>(raw).ok()? else {
        return None;
    };
    let Some(key) = top.get(KEY) else {
        return Some(FileKey::default());
    };
    let key = key.as_object()?;
    let enabled = match key.get("enabled") {
        None => None,
        Some(v) => Some(v.as_bool()?),
    };
    let max_wait_secs = match key.get("max_wait_secs") {
        None => None,
        Some(v) => Some(v.as_u64()?),
    };
    Some(FileKey {
        enabled,
        max_wait_secs,
    })
}

/// Reads `$HOME/.amplihack/config` with the trust checks.
pub fn read_file(home: &Path) -> FileRead {
    let path = home.join(".amplihack").join("config");
    match fsx::read_bounded_nofollow(&path, MAX_FILE) {
        Ok(None) => FileRead::Missing,
        Ok(Some((raw, meta))) if meta.uid() == fsx::euid() && !fsx::writable_by_others(&meta) => {
            FileRead::Content(raw)
        }
        Ok(Some(_)) => FileRead::Untrusted,
        Err(e) if e.kind() == io::ErrorKind::NotADirectory => FileRead::Missing,
        Err(_) => FileRead::Untrusted,
    }
}

/// `$HOME/.amplihack`, created at `0700` when missing. An existing one must
/// be a real directory (not a symlink) owned by the current user that
/// group and others cannot write; it is never `chmod`ed. Shared by `write`
/// and the usage-limit wait's scratch directory.
pub(crate) fn private_amplihack_dir(home: &Path) -> io::Result<PathBuf> {
    let dir = home.join(".amplihack");
    match std::fs::symlink_metadata(&dir) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => fsx::ensure_private_dir(&dir)?,
        Err(e) => return Err(e),
        Ok(m) if m.file_type().is_symlink() || !m.is_dir() => {
            return Err(fsx::invalid("~/.amplihack is not a real directory"));
        }
        Ok(m) if m.uid() != fsx::euid() => {
            return Err(fsx::invalid("~/.amplihack is owned by another user"));
        }
        Ok(m) if fsx::writable_by_others(&m) => {
            return Err(fsx::invalid("~/.amplihack is writable by other users"));
        }
        Ok(_) => {}
    }
    Ok(dir)
}

/// Sets the `usage_limit_wait` key of `$HOME/.amplihack/config`, keeping
/// every other key. Written atomically through a private temporary file and
/// left at mode `0600`. Refuses a `~/.amplihack` that fails
/// [`private_amplihack_dir`], a symlinked file, a file owned by another
/// user, a file over 64 KiB and content that is not a JSON object. Takes no
/// lock: a concurrent saved-preference write can lose one of the updates.
pub fn write(home: &Path, settings: &WaitSettings) -> io::Result<()> {
    let dir = private_amplihack_dir(home)?;
    let mut top = match fsx::read_bounded_nofollow(&dir.join("config"), MAX_FILE)? {
        None => Map::new(),
        Some((_, meta)) if meta.uid() != fsx::euid() => {
            return Err(fsx::invalid("~/.amplihack/config is owned by another user"));
        }
        Some((raw, _)) => match serde_json::from_slice::<Value>(&raw) {
            Ok(Value::Object(m)) => m,
            _ => return Err(fsx::invalid("~/.amplihack/config is not a JSON object")),
        },
    };
    let mut key = Map::new();
    key.insert("enabled".into(), Value::Bool(settings.enabled));
    if let Some(max) = settings.max_wait_secs {
        key.insert("max_wait_secs".into(), Value::from(max));
    }
    top.insert(KEY.into(), Value::Object(key));
    let mut bytes = serde_json::to_vec_pretty(&Value::Object(top)).map_err(io::Error::other)?;
    bytes.push(b'\n');
    fsx::write_private_atomic(&dir, "config", &bytes)
}
