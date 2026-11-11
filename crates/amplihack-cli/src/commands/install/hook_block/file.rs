//! The file side of [`super`]: bounded no-follow reads, owner and link
//! checks, private atomic writes, and reasons that hold no file content.

use super::{Drift, HookTarget, drift, merge};
use serde_json::Value;
use std::path::Path;

/// Larger settings files are refused, as when seeding a profile.
pub(crate) const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;

/// What a file update did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FileMerge {
    Written,
    AlreadyCurrent,
    /// No file; none was created.
    Absent,
    Unparseable(String),
    Refused(String),
}

/// A file's state against a [`HookTarget`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FileDrift {
    Clean,
    Absent,
    Unparseable(String),
    Refused(String),
    Drift(Drift),
}

/// Why a file was not used.
enum Failure {
    Absent,
    Unparseable(String),
    Refused(String),
}

impl From<Failure> for FileMerge {
    fn from(f: Failure) -> Self {
        match f {
            Failure::Absent => Self::Absent,
            Failure::Unparseable(r) => Self::Unparseable(r),
            Failure::Refused(r) => Self::Refused(r),
        }
    }
}

/// `Some(reason)` when a file owned by `file_uid` may not be written by
/// `euid`.
pub(crate) fn owner_refusal(file_uid: u32, euid: u32) -> Option<String> {
    (file_uid != euid).then(|| "owned by another user".to_string())
}

/// An I/O error as a reason: the fixed phrase of a refusal raised by the
/// secure-file helpers, or the error kind. Never the file's content.
fn reason(error: &std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::InvalidData {
        error.to_string()
    } else {
        error.kind().to_string()
    }
}

/// A parse error as a position only: serde's message can quote the input.
fn parse_reason(error: &serde_json::Error) -> String {
    format!(
        "invalid JSON at line {} column {}",
        error.line(),
        error.column()
    )
}

/// Reads and parses `path`, which must be a private-to-us regular file with
/// one link and a JSON object at its root.
#[cfg(unix)]
fn load(path: &Path) -> Result<(Vec<u8>, Value), Failure> {
    use amplihack_utils::secure_files::{euid, read_bounded_nofollow};
    use std::os::unix::fs::MetadataExt;
    let (raw, meta) = match read_bounded_nofollow(path, MAX_SETTINGS_BYTES) {
        Ok(Some(found)) => found,
        Ok(None) => return Err(Failure::Absent),
        Err(error) => return Err(Failure::Refused(reason(&error))),
    };
    if let Some(why) = owner_refusal(meta.uid(), euid()) {
        return Err(Failure::Refused(why));
    }
    if meta.nlink() != 1 {
        return Err(Failure::Refused("has more than one hard link".into()));
    }
    match serde_json::from_slice::<Value>(&raw) {
        Ok(value @ Value::Object(_)) => Ok((raw, value)),
        Ok(_) => Err(Failure::Unparseable("not a JSON object".into())),
        Err(error) => Err(Failure::Unparseable(parse_reason(&error))),
    }
}

#[cfg(not(unix))]
fn load(_path: &Path) -> Result<(Vec<u8>, Value), Failure> {
    Err(Failure::Refused("unix-only".into()))
}

fn pretty(value: &Value) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|e| format!("could not serialise settings ({:?})", e.classify()))?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// The directory and file name of `path`.
fn split(path: &Path) -> Result<(&Path, &str), String> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "file name is not UTF-8".to_string())?;
    Ok((dir, name))
}

#[cfg(unix)]
fn replace(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let (dir, name) = split(path)?;
    amplihack_utils::secure_files::write_private_atomic(dir, name, bytes)
        .map_err(|e| format!("write failed: {}", reason(&e)))
}

#[cfg(not(unix))]
fn replace(_path: &Path, _bytes: &[u8]) -> Result<(), String> {
    Err("unix-only".into())
}

/// Writes `raw` to a new `backup` (mode 0600), never following or replacing
/// anything already there.
#[cfg(unix)]
fn write_backup(backup: &Path, raw: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let mut file = opts
        .open(backup)
        .map_err(|e| format!("backup not written: {}", reason(&e)))?;
    file.write_all(raw)
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("backup not written: {}", reason(&e)))
}

#[cfg(not(unix))]
fn write_backup(_backup: &Path, _raw: &[u8]) -> Result<(), String> {
    Err("unix-only".into())
}

fn update_inner(path: &Path, backup: Option<&Path>, update: &dyn Fn(&mut Value)) -> FileMerge {
    let (raw, mut value) = match load(path) {
        Ok(found) => found,
        Err(failure) => return failure.into(),
    };
    let before = value.clone();
    update(&mut value);
    if value == before {
        return FileMerge::AlreadyCurrent;
    }
    let bytes = match pretty(&value) {
        Ok(bytes) => bytes,
        Err(why) => return FileMerge::Refused(why),
    };
    if let Some(backup) = backup
        && let Err(why) = write_backup(backup, &raw)
    {
        return FileMerge::Refused(why);
    }
    match replace(path, &bytes) {
        Ok(()) => FileMerge::Written,
        Err(why) => FileMerge::Refused(why),
    }
}

/// Applies `update` to an existing settings file and writes it (0600) when
/// the result differs. Never creates the file.
pub(crate) fn update_file(path: &Path, update: &dyn Fn(&mut Value)) -> FileMerge {
    update_inner(path, None, update)
}

/// [`update_file`] that first writes the bytes it read to a new `backup`,
/// only when it is about to change the file. No backup, no write.
pub(crate) fn update_file_with_backup(
    path: &Path,
    backup: &Path,
    update: &dyn Fn(&mut Value),
) -> FileMerge {
    update_inner(path, Some(backup), update)
}

/// [`update_file`] with [`merge`].
pub(crate) fn merge_file(path: &Path, t: &HookTarget) -> FileMerge {
    update_file(path, &|settings| {
        merge(settings, t);
    })
}

/// Creates `path` holding `{}` with `update` applied, mode 0600. Never
/// replaces a file: one that appears meanwhile is `Refused`.
#[cfg(unix)]
pub(in crate::commands::install) fn create_file(
    path: &Path,
    update: &dyn Fn(&mut Value),
) -> FileMerge {
    let mut value = Value::Object(serde_json::Map::new());
    update(&mut value);
    let bytes = match pretty(&value) {
        Ok(bytes) => bytes,
        Err(why) => return FileMerge::Refused(why),
    };
    let (dir, name) = match split(path) {
        Ok(parts) => parts,
        Err(why) => return FileMerge::Refused(why),
    };
    let linked = amplihack_utils::secure_files::PrivateTemp::create(dir, name)
        .and_then(|temp| temp.link_new(&bytes));
    match linked {
        Ok(true) => FileMerge::Written,
        Ok(false) => FileMerge::Refused("appeared while it was being created".into()),
        Err(e) => FileMerge::Refused(format!("create failed: {}", reason(&e))),
    }
}

#[cfg(not(unix))]
pub(in crate::commands::install) fn create_file(
    _path: &Path,
    _update: &dyn Fn(&mut Value),
) -> FileMerge {
    FileMerge::Refused("unix-only".into())
}

/// `path`'s state against `t`. Read-only.
pub(crate) fn drift_file(path: &Path, t: &HookTarget) -> FileDrift {
    match load(path) {
        Ok((_, value)) => {
            let d = drift(&value, t);
            if d.is_clean() {
                FileDrift::Clean
            } else {
                FileDrift::Drift(d)
            }
        }
        Err(Failure::Absent) => FileDrift::Absent,
        Err(Failure::Unparseable(r)) => FileDrift::Unparseable(r),
        Err(Failure::Refused(r)) => FileDrift::Refused(r),
    }
}
