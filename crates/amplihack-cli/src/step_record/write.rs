//! Issue #2018: writing a step record, and removing it when the step ends.
//!
//! The record is written with a temporary file and a rename, so a reader
//! never sees half a record. Directories are created `0700` and the record
//! `0600`, at creation and never by a later `chmod`. The root and run-id
//! directory are checked without following symlinks, and a symlink, a
//! non-directory, a directory owned by someone else or one writable by group
//! or others is refused.

use super::{MAX_RECORD_BYTES, StepRecord, effective_uid, record_path, valid_run_id};
use anyhow::{Result, anyhow, bail};
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// Owns one written record. Dropping it removes the record, then the run-id
/// directory if that is now empty. It never removes anything else.
#[derive(Debug)]
pub struct StepRecordGuard {
    path: PathBuf,
    run_dir: PathBuf,
    uid: u32,
}

impl StepRecordGuard {
    /// Writes `record` under `root` as this process's effective uid.
    ///
    /// # Errors
    ///
    /// The error text starts with one of the fixed reasons in the reference
    /// page's Diagnostics table: `invalid recipe run id`, `record too large`,
    /// `unsafe directory <path>`, `cannot create <path>` or
    /// `cannot write <path>`. The run id itself is never included.
    pub fn try_write(root: &Path, record: &StepRecord) -> Result<Self> {
        Self::try_write_as(root, record, effective_uid())
    }

    /// [`Self::try_write`] with the uid every directory must belong to. The
    /// uid is a parameter only so that tests can stand in for another user.
    pub fn try_write_as(root: &Path, record: &StepRecord, uid: u32) -> Result<Self> {
        if !valid_run_id(&record.run_id) {
            bail!("invalid recipe run id");
        }
        let body = serde_json::to_vec_pretty(record)
            .map_err(|e| anyhow!("cannot serialise the record: {e}"))?;
        if body.len() > MAX_RECORD_BYTES {
            bail!(
                "record too large ({} bytes, limit {MAX_RECORD_BYTES})",
                body.len()
            );
        }

        ensure_private_dir(root, uid)?;
        let run_dir = root.join(&record.run_id);
        ensure_private_dir(&run_dir, uid)?;

        let path = record_path(root, &record.run_id, record.launcher.pid);
        write_new_then_rename(&path, &body)?;
        Ok(Self { path, run_dir, uid })
    }

    /// The record this guard wrote.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StepRecordGuard {
    fn drop(&mut self) {
        // Recheck: if the run-id directory has been swapped for a symlink or
        // someone else's directory since the write, nothing is removed.
        let still_ours = fs::symlink_metadata(&self.run_dir)
            .is_ok_and(|meta| meta.file_type().is_dir() && meta.uid() == self.uid);
        if !still_ours {
            tracing::debug!("step record directory changed; leaving it alone");
            return;
        }
        // Both may fail (the record was removed by hand, or the directory
        // holds another step's record). Neither is worth reporting.
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_dir(&self.run_dir);
    }
}

/// Uses `dir` if it is safe, or creates it `0700` if it is missing.
fn ensure_private_dir(dir: &Path, uid: u32) -> Result<()> {
    match fs::symlink_metadata(dir) {
        Ok(meta) => return check_private_dir(dir, &meta, uid),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => bail!("cannot create {}: {e}", dir.display()),
    }
    if let Some(parent) = dir.parent() {
        // HOME and the directories between it and recipe-steps are trusted,
        // so a symlinked ~/.amplihack keeps working.
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(|e| anyhow!("cannot create {}: {e}", parent.display()))?;
    }
    match fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        // Another step created it first: check it like any existing one.
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => bail!("cannot create {}: {e}", dir.display()),
    }
    let meta =
        fs::symlink_metadata(dir).map_err(|e| anyhow!("cannot create {}: {e}", dir.display()))?;
    check_private_dir(dir, &meta, uid)
}

/// Refuses a symlink, a non-directory, a directory owned by another uid, or
/// one writable by group or others. The mode is never changed.
pub(super) fn check_private_dir(dir: &Path, meta: &fs::Metadata, uid: u32) -> Result<()> {
    let safe = meta.file_type().is_dir() && meta.uid() == uid && meta.mode() & 0o022 == 0;
    if safe {
        Ok(())
    } else {
        Err(anyhow!("unsafe directory {}", dir.display()))
    }
}

/// Writes `body` to a new `0600` temporary file beside `path`, flushes it,
/// and renames it into place. The temporary file is removed on any failure.
fn write_new_then_rename(path: &Path, body: &[u8]) -> Result<()> {
    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(format!(".{}.tmp", uuid::Uuid::new_v4().simple()));
    let tmp = PathBuf::from(tmp_name);

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&tmp)
        .map_err(|e| anyhow!("cannot write {}: {e}", path.display()))?;
    let flushed = file.write_all(body).and_then(|()| file.sync_all());
    drop(file);
    let written = flushed.and_then(|()| fs::rename(&tmp, path));
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        bail!("cannot write {}: {e}", path.display());
    }
    Ok(())
}
