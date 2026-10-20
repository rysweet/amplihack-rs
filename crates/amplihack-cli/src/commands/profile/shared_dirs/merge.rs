//! Merges one real session directory of a profile into the shared store
//! (issue #1634). The walk is descriptor-based: every directory is opened
//! with `O_NOFOLLOW|O_DIRECTORY`, file types come from `fstatat` without
//! following or from `fstat` on the opened file, and every move is a
//! `renameat2(RENAME_NOREPLACE)` relative to the two directory descriptors.
//! Nothing in the shared store is renamed, overwritten or deleted, except
//! leftover temporary files with the reserved prefix.

use super::super::name::ProfileName;
use super::super::store::{fdwalk, renameat_noreplace};
use super::copy::{copy_across, same_bytes};
use super::memory;
use std::ffi::{OsStr, OsString};
use std::fs::{File, Metadata, Permissions};
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

const MAX_DEPTH: usize = 16;
/// Temporary copies in the shared store; the only files ever deleted there.
pub(super) const TMP_PREFIX: &[u8] = b".amplihack-share-tmp-";
const MAX_CONFLICT_SLOTS: usize = 1000;
pub(super) const DIR_FLAGS: libc::c_int = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_DIRECTORY;

#[derive(Debug, Default)]
pub(super) struct MergeOutcome {
    pub moved: usize,
    pub identical: usize,
    /// `(path relative to the shared dir, absolute name it was saved as)`.
    pub conflicts: Vec<(PathBuf, PathBuf)>,
    /// Credential-named files, relative to the shared dir, left in the source.
    pub logins: Vec<PathBuf>,
    /// Nested symlinks and special files left in the source.
    pub skipped: usize,
    /// Memory indexes (relative to the shared dir) that kept the shared copy
    /// and had this profile's missing entries appended, with their count.
    pub indexes: Vec<(PathBuf, usize)>,
}

#[derive(Debug)]
pub(super) enum MergeError {
    /// `step` is one of `open`, `read`, `copy`, `rename`, `mkdir`, `chmod`.
    Io {
        step: &'static str,
        err: io::Error,
    },
    NoSpace {
        path: PathBuf,
        bytes: u64,
    },
}

pub(super) fn fail(step: &'static str) -> impl FnOnce(io::Error) -> MergeError {
    move |err| MergeError::Io { step, err }
}

/// Login files, matched on the last component as bytes before any open.
pub(super) fn is_credential_name(name: &OsStr) -> bool {
    let b = name.as_bytes();
    b.starts_with(b".credentials") || b == b".claude.json"
}

/// Moves every regular file below `src` into `dst` (whose absolute path is
/// `dst_abs`), skipping credentials, symlinks, special files and reserved
/// temporary names. Files already moved are gone from `src`; everything else
/// stays there.
pub(super) fn merge_tree(
    src: &OwnedFd,
    dst: &OwnedFd,
    dst_abs: &Path,
    profile: &ProfileName,
) -> Result<MergeOutcome, MergeError> {
    let mut out = MergeOutcome::default();
    walk(src, dst, Path::new(""), dst_abs, 0, profile, &mut out)?;
    Ok(out)
}

fn walk(
    src: &OwnedFd,
    dst: &OwnedFd,
    rel: &Path,
    dst_abs: &Path,
    depth: usize,
    profile: &ProfileName,
    out: &mut MergeOutcome,
) -> Result<(), MergeError> {
    if depth > MAX_DEPTH {
        return Err(fail("open")(io::Error::other(format!(
            "{} is nested deeper than {MAX_DEPTH} directories",
            rel.display()
        ))));
    }
    remove_reserved_temps(dst)?;
    let memory_dir = memory::is_memory_dir(rel);
    let mut index = None;
    for name in fdwalk::list(src).map_err(fail("read"))? {
        let path = rel.join(&name);
        // Last, so the conflict names of the files it lists are known.
        if memory_dir && name == memory::INDEX {
            index = Some((name, path));
            continue;
        }
        if is_credential_name(&name) {
            out.logins.push(path);
            continue;
        }
        if name.as_bytes().starts_with(TMP_PREFIX) {
            continue;
        }
        let st = match stat_at(src.as_raw_fd(), &name) {
            Ok(st) => st,
            Err(e) if e.raw_os_error() == Some(libc::ENOENT) => continue,
            Err(e) => return Err(fail("open")(e)),
        };
        match st.st_mode & libc::S_IFMT {
            libc::S_IFDIR => merge_dir(src, dst, &name, &path, dst_abs, depth, profile, out)?,
            libc::S_IFREG => merge_file(src, dst, &name, &path, dst_abs, profile, out)?,
            _ => out.skipped += 1,
        }
    }
    match index {
        Some((name, path)) => memory::merge_index(src, dst, &name, &path, dst_abs, profile, out),
        None => Ok(()),
    }
}

#[allow(clippy::too_many_arguments)]
fn merge_dir(
    src: &OwnedFd,
    dst: &OwnedFd,
    name: &OsStr,
    path: &Path,
    dst_abs: &Path,
    depth: usize,
    profile: &ProfileName,
    out: &mut MergeOutcome,
) -> Result<(), MergeError> {
    let child = match fdwalk::openat(src.as_raw_fd(), name, DIR_FLAGS, 0) {
        Ok(fd) => fd,
        Err(e) if not_a_dir(&e) => {
            out.skipped += 1;
            return Ok(());
        }
        Err(e) => return Err(fail("open")(e)),
    };
    let c = fdwalk::cstr(name).map_err(fail("mkdir"))?;
    // SAFETY: a valid directory descriptor and NUL-terminated name.
    if unsafe { libc::mkdirat(dst.as_raw_fd(), c.as_ptr(), 0o700) } != 0 {
        let e = io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::EEXIST) {
            return Err(fail("mkdir")(e));
        }
    }
    let target = match fdwalk::openat(dst.as_raw_fd(), name, DIR_FLAGS, 0) {
        Ok(fd) => fd,
        // The shared store has a non-directory here: the subtree stays put.
        Err(e) if not_a_dir(&e) => {
            out.skipped += 1;
            return Ok(());
        }
        Err(e) => return Err(fail("open")(e)),
    };
    walk(
        &child,
        &target,
        path,
        &dst_abs.join(name),
        depth + 1,
        profile,
        out,
    )
}

pub(super) fn merge_file(
    src: &OwnedFd,
    dst: &OwnedFd,
    name: &OsStr,
    path: &Path,
    dst_abs: &Path,
    profile: &ProfileName,
    out: &mut MergeOutcome,
) -> Result<(), MergeError> {
    let Some((input, meta)) = open_regular(src.as_raw_fd(), name)? else {
        out.skipped += 1;
        return Ok(());
    };
    match place(&input, &meta, src, name, dst, name, path) {
        Ok(()) => {
            out.moved += 1;
            return Ok(());
        }
        Err(Placed::Exists) => {}
        Err(Placed::Fail(e)) => return Err(e),
    }
    if let Some((existing, _)) = open_regular(dst.as_raw_fd(), name)?
        && same_bytes(&input, &existing).map_err(fail("read"))?
    {
        out.identical += 1;
        return Ok(());
    }
    // Canonical always wins; this profile's copy goes to a conflict name.
    for _ in 0..MAX_CONFLICT_SLOTS {
        let (slot, held) = conflict_slot(dst, name, profile, &input)?;
        if !held {
            match place(&input, &meta, src, name, dst, &slot, path) {
                Ok(()) => {}
                Err(Placed::Exists) => continue,
                Err(Placed::Fail(e)) => return Err(e),
            }
        }
        out.conflicts.push((path.to_path_buf(), dst_abs.join(slot)));
        return Ok(());
    }
    Err(fail("rename")(io::Error::other(format!(
        "no free conflict name for {}",
        path.display()
    ))))
}

/// The first `<name>.conflict-<profile>[-N]` that is free, or that already
/// holds exactly these bytes (`true`).
fn conflict_slot(
    dst: &OwnedFd,
    name: &OsStr,
    profile: &ProfileName,
    input: &File,
) -> Result<(OsString, bool), MergeError> {
    for n in 1..=MAX_CONFLICT_SLOTS {
        let mut slot = name.to_os_string();
        slot.push(".conflict-");
        slot.push(profile.as_str());
        if n > 1 {
            slot.push(format!("-{n}"));
        }
        match stat_at(dst.as_raw_fd(), &slot) {
            Err(e) if e.raw_os_error() == Some(libc::ENOENT) => return Ok((slot, false)),
            Err(e) => return Err(fail("open")(e)),
            Ok(_) => {}
        }
        if let Some((held, _)) = open_regular(dst.as_raw_fd(), &slot)?
            && same_bytes(input, &held).map_err(fail("read"))?
        {
            return Ok((slot, true));
        }
    }
    Err(fail("rename")(io::Error::other("no free conflict name")))
}

pub(super) enum Placed {
    Exists,
    Fail(MergeError),
}

/// Moves `src/name` to `dst/to` without replacing anything, copying when the
/// two are on different filesystems.
fn place(
    input: &File,
    meta: &Metadata,
    src: &OwnedFd,
    name: &OsStr,
    dst: &OwnedFd,
    to: &OsStr,
    path: &Path,
) -> Result<(), Placed> {
    // The profile directory hid group/other bits; the shared store may not.
    // Owner-only before the move, so a failure leaves nothing half done.
    let private = meta.mode() & 0o700;
    if meta.mode() & 0o7777 != private {
        input
            .set_permissions(Permissions::from_mode(private))
            .map_err(|e| Placed::Fail(fail("chmod")(e)))?;
    }
    let err = if forced_exdev() {
        io::Error::from_raw_os_error(libc::EXDEV)
    } else {
        match renameat_noreplace(src.as_raw_fd(), name, dst.as_raw_fd(), to) {
            Ok(()) => return Ok(()),
            Err(e) => e,
        }
    };
    match err.raw_os_error() {
        Some(libc::EEXIST) => Err(Placed::Exists),
        // EXDEV comes before any check of `to`; look first so a name the
        // shared store already has is never copied. The no-replace rename at
        // the end of the copy still covers a name that appears meanwhile.
        Some(libc::EXDEV) => match stat_at(dst.as_raw_fd(), to) {
            Ok(_) => Err(Placed::Exists),
            Err(e) if e.raw_os_error() == Some(libc::ENOENT) => {
                copy_across(input, meta, src, name, dst, to, path)
            }
            Err(e) => Err(Placed::Fail(fail("open")(e))),
        },
        _ => Err(Placed::Fail(fail("rename")(err))),
    }
}

#[cfg(test)]
fn forced_exdev() -> bool {
    super::test_hook::force_exdev()
}

#[cfg(not(test))]
fn forced_exdev() -> bool {
    false
}

/// Removes leftover reserved temporary files from an interrupted copy, but
/// only regular files owned by the effective user.
fn remove_reserved_temps(dst: &OwnedFd) -> Result<(), MergeError> {
    let euid = super::super::auto_switch::euid();
    for name in fdwalk::list(dst).map_err(fail("read"))? {
        if !name.as_bytes().starts_with(TMP_PREFIX) {
            continue;
        }
        match stat_at(dst.as_raw_fd(), &name) {
            Ok(st) if st.st_mode & libc::S_IFMT == libc::S_IFREG && st.st_uid == euid => {
                match unlink_at(dst.as_raw_fd(), &name) {
                    Ok(()) => {}
                    Err(e) if e.raw_os_error() == Some(libc::ENOENT) => {}
                    Err(e) => return Err(fail("rename")(e)),
                }
            }
            Ok(_) => {}
            Err(e) if e.raw_os_error() == Some(libc::ENOENT) => {}
            Err(e) => return Err(fail("open")(e)),
        }
    }
    Ok(())
}

/// Opens `dir/name` without following; `None` unless it is a regular file.
fn open_regular(dir: RawFd, name: &OsStr) -> Result<Option<(File, Metadata)>, MergeError> {
    let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK;
    let file = match fdwalk::openat(dir, name, flags, 0) {
        Ok(fd) => File::from(fd),
        Err(e) if matches!(e.raw_os_error(), Some(libc::ELOOP | libc::ENOENT)) => {
            return Ok(None);
        }
        Err(e) => return Err(fail("open")(e)),
    };
    // The opened descriptor, not the earlier path stat, decides the type.
    let meta = file.metadata().map_err(fail("open"))?;
    Ok(meta.is_file().then_some((file, meta)))
}

/// `fstatat` without following the last component.
pub(super) fn stat_at(dir: RawFd, name: &OsStr) -> io::Result<libc::stat> {
    let c = fdwalk::cstr(name)?;
    let mut st = std::mem::MaybeUninit::<libc::stat>::zeroed();
    // SAFETY: valid path and out-pointer; initialised only on success.
    let rc = unsafe { libc::fstatat(dir, c.as_ptr(), st.as_mut_ptr(), libc::AT_SYMLINK_NOFOLLOW) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fstatat succeeded.
    Ok(unsafe { st.assume_init() })
}

/// `unlinkat` of a non-directory entry.
pub(super) fn unlink_at(dir: RawFd, name: &OsStr) -> io::Result<()> {
    let c = fdwalk::cstr(name)?;
    // SAFETY: a valid directory descriptor and NUL-terminated name.
    if unsafe { libc::unlinkat(dir, c.as_ptr(), 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn not_a_dir(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc::ELOOP | libc::ENOTDIR | libc::ENOENT)
    )
}
