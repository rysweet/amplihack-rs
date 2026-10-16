//! File helpers for auto-switch state: bounded no-follow reads, private
//! atomic writes, directory syncs and ownership checks. Every read in the
//! auto-switch modules goes through `read_bounded_nofollow` or
//! `read_bounded`, so no file or pipe can make amplihack allocate without
//! limit.
//!
//! The #1556 helpers in `store.rs` and `pointer.rs` are reused where they fit
//! (`private_dir_builder`, `rename_noreplace`, `check_root_not_symlink`). The rest are not: they return
//! `ProfileError`, have best-effort non-Unix fallbacks, and do not refuse a
//! file owned by another user or sync the parent directory, all of which the
//! credential moves here rely on.

use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

pub fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
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
    write_atomic(dir, name, bytes, 0o600)
}

/// Like `write_private_atomic`, but an existing file keeps its permission
/// bits; a new file is `0600`. For files amplihack edits on Claude's behalf
/// (`.claude.json`, `settings.json`), whose mode is the user's choice. The
/// setuid, setgid and sticky bits are dropped: a JSON file never needs them.
pub fn write_atomic_keep_mode(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    let mode = check_replaceable(&dir.join(name))?.map_or(0o600, |m| m.mode() & 0o777);
    write_atomic(dir, name, bytes, mode)
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

fn write_atomic(dir: &Path, name: &str, bytes: &[u8], mode: u32) -> io::Result<()> {
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
        f.sync_all()?;
        fs::rename(&tmp, &target)?;
        fsync_dir(dir)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Creates `path` as a private directory, or checks an existing one: a real
/// directory owned by the current user with no group or other mode bits.
pub fn ensure_private_dir(path: &Path) -> io::Result<()> {
    match crate::commands::profile::store::private_dir_builder().create(path) {
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

/// True when `path` exists (without following a symlink).
pub fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// A login file amplihack may move: a regular file owned by the current user
/// with exactly one hard link. Only metadata is looked at.
pub fn movable_login(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && m.uid() == euid() && m.nlink() == 1)
}

/// Sets the modification time to now without following a symlink, so Claude
/// notices the file changed.
pub fn touch_nofollow(path: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: valid NUL-terminated path; a null `times` means "now".
    let rc = unsafe {
        libc::utimensat(
            libc::AT_FDCWD,
            c.as_ptr(),
            std::ptr::null(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
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

/// The root may hold auto-switch state only if it is a real directory owned
/// by the current user that nobody else can write to. The symlink check is
/// the profile store's own (#1556).
pub fn check_root_private(root: &Path) -> io::Result<()> {
    crate::commands::profile::store::check_root_not_symlink(root)
        .map_err(|e| invalid(&e.to_string()))?;
    let m = fs::symlink_metadata(root)?;
    if !m.is_dir() {
        return Err(invalid("is not a real directory"));
    }
    if m.uid() != euid() {
        return Err(invalid("is owned by another user"));
    }
    if writable_by_others(&m) {
        return Err(invalid("is writable by other users"));
    }
    Ok(())
}
