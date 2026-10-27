//! The backup directories of hand-change recovery (issue #1693):
//! `<root>/.rejected-logins/rotate-<M>-<YYYYMMDDTHHMMSSZ>[-<k>]/`, each
//! created fresh and private, never through a symlink, and never reused.
//! Directories only: no login is touched here.

use super::log::sh_quote;
use crate::commands::profile::auto_switch::kit::{euid, exists};
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

/// Why no backup directory could be used. Each refuses the move.
#[derive(Debug)]
pub enum BackupRefusal {
    /// All 99 names for this profile and second are taken.
    NoFreeName,
    /// The directory is not a real directory owned by this user with no
    /// access for anyone else.
    NotPrivate(PathBuf),
    /// Creating or checking it failed.
    Io(std::io::Error),
}

impl BackupRefusal {
    /// The `recovery refused:` reason.
    pub fn reason(&self) -> String {
        match self {
            Self::NoFreeName => "no free backup name".into(),
            Self::NotPrivate(d) => format!(
                "backup directory is not private: {}",
                sh_quote(d).unwrap_or_else(|| "(its path cannot be printed)".into())
            ),
            Self::Io(e) => format!("cannot create the backup directory: {e}"),
        }
    }
}

/// A real directory owned by this user that nobody else can use.
fn private(dir: &Path) -> std::io::Result<bool> {
    let md = std::fs::symlink_metadata(dir)?;
    Ok(md.is_dir() && md.uid() == euid() && md.mode() & 0o077 == 0)
}

/// Creates the first free backup directory, never through a symlink, and
/// checks it is a private directory owned by this user. One that fails the
/// check is removed again (it is new and empty).
pub fn fresh_backup(rejected: &Path, m: &ProfileName, t: i64) -> Result<PathBuf, BackupRefusal> {
    for k in 1..=99 {
        let dir = rejected.join(backup_name(m, t, k));
        match create_fresh_dir(&dir) {
            Ok(()) => {
                if private(&dir).map_err(BackupRefusal::Io)? {
                    return Ok(dir);
                }
                if let Err(e) = std::fs::remove_dir(&dir) {
                    eprintln!("warning: {} was not removed: {e}", dir.display());
                }
                return Err(BackupRefusal::NotPrivate(dir));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(BackupRefusal::Io(e)),
        }
    }
    Err(BackupRefusal::NoFreeName)
}

/// `.rejected-logins`, created `0700` when missing; one that exists must be
/// a real directory owned by this user with no access for anyone else, as
/// `profile login` requires.
pub fn rejected_dir(dir: &Path) -> Result<PathBuf, BackupRefusal> {
    match create_fresh_dir(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => {
            return Err(BackupRefusal::Io(e));
        }
        _ => {}
    }
    match private(dir) {
        Ok(true) => Ok(dir.to_path_buf()),
        Ok(false) => Err(BackupRefusal::NotPrivate(dir.to_path_buf())),
        Err(e) => Err(BackupRefusal::Io(e)),
    }
}

/// The backup [`fresh_backup`] would create now, for `--dry-run`.
pub fn first_free(rejected: &Path, m: &ProfileName, t: i64) -> Option<PathBuf> {
    (1..=99)
        .map(|k| rejected.join(backup_name(m, t, k)))
        .find(|d| !exists(d))
}
