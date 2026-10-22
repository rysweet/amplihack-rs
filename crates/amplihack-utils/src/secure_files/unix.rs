//! Unix-only private-file helpers: bounded no-follow reads, private atomic
//! writes, private directories, ownership checks and the private no-follow
//! open used for lock and log files.
//!
//! Moved unchanged from the CLI's auto-switch `fsx.rs` (issue #1658), which
//! now re-exports them; `owned_by_euid` and `open_private_nofollow` are new.

use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

pub fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

/// Whether `meta` belongs to the effective user of this process.
pub fn owned_by_euid(meta: &fs::Metadata) -> bool {
    meta.uid() == euid()
}

/// Reads at most `limit` bytes from `r`. Returns `limit + 1` bytes when the
/// input is longer, so the caller can tell it was cut.
pub fn read_bounded<R: Read>(r: R, limit: u64) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    r.take(limit + 1).read_to_end(&mut buf)?;
    Ok(buf)
}

/// Reads a regular file without following a final symlink. `Ok(None)` when it
/// does not exist. A symlink, a non-regular file or a file over `limit` bytes
/// is an `InvalidData` error.
pub fn read_bounded_nofollow(
    path: &Path,
    limit: u64,
) -> io::Result<Option<(Vec<u8>, fs::Metadata)>> {
    let mut opts = fs::OpenOptions::new();
    opts.read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    let file = match opts.open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
            return Err(invalid("is a symlink"));
        }
        Err(e) => return Err(e),
    };
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(invalid("is not a regular file"));
    }
    let buf = read_bounded(file, limit)?;
    if buf.len() as u64 > limit {
        return Err(invalid("is too large"));
    }
    Ok(Some((buf, meta)))
}

/// Opens `path` for reading and appending without following a final
/// symlink, creating it with mode `0600` when missing. The opened file must
/// be a regular file owned by the current user with exactly one hard link
/// and no group or other mode bits; anything else is an `InvalidData` error
/// and is left as it is. Never blocks on a FIFO.
pub fn open_private_nofollow(path: &Path) -> io::Result<fs::File> {
    let mut opts = fs::OpenOptions::new();
    opts.read(true)
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    let file = opts.open(path).map_err(|e| {
        if e.raw_os_error() == Some(libc::ELOOP) {
            invalid("is a symlink")
        } else {
            e
        }
    })?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(invalid("is not a regular file"));
    }
    if !owned_by_euid(&meta) {
        return Err(invalid("is owned by another user"));
    }
    if meta.nlink() != 1 {
        return Err(invalid("has more than one hard link"));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(invalid("is not private (owner-only mode 0600)"));
    }
    Ok(file)
}

pub fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

pub fn fsync_dir(dir: &Path) -> io::Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    opts.open(dir)?.sync_all()
}

/// Writes `bytes` to `dir/name` through a new private temp file in `dir`,
/// synced and renamed over the target, then syncs `dir`. The result is
/// always `0600`. Refuses to replace a symlink, a non-regular file or a file
/// owned by someone else.
pub fn write_private_atomic(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    check_replaceable(&dir.join(name))?;
    write_atomic(dir, name, bytes, 0o600, true)
}

/// Like `write_private_atomic`, but syncs neither the file nor `dir`: a
/// crash may lose the write, never half-write it. For small hints written
/// on a latency-sensitive path, such as a prompt hook, where losing one is
/// harmless and two fsyncs per call are not.
pub fn write_private_atomic_nosync(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    check_replaceable(&dir.join(name))?;
    write_atomic(dir, name, bytes, 0o600, false)
}

/// Like `write_private_atomic`, but an existing file keeps its permission
/// bits; a new file is `0600`. For files amplihack edits on Claude's behalf
/// (`.claude.json`, `settings.json`), whose mode is the user's choice. The
/// setuid, setgid and sticky bits are dropped: a JSON file never needs them.
pub fn write_atomic_keep_mode(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    let mode = check_replaceable(&dir.join(name))?.map_or(0o600, |m| m.mode() & 0o777);
    write_atomic(dir, name, bytes, mode, true)
}

/// The target's metadata, or `None` when it does not exist; an error when it
/// may not be replaced.
fn check_replaceable(target: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(target) {
        Ok(m) if m.file_type().is_symlink() => Err(invalid("is a symlink")),
        Ok(m) if !m.is_file() => Err(invalid("is not a regular file")),
        Ok(m) if m.uid() != euid() => Err(invalid("is owned by another user")),
        Ok(m) => Ok(Some(m)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn write_atomic(dir: &Path, name: &str, bytes: &[u8], mode: u32, sync: bool) -> io::Result<()> {
    let target = dir.join(name);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let tmp = dir.join(format!(".{name}.tmp.{}.{nanos}", std::process::id()));
    let mut opts = fs::OpenOptions::new();
    opts.write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let result = (|| -> io::Result<()> {
        let mut f = opts.open(&tmp)?;
        // `mode` on open is filtered by the umask; set it exactly.
        f.set_permissions(fs::Permissions::from_mode(mode))?;
        f.write_all(bytes)?;
        if sync {
            f.sync_all()?;
        }
        fs::rename(&tmp, &target)?;
        if sync { fsync_dir(dir) } else { Ok(()) }
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Creates `path` as a private directory (one level only, mode `0700`), or
/// checks an existing one: a real directory owned by the current user with
/// no group or other mode bits.
pub fn ensure_private_dir(path: &Path) -> io::Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {
            if let Some(parent) = path.parent() {
                fsync_dir(parent)?;
            }
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => check_private_dir(path),
        Err(e) => Err(e),
    }
}

pub fn check_private_dir(path: &Path) -> io::Result<()> {
    let m = fs::symlink_metadata(path)?;
    if m.file_type().is_symlink() || !m.is_dir() {
        return Err(invalid("is not a real directory"));
    }
    if m.uid() != euid() || m.mode() & 0o077 != 0 {
        return Err(invalid("is not private (owner-only mode 0700)"));
    }
    Ok(())
}

/// Whether group or other users can write through `meta`. Group write is
/// accepted only for the user's own private group (named like the user, no
/// listed members), the usual layout with a umask of 002.
pub fn writable_by_others(meta: &fs::Metadata) -> bool {
    let mode = meta.mode();
    if mode & 0o002 != 0 {
        return true;
    }
    mode & 0o020 != 0 && !is_user_private_group(meta.gid())
}

fn is_user_private_group(gid: u32) -> bool {
    // SAFETY: getgid has no preconditions.
    if gid != unsafe { libc::getgid() } {
        return false;
    }
    let (Some(user), Some((group, members))) = (user_name(euid()), group_info(gid)) else {
        return false;
    };
    user == group && members == 0
}

fn user_name(uid: u32) -> Option<String> {
    let mut buf = vec![0 as libc::c_char; 16 * 1024];
    let mut pwd = std::mem::MaybeUninit::<libc::passwd>::zeroed();
    let mut out: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: all pointers are valid for the given sizes; `out` is only read
    // when the call reports success.
    let rc =
        unsafe { libc::getpwuid_r(uid, pwd.as_mut_ptr(), buf.as_mut_ptr(), buf.len(), &mut out) };
    if rc != 0 || out.is_null() {
        return None;
    }
    // SAFETY: on success pw_name points into `buf` and is NUL-terminated.
    let name = unsafe { std::ffi::CStr::from_ptr((*out).pw_name) };
    Some(name.to_string_lossy().into_owned())
}

fn group_info(gid: u32) -> Option<(String, usize)> {
    let mut buf = vec![0 as libc::c_char; 64 * 1024];
    let mut grp = std::mem::MaybeUninit::<libc::group>::zeroed();
    let mut out: *mut libc::group = std::ptr::null_mut();
    // SAFETY: as in `user_name`.
    let rc =
        unsafe { libc::getgrgid_r(gid, grp.as_mut_ptr(), buf.as_mut_ptr(), buf.len(), &mut out) };
    if rc != 0 || out.is_null() {
        return None;
    }
    // SAFETY: on success the strings and the member array point into `buf`;
    // the member array is NULL-terminated.
    unsafe {
        let name = std::ffi::CStr::from_ptr((*out).gr_name)
            .to_string_lossy()
            .into_owned();
        let mut count = 0;
        let mut p = (*out).gr_mem;
        while !p.is_null() && !(*p).is_null() {
            count += 1;
            p = p.add(1);
        }
        Some((name, count))
    }
}
