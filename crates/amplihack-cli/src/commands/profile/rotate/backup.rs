//! The backup directories of hand-change recovery (issue #1693):
//! `<root>/.rejected-logins/rotate-<M>-<YYYYMMDDTHHMMSSZ>[-<k>]/`, each
//! created fresh and private, never through a symlink, and never reused.
//! Directories only: no login is touched here.

use crate::commands::profile::auto_switch::kit::{euid, exists, writable_by_others};
use crate::commands::profile::login_check::create_fresh_dir;
use crate::commands::profile::name::ProfileName;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// `t` as `YYYYMMDDTHHMMSSZ`.
pub fn stamp(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0).map_or_else(
        || "19700101T000000Z".to_string(),
        |d| d.format("%Y%m%dT%H%M%SZ").to_string(),
    )
}

fn backup_name(m: &ProfileName, t: i64, k: u32) -> String {
    match k {
        1 => format!("rotate-{m}-{}", stamp(t)),
        k => format!("rotate-{m}-{}-{k}", stamp(t)),
    }
}

/// Creates the first free backup directory, never through a symlink, and
/// checks it is a private directory owned by this user.
pub fn fresh_backup(rejected: &Path, m: &ProfileName, t: i64) -> Option<PathBuf> {
    for k in 1..=99 {
        let dir = rejected.join(backup_name(m, t, k));
        match create_fresh_dir(&dir) {
            Ok(()) => {
                let private = std::fs::symlink_metadata(&dir)
                    .is_ok_and(|md| md.is_dir() && md.uid() == euid() && md.mode() & 0o077 == 0);
                return private.then_some(dir);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                eprintln!("warning: backup directory {}: {e}", dir.display());
                return None;
            }
        }
    }
    None
}

/// `.rejected-logins`, created `0700` when missing; one that exists must be
/// a real directory owned by this user that nobody else can write to (each
/// backup in it is `0700`).
pub fn rejected_dir(dir: &Path) -> std::io::Result<()> {
    match create_fresh_dir(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(e),
        _ => {}
    }
    let m = std::fs::symlink_metadata(dir)?;
    if m.file_type().is_symlink() || !m.is_dir() || m.uid() != euid() || writable_by_others(&m) {
        return Err(std::io::Error::other(
            "is not a directory only you can write to",
        ));
    }
    Ok(())
}

/// The backup [`fresh_backup`] would create now, for `--dry-run`.
pub fn first_free(rejected: &Path, m: &ProfileName, t: i64) -> Option<PathBuf> {
    (1..=99)
        .map(|k| rejected.join(backup_name(m, t, k)))
        .find(|d| !exists(d))
}
