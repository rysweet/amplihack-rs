//! The cross-filesystem copy and the byte comparison used by the merge
//! (issue #1634). A copy goes to an `O_EXCL` temporary file with the
//! reserved prefix and is renamed into place without replacing anything.

use super::super::store::{fdwalk, renameat_noreplace};
use super::merge::{MergeError, Placed, TMP_PREFIX, fail, unlink_at};
use std::ffi::{OsStr, OsString};
use std::fs::{File, Metadata, Permissions};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileExt, MetadataExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

const CHUNK: usize = 64 * 1024;

/// Copy to an `O_EXCL` temporary file, fsync, set mode (`& 0o700`) and
/// mtime, rename into place without replacing, fsync the directory, and
/// only then unlink the source.
pub(super) fn copy_across(
    input: &File,
    meta: &Metadata,
    src: &OwnedFd,
    name: &OsStr,
    dst: &OwnedFd,
    to: &OsStr,
    path: &Path,
) -> Result<(), Placed> {
    let bytes = meta.len();
    if free_bytes(dst).map_err(|e| Placed::Fail(fail("copy")(e)))? < bytes {
        return Err(Placed::Fail(MergeError::NoSpace {
            path: path.to_path_buf(),
            bytes,
        }));
    }
    let (tmp, mut output) = create_temp(dst).map_err(|e| Placed::Fail(fail("copy")(e)))?;
    let written = copy_bytes(input, &mut output, bytes)
        .and_then(|()| output.set_permissions(Permissions::from_mode(meta.mode() & 0o700)))
        .and_then(|()| output.set_modified(meta.modified()?))
        .and_then(|()| output.sync_all());
    drop(output);
    if let Err(e) = written {
        discard_temp(dst, &tmp);
        return Err(Placed::Fail(fail("copy")(e)));
    }
    if let Err(e) = renameat_noreplace(dst.as_raw_fd(), &tmp, dst.as_raw_fd(), to) {
        discard_temp(dst, &tmp);
        return Err(if e.raw_os_error() == Some(libc::EEXIST) {
            Placed::Exists
        } else {
            Placed::Fail(fail("rename")(e))
        });
    }
    // SAFETY: fsync on a directory descriptor we own.
    if unsafe { libc::fsync(dst.as_raw_fd()) } != 0 {
        return Err(Placed::Fail(fail("copy")(io::Error::last_os_error())));
    }
    #[cfg(test)]
    if super::test_hook::grow_during_copy() {
        grow(src, name);
    }
    // A writer appended meanwhile: the source is kept, so those bytes are
    // not lost; the next run saves it under a conflict name.
    let now = input
        .metadata()
        .map_err(|e| Placed::Fail(fail("copy")(e)))?;
    if now.len() != bytes || now.modified().ok() != meta.modified().ok() {
        return Err(Placed::Fail(fail("copy")(io::Error::other(format!(
            "{} changed during the copy; kept",
            path.display()
        )))));
    }
    unlink_at(src.as_raw_fd(), name).map_err(|e| Placed::Fail(fail("rename")(e)))
}

#[cfg(test)]
fn grow(src: &OwnedFd, name: &OsStr) {
    let flags = libc::O_WRONLY | libc::O_APPEND;
    let mut f = File::from(fdwalk::openat(src.as_raw_fd(), name, flags, 0).unwrap());
    f.write_all(b"+").unwrap();
}

fn create_temp(dst: &OwnedFd) -> io::Result<(OsString, File)> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW;
    for _ in 0..100 {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let mut tmp = OsString::from(OsStr::from_bytes(TMP_PREFIX));
        tmp.push(format!("{}-{n}", std::process::id()));
        match fdwalk::openat(dst.as_raw_fd(), &tmp, flags, 0o600) {
            Ok(fd) => return Ok((tmp, File::from(fd))),
            Err(e) if e.raw_os_error() == Some(libc::EEXIST) => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("no free temporary name"))
}

fn discard_temp(dst: &OwnedFd, tmp: &OsStr) {
    if let Err(e) = unlink_at(dst.as_raw_fd(), tmp) {
        tracing::warn!(
            "could not remove temporary file {}: {e}",
            tmp.to_string_lossy()
        );
    }
}

/// Size first, then a streamed comparison.
pub(super) fn same_bytes(a: &File, b: &File) -> io::Result<bool> {
    let len = a.metadata()?.len();
    if b.metadata()?.len() != len {
        return Ok(false);
    }
    let (mut x, mut y) = (vec![0u8; CHUNK], vec![0u8; CHUNK]);
    let mut off = 0u64;
    while off < len {
        let n = CHUNK.min((len - off) as usize);
        a.read_exact_at(&mut x[..n], off)?;
        b.read_exact_at(&mut y[..n], off)?;
        if x[..n] != y[..n] {
            return Ok(false);
        }
        off += n as u64;
    }
    Ok(true)
}

fn copy_bytes(input: &File, output: &mut File, len: u64) -> io::Result<()> {
    let mut buf = vec![0u8; CHUNK];
    let mut off = 0u64;
    while off < len {
        let want = CHUNK.min((len - off) as usize);
        let n = input.read_at(&mut buf[..want], off)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "source file shrank during the copy",
            ));
        }
        output.write_all(&buf[..n])?;
        off += n as u64;
    }
    Ok(())
}

#[allow(clippy::unnecessary_cast)]
fn free_bytes(dir: &OwnedFd) -> io::Result<u64> {
    #[cfg(test)]
    if super::test_hook::no_space() {
        return Ok(0);
    }
    let mut st = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY: a valid descriptor and out-pointer; read only on success.
    if unsafe { libc::fstatvfs(dir.as_raw_fd(), st.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fstatvfs succeeded.
    let st = unsafe { st.assume_init() };
    Ok((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
}
