//! Issue #2018: reading every step record under the root (D10).
//!
//! Pure and infallible: whatever cannot be read becomes an
//! [`Listed::Unreadable`] entry with one of the fixed reasons in the
//! reference page's "Which entries are listed" table. Record files are
//! opened with `O_NOFOLLOW | O_NONBLOCK` and checked with `fstat`, so a
//! symlink is never followed and a FIFO never blocks the listing.
//!
//! This module only reads. It never signals a process or removes a file,
//! and nothing may do so on the strength of what it returns alone: a tool
//! that acts on a record needs a fresh uid, start-time and ancestry check
//! and its own review.

use super::{MAX_RECORD_BYTES, StepRecord, effective_uid, pid_from_record_name, valid_run_id};
use serde_json::Value;
use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// One entry found under the root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listed {
    /// A record that was read and passed every check.
    Record { path: PathBuf, record: StepRecord },
    /// A record file, run-id directory or the root itself that could not be
    /// read, with the reason.
    Unreadable { path: PathBuf, reason: String },
}

/// Every entry under `root`, checking run-id directories against this
/// process's effective uid. See [`list_as`].
pub fn list(root: &Path) -> Vec<Listed> {
    list_as(root, effective_uid())
}

/// Every entry under `root`, sorted by run id, then with the run-id
/// directory's own entry first, then by launcher pid as a number.
///
/// A missing root lists nothing. A root that cannot be examined or listed
/// is the only entry. `uid` is the owner every run-id directory must have;
/// it is a parameter only so that tests can stand in for another user.
pub fn list_as(root: &Path, uid: u32) -> Vec<Listed> {
    match fs::symlink_metadata(root) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => return vec![cannot_read(root, &e)],
        Ok(meta) if !meta.file_type().is_dir() => {
            // `recipe steps` refuses this case before listing; a caller that
            // did not still gets no record read through a symlink.
            return vec![unreadable(
                root,
                format!("unsafe directory {}", root.display()),
            )];
        }
        Ok(_) => {}
    }
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) => return vec![cannot_read(root, &e)],
    };

    // (run id, pid), with -1 for the run-id directory's own entry.
    let mut found: Vec<(String, i64, Listed)> = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => return vec![cannot_read(root, &e)],
        };
        let Some(run_id) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !valid_run_id(&run_id) {
            continue;
        }
        list_run_dir(&entry.path(), &run_id, uid, &mut found);
    }
    found.sort_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));
    found.into_iter().map(|(_, _, listed)| listed).collect()
}

/// Adds the records in one run-id directory to `found`, or one entry for
/// the directory itself when it is not descended.
pub(super) fn list_run_dir(
    dir: &Path,
    run_id: &str,
    uid: u32,
    found: &mut Vec<(String, i64, Listed)>,
) {
    let mut directory_entry = |listed: Listed| found.push((run_id.to_string(), -1, listed));
    let meta = match fs::symlink_metadata(dir) {
        Ok(meta) => meta,
        Err(e) => return directory_entry(cannot_read(dir, &e)),
    };
    if meta.file_type().is_symlink() {
        return directory_entry(unreadable(dir, "run directory is a symlink"));
    }
    if !meta.file_type().is_dir() {
        return;
    }
    if meta.uid() != uid {
        return directory_entry(unreadable(dir, "run directory is not owned by this user"));
    }
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => return directory_entry(cannot_read(dir, &e)),
    };
    let mut records = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => return directory_entry(cannot_read(dir, &e)),
        };
        let Some(pid) = entry.file_name().to_str().and_then(pid_from_record_name) else {
            continue;
        };
        let path = entry.path();
        // A directory named like a record is ignored; every other kind of
        // entry is opened, without following it, and judged by fstat.
        if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_dir()) {
            continue;
        }
        let listed = read_record(&path, run_id, pid);
        records.push((run_id.to_string(), i64::from(pid), listed));
    }
    found.extend(records);
}

/// One record file, read and checked against its path.
pub(super) fn read_record(path: &Path, run_id: &str, pid: i32) -> Listed {
    match read_capped(path).and_then(|bytes| parse_record(&bytes, run_id, pid)) {
        Ok(record) => Listed::Record {
            path: path.to_path_buf(),
            record,
        },
        Err(reason) => unreadable(path, reason),
    }
}

/// At most [`MAX_RECORD_BYTES`] from a regular file, opened without
/// following a symlink and without blocking on a FIFO.
fn read_capped(path: &Path) -> Result<Vec<u8>, String> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| match e.raw_os_error() {
            Some(libc::ELOOP) => "not a regular file".to_string(),
            _ => format!("cannot read: {e}"),
        })?;
    let meta = file.metadata().map_err(|e| format!("cannot read: {e}"))?;
    if !meta.file_type().is_file() {
        return Err("not a regular file".to_string());
    }
    if meta.len() > MAX_RECORD_BYTES as u64 {
        return Err("record too large".to_string());
    }
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read: {e}"))?;
    if bytes.len() > MAX_RECORD_BYTES {
        return Err("record too large".to_string());
    }
    Ok(bytes)
}

/// Two stages: `format` is checked before any other field, so a later
/// format may look nothing like this one; then the format-1 fields.
fn parse_record(bytes: &[u8], run_id: &str, pid: i32) -> Result<StepRecord, String> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| "invalid JSON".to_string())?;
    let Some(object) = value.as_object() else {
        return Err("invalid JSON".to_string());
    };
    match object.get("format") {
        None => return Err("missing format".to_string()),
        Some(format) if format.as_u64() == Some(u64::from(super::FORMAT)) => {}
        Some(format) => {
            return Err(match (format.as_i64(), format.as_u64()) {
                (Some(n), _) => format!("unsupported format {n}"),
                (None, Some(n)) => format!("unsupported format {n}"),
                (None, None) => "invalid JSON".to_string(),
            });
        }
    }
    let record: StepRecord =
        serde_json::from_value(value).map_err(|_| "invalid JSON".to_string())?;
    let pids_valid = record.launcher.pid >= 1
        && record.agent.pid >= 1
        && record.parent.as_ref().is_none_or(|p| p.pid >= 1);
    if !pids_valid {
        return Err("invalid pid".to_string());
    }
    if record.run_id != run_id || record.launcher.pid != pid {
        return Err("record does not match its path".to_string());
    }
    Ok(record)
}

fn unreadable(path: &Path, reason: impl Into<String>) -> Listed {
    Listed::Unreadable {
        path: path.to_path_buf(),
        reason: reason.into(),
    }
}

fn cannot_read(path: &Path, error: &io::Error) -> Listed {
    unreadable(path, format!("cannot read: {error}"))
}
