//! Issue #1976 (D10): `orphaned.json` in the auto-drive state directory.
//!
//! Written only into an absolute directory that is not a symlink, is owned
//! by the current user and has no group or other write bit (C-S9). The file
//! is created as a 0600 temp file with `O_NOFOLLOW`, fsynced, then renamed
//! into place (C-S10). It holds only the minimal fields (C-S11).

use anyhow::{Context, Result, bail};
use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use std::path::Path;

/// The state directory auto-drive hands a nested run.
pub(crate) const AUTODRIVE_STATE_DIR_ENV: &str = "AMPLIHACK_AUTODRIVE_STATE_DIR";
/// The file name inside the state directory.
pub(crate) const ORPHAN_RECORD_FILE: &str = "orphaned.json";

/// What the caller knows about the orphan stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OrphanRecord {
    pub(crate) parent_pid: i32,
    pub(crate) tree_id: String,
    pub(crate) run_id: String,
}

#[derive(Serialize)]
struct OrphanRecordFile<'a> {
    parent_pid: i32,
    tree_id: &'a str,
    run_id: &'a str,
    reason: &'a str,
    timestamp: String,
}

/// Write `orphaned.json` into `dir`. Every refused directory is an `Err`;
/// nothing is written then.
#[cfg(unix)]
pub(crate) fn write_orphan_record(dir: &Path, record: &OrphanRecord) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    if !dir.is_absolute() {
        bail!("state dir {} is not an absolute path", dir.display());
    }
    let meta = std::fs::symlink_metadata(dir)
        .with_context(|| format!("cannot inspect state dir {}", dir.display()))?;
    if meta.file_type().is_symlink() {
        bail!("state dir {} is a symlink", dir.display());
    }
    if !meta.is_dir() {
        bail!("state dir {} is not a directory", dir.display());
    }
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    if meta.uid() != uid {
        bail!("state dir {} is not owned by uid {uid}", dir.display());
    }
    if meta.mode() & 0o022 != 0 {
        bail!(
            "state dir {} is writable by group or others (mode {:o})",
            dir.display(),
            meta.mode() & 0o777
        );
    }

    let body = serde_json::to_string(&OrphanRecordFile {
        parent_pid: record.parent_pid,
        tree_id: &record.tree_id,
        run_id: &record.run_id,
        reason: super::correlation::ORPHAN_REASON,
        timestamp: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
    })
    .context("cannot serialise orphaned.json")?;

    let temp = dir.join(format!(".{ORPHAN_RECORD_FILE}.{}.tmp", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temp)
        .with_context(|| format!("cannot create {}", temp.display()))?;
    let written = file
        .write_all(body.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(error).with_context(|| format!("cannot write {}", temp.display()));
    }
    let target = dir.join(ORPHAN_RECORD_FILE);
    if let Err(error) = std::fs::rename(&temp, &target) {
        let _ = std::fs::remove_file(&temp);
        return Err(error).with_context(|| format!("cannot rename into {}", target.display()));
    }
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn write_orphan_record(dir: &Path, _record: &OrphanRecord) -> Result<()> {
    bail!(
        "orphaned.json is only written on unix (state dir {})",
        dir.display()
    )
}

/// Write the record into [`AUTODRIVE_STATE_DIR_ENV`] when it is set. A
/// refused directory is a warning: the final record and the stderr line are
/// written whatever happens here.
pub(crate) fn write_to_state_dir(record: &OrphanRecord) {
    let Some(dir) = std::env::var_os(AUTODRIVE_STATE_DIR_ENV).filter(|d| !d.is_empty()) else {
        return;
    };
    if let Err(error) = write_orphan_record(Path::new(&dir), record) {
        tracing::warn!(error = %format!("{error:#}"), "orphaned.json not written");
        eprintln!("warning: orphaned.json not written: {error:#}");
    }
}
