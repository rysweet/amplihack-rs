//! Swaps a profile's merged real directory for the link to the shared store
//! (issue #1634). The old directory is kept as `<name>.pre-share-<UTC>[-N]`
//! and never deleted here.

use super::super::store::{exchange, rename_noreplace};
use super::merge::{stat_at, unlink_at};
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};

pub(super) enum Swap {
    /// The link is in place; the old directory is at this backup path.
    Done(PathBuf),
    /// Nothing changed: the real directory is still at `<name>`.
    RolledBack(io::Error),
    /// The old directory is at `backup` and `<name>` is missing.
    Stranded { err: io::Error, backup: PathBuf },
    /// The link is in place, but the old directory is still at `at`.
    Leftover { err: io::Error, at: PathBuf },
}

/// `<profile>/.<name>.share-tmp`, the temporary link.
fn temp_name(name: &str) -> OsString {
    OsString::from(format!(".{name}.share-tmp"))
}

pub(super) fn swap_in_link(dir: &OwnedFd, profile_dir: &Path, name: &str, target: &Path) -> Swap {
    let tmp = temp_name(name);
    let tmp_path = profile_dir.join(&tmp);
    let real = profile_dir.join(name);
    if let Err(e) = make_temp_link(dir, &tmp, target) {
        return Swap::RolledBack(e);
    }
    // Another process may have swapped it in meanwhile; a link is never
    // exchanged for a link and reported as a backup.
    match stat_at(dir.as_raw_fd(), OsStr::new(name)) {
        Ok(st) if st.st_mode & libc::S_IFMT == libc::S_IFDIR => {}
        Ok(_) => {
            let e = io::Error::other("it is no longer a directory");
            return rolled_back(dir, &tmp, e);
        }
        Err(e) => return rolled_back(dir, &tmp, e),
    }
    let backup = match backup_name(dir, name) {
        Ok(b) => profile_dir.join(b),
        Err(e) => return rolled_back(dir, &tmp, e),
    };
    match exchange(&tmp_path, &real) {
        Ok(true) => {
            return match rename_noreplace(&tmp_path, &backup) {
                Ok(()) => Swap::Done(backup),
                Err(err) => Swap::Leftover { err, at: tmp_path },
            };
        }
        Ok(false) => {}
        Err(e) => return rolled_back(dir, &tmp, e),
    }
    // No atomic exchange: two renames, the first undone if the second fails.
    if let Err(e) = rename_noreplace(&real, &backup) {
        return rolled_back(dir, &tmp, e);
    }
    match link_rename(&tmp_path, &real) {
        Ok(()) => Swap::Done(backup),
        Err(e) => match restore_rename(&backup, &real) {
            Ok(()) => rolled_back(dir, &tmp, e),
            Err(err) => Swap::Stranded { err, backup },
        },
    }
}

#[cfg(test)]
fn link_rename(from: &Path, to: &Path) -> io::Result<()> {
    if super::test_hook::fail_link_rename() {
        return Err(io::Error::other("link rename failed (test hook)"));
    }
    rename_noreplace(from, to)
}

#[cfg(not(test))]
fn link_rename(from: &Path, to: &Path) -> io::Result<()> {
    rename_noreplace(from, to)
}

#[cfg(test)]
fn restore_rename(from: &Path, to: &Path) -> io::Result<()> {
    if super::test_hook::fail_restore() {
        return Err(io::Error::other("restore rename failed (test hook)"));
    }
    rename_noreplace(from, to)
}

#[cfg(not(test))]
fn restore_rename(from: &Path, to: &Path) -> io::Result<()> {
    rename_noreplace(from, to)
}

fn rolled_back(dir: &OwnedFd, tmp: &OsStr, e: io::Error) -> Swap {
    if let Err(u) = unlink_at(dir.as_raw_fd(), tmp)
        && u.raw_os_error() != Some(libc::ENOENT)
    {
        tracing::warn!(
            "could not remove temporary link {}: {u}",
            tmp.to_string_lossy()
        );
    }
    Swap::RolledBack(e)
}

/// Creates the temporary link, first removing a stale one of that name.
/// Anything there that is not a symlink is left alone and refuses the swap.
fn make_temp_link(dir: &OwnedFd, tmp: &OsStr, target: &Path) -> io::Result<()> {
    match stat_at(dir.as_raw_fd(), tmp) {
        Ok(st) if st.st_mode & libc::S_IFMT == libc::S_IFLNK => unlink_at(dir.as_raw_fd(), tmp)?,
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} exists and is not a link", tmp.to_string_lossy()),
            ));
        }
        Err(e) if e.raw_os_error() == Some(libc::ENOENT) => {}
        Err(e) => return Err(e),
    }
    symlink_at(target, dir, tmp)
}

/// `symlinkat(target, dir, name)`; fails with `EEXIST` if `name` exists.
pub(super) fn symlink_at(target: &Path, dir: &OwnedFd, name: &OsStr) -> io::Result<()> {
    let t = super::super::store::fdwalk::cstr(target.as_os_str())?;
    let n = super::super::store::fdwalk::cstr(name)?;
    // SAFETY: valid NUL-terminated strings and a directory descriptor we own.
    if unsafe { libc::symlinkat(t.as_ptr(), dir.as_raw_fd(), n.as_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The first free `<name>.pre-share-<YYYYMMDDTHHMMSSZ>[-N]`.
fn backup_name(dir: &OwnedFd, name: &str) -> io::Result<OsString> {
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    for n in 1..=1000 {
        let candidate = if n == 1 {
            format!("{name}.pre-share-{stamp}")
        } else {
            format!("{name}.pre-share-{stamp}-{n}")
        };
        match stat_at(dir.as_raw_fd(), OsStr::new(&candidate)) {
            Err(e) if e.raw_os_error() == Some(libc::ENOENT) => return Ok(candidate.into()),
            Err(e) => return Err(e),
            Ok(_) => {}
        }
    }
    Err(io::Error::other("no free backup name"))
}

/// Single-quotes `p` for a shell command line.
pub(super) fn shell_quote(p: &Path) -> String {
    format!("'{}'", p.to_string_lossy().replace('\'', r"'\''"))
}
