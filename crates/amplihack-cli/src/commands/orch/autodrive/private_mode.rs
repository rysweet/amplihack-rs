//! The shared rule for private auto-drive state entries (#2546).
//!
//! `access` decides and `make_private` acts. The evidence store
//! (`autodrive::state`) and the completion checker
//! (`autodrive_completion::secure_fs`) both use this rule, and both decide
//! once, from the state folder's location, whether repair is allowed
//! (`repairable_root`, `in_state_root`).
#[cfg(unix)]
use anyhow::Context;
use anyhow::Result;
use std::ffi::OsString;
use std::fs::File;
use std::path::{Path, PathBuf};

/// The refusal for an entry with group or other bits where repair is not
/// allowed.
pub(crate) const OUTSIDE_TREE: &str =
    "evidence outside the auto-drive state tree has group or other access";

/// What the check does with one opened state entry.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Access {
    /// Owned by this user, the right type, no group or other bits.
    Private,
    /// Owned by this user and the right type, but group or other bits are set
    /// (a writer ran under a umask such as 002). Set the entry to this mode.
    Repair(u32),
    /// Never trusted: symlink, wrong type, foreign owner, world-writable,
    /// foreign group with group bits, hard link, or extra bits where repair
    /// is not allowed.
    Refuse(&'static str),
}

/// The type an entry is expected to have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Dir,
    /// A regular file. A clean file with more than one link is accepted.
    File,
    /// A regular file that must have exactly one link (evidence store).
    SingleLinkFile,
}

/// Whether group and other bits may be removed from an entry this user owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Repair {
    /// The entry is at or below a repairable state root.
    Allowed,
    /// Anywhere else: extra bits are refused, as before #2546.
    Refuse,
}

/// The facts about an entry that decide its access.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Entry {
    /// Always false after `fstat` on an `O_NOFOLLOW` descriptor; kept so that
    /// `access` is complete on its own (D6).
    pub symlink: bool,
    pub is_dir: bool,
    pub is_file: bool,
    pub uid: u32,
    pub gid: u32,
    pub mode: u32,
    pub nlink: u64,
}

/// The auto-drive state root, resolved as autodrive_state.sh resolves it:
/// `${AMPLIHACK_STATE_DIR:-${HOME:-/tmp}/.amplihack/state}/auto-drive`.
/// An empty value counts as unset, as with the shell's `:-`. Pure.
pub(crate) fn state_root_from(state_dir: Option<OsString>, home: Option<OsString>) -> PathBuf {
    let state_dir = state_dir.filter(|v| !v.is_empty());
    let home = home.filter(|v| !v.is_empty());
    let base = match (state_dir, home) {
        (Some(state_dir), _) => PathBuf::from(state_dir),
        (None, Some(home)) => PathBuf::from(home).join(".amplihack/state"),
        (None, None) => PathBuf::from("/tmp/.amplihack/state"),
    };
    base.join("auto-drive")
}

/// The auto-drive state root for this process's environment. Reads
/// AMPLIHACK_STATE_DIR and HOME only; AMPLIHACK_HOME plays no part.
pub(crate) fn autodrive_state_root() -> PathBuf {
    state_root_from(
        std::env::var_os("AMPLIHACK_STATE_DIR"),
        std::env::var_os("HOME"),
    )
}

/// The canonical state root, when it can be trusted; `None` otherwise.
///
/// Fails strict (S2, S7, D3, D4): the `/tmp` fallback (both variables unset
/// or empty), a relative root, a root path that is a symlink, a root that
/// cannot be canonicalised, is not a directory, is owned by another user or
/// has group or other write bits.
#[cfg(unix)]
fn trusted_state_root() -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let state_dir = std::env::var_os("AMPLIHACK_STATE_DIR").filter(|v| !v.is_empty());
    let home = std::env::var_os("HOME").filter(|v| !v.is_empty());
    if state_dir.is_none() && home.is_none() {
        return None;
    }
    let root = state_root_from(state_dir, home);
    if !root.is_absolute() {
        return None;
    }
    let link = std::fs::symlink_metadata(&root).ok()?;
    if link.file_type().is_symlink() {
        return None;
    }
    let canonical = root.canonicalize().ok()?;
    let meta = std::fs::symlink_metadata(&canonical).ok()?;
    // SAFETY: geteuid has no failure mode.
    let euid = unsafe { libc::geteuid() };
    if !meta.is_dir() || meta.uid() != euid || meta.mode() & 0o022 != 0 {
        return None;
    }
    Some(canonical)
}

/// Whether `canonical_dir` is a folder directly under the trusted auto-drive
/// state root (a slug folder). The root itself and deeper folders do not
/// qualify. Paths are compared part by part, never as string prefixes (S3).
/// `canonical_dir` must already be canonical; callers canonicalise it.
#[cfg(unix)]
pub(crate) fn in_state_root(canonical_dir: &Path) -> bool {
    trusted_state_root().is_some_and(|root| canonical_dir.parent() == Some(root.as_path()))
}

#[cfg(not(unix))]
pub(crate) fn in_state_root(_: &Path) -> bool {
    false
}

/// A state folder may be repaired only if this call has just created it, or
/// it lies directly under the auto-drive state root (`in_state_root`). Its
/// name never matters. Any other folder is checked strictly, as before #2546,
/// and must already be 0700.
pub(crate) fn repairable_root(canonical_root: &Path, created_now: bool) -> bool {
    created_now || in_state_root(canonical_root)
}

/// Whether a group's listed members include anyone other than `me`. Names
/// are compared as raw bytes (S15). Pure.
pub(crate) fn shared_group(members: &[&[u8]], me: &[u8]) -> bool {
    members.iter().any(|member| *member != me)
}

/// The start and cap of the buffer for the re-entrant NSS lookups.
#[cfg(unix)]
const NSS_BUF_START: usize = 16 * 1024;
#[cfg(unix)]
const NSS_BUF_CAP: usize = 1024 * 1024;

#[cfg(unix)]
fn nss_buf_start(name: libc::c_int) -> usize {
    // SAFETY: sysconf only reads a configuration value.
    let size = unsafe { libc::sysconf(name) };
    if size > 0 {
        (size as usize).min(NSS_BUF_CAP)
    } else {
        NSS_BUF_START
    }
}

/// The next buffer size after ERANGE, or `Err` at the cap.
#[cfg(unix)]
fn nss_buf_grow(size: usize) -> Result<usize, ()> {
    if size >= NSS_BUF_CAP {
        Err(())
    } else {
        Ok((size * 2).min(NSS_BUF_CAP))
    }
}

/// This user's name, from `getpwuid_r`.
#[cfg(unix)]
fn user_name(euid: u32) -> Result<Vec<u8>, ()> {
    let mut size = nss_buf_start(libc::_SC_GETPW_R_SIZE_MAX);
    loop {
        let mut buf = vec![0 as libc::c_char; size];
        let mut pwd = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: every pointer is valid for the call, and `buf.len()` is the
        // length of `buf`.
        let rc = unsafe {
            libc::getpwuid_r(
                euid,
                pwd.as_mut_ptr(),
                buf.as_mut_ptr(),
                buf.len(),
                &mut result,
            )
        };
        if rc == libc::ERANGE {
            size = nss_buf_grow(size)?;
            continue;
        }
        if rc != 0 || result.is_null() {
            return Err(());
        }
        // SAFETY: on success `result` points to the initialised `pwd`, whose
        // strings live in `buf`, which is still alive here.
        let name = unsafe { (*result).pw_name };
        if name.is_null() {
            return Err(());
        }
        // SAFETY: `name` is a NUL-terminated string inside `buf`.
        return Ok(unsafe { std::ffi::CStr::from_ptr(name) }
            .to_bytes()
            .to_vec());
    }
}

/// The name of group `egid` and whether its listed members (`gr_mem`)
/// include anyone other than this user (M3). `Err(())` when either lookup
/// fails. Users whose primary group is `egid` are not listed in `gr_mem`.
#[cfg(unix)]
fn group_sharing(euid: u32, egid: u32) -> Result<(Vec<u8>, bool), ()> {
    let me = user_name(euid)?;
    let mut size = nss_buf_start(libc::_SC_GETGR_R_SIZE_MAX);
    loop {
        let mut buf = vec![0 as libc::c_char; size];
        let mut grp = std::mem::MaybeUninit::<libc::group>::uninit();
        let mut result: *mut libc::group = std::ptr::null_mut();
        // SAFETY: every pointer is valid for the call, and `buf.len()` is the
        // length of `buf`.
        let rc = unsafe {
            libc::getgrgid_r(
                egid,
                grp.as_mut_ptr(),
                buf.as_mut_ptr(),
                buf.len(),
                &mut result,
            )
        };
        if rc == libc::ERANGE {
            size = nss_buf_grow(size)?;
            continue;
        }
        if rc != 0 || result.is_null() {
            return Err(());
        }
        // SAFETY: on success `result` points to the initialised `grp`, whose
        // strings and member array live in `buf`, which is still alive here.
        let (name, mem) = unsafe { ((*result).gr_name, (*result).gr_mem) };
        if name.is_null() {
            return Err(());
        }
        // SAFETY: `name` is a NUL-terminated string inside `buf`.
        let name = unsafe { std::ffi::CStr::from_ptr(name) }
            .to_bytes()
            .to_vec();
        let mut members = Vec::new();
        if !mem.is_null() {
            let mut i = 0;
            loop {
                // SAFETY: `gr_mem` is a NULL-terminated array inside `buf`, and
                // `i` never passes its NULL entry.
                let member = unsafe { *mem.add(i) };
                if member.is_null() {
                    break;
                }
                // SAFETY: each entry is a NUL-terminated string inside `buf`.
                members.push(
                    unsafe { std::ffi::CStr::from_ptr(member) }
                        .to_bytes()
                        .to_vec(),
                );
                i += 1;
            }
        }
        let members: Vec<&[u8]> = members.iter().map(Vec::as_slice).collect();
        let shared = shared_group(&members, &me);
        return Ok((name, shared));
    }
}

/// Decides access for `entry`. Reads no files and changes nothing.
///
/// The first match wins: symlink, wrong type, foreign owner, world-writable,
/// group bits on a group other than `egid`, hard links, then extra bits. A
/// file with more than one link is refused for `Kind::SingleLinkFile`, and is
/// never repaired (its other names would change too). Extra bits are repaired
/// only with `Repair::Allowed`; otherwise they are refused.
///
/// Remaining risk: when `egid` is a group shared with other users, a member
/// of that group could have written a group-writable entry before it was
/// repaired. `make_private` logs such a repair as a WARNING when the group
/// lists other members, or when its members cannot be read (M3); users whose
/// primary group it is are not listed and are not detected. Umask 002 cannot
/// produce a world-writable bit, so that bit is always refused, and a foreign
/// or inherited (setgid) group is refused.
pub(crate) fn access(entry: Entry, kind: Kind, euid: u32, egid: u32, repair: Repair) -> Access {
    if entry.symlink {
        return Access::Refuse("evidence is a symlink");
    }
    let right_type = match kind {
        Kind::Dir => entry.is_dir,
        Kind::File | Kind::SingleLinkFile => entry.is_file,
    };
    if !right_type {
        return Access::Refuse("evidence is not a regular file or state directory");
    }
    if entry.uid != euid
        || entry.mode & 0o002 != 0
        || (entry.mode & 0o070 != 0 && entry.gid != egid)
    {
        return Access::Refuse("untrusted evidence ownership or permissions");
    }
    let extra = entry.mode & 0o077 != 0;
    if kind != Kind::Dir && entry.nlink != 1 && (kind == Kind::SingleLinkFile || extra) {
        return Access::Refuse("evidence has unexpected hard links");
    }
    match (extra, repair) {
        (false, _) => Access::Private,
        (true, Repair::Allowed) => Access::Repair(entry.mode & 0o700),
        (true, Repair::Refuse) => Access::Refuse(OUTSIDE_TREE),
    }
}

/// Checks an opened state entry and, with `Repair::Allowed`, removes group and
/// other bits from one this user owns, logging the repair on stderr. Refuses
/// everything `access` refuses, changing nothing.
#[cfg(unix)]
pub(crate) fn make_private(file: &File, path: &Path, kind: Kind, repair: Repair) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let m = file.metadata()?;
    let entry = Entry {
        symlink: m.file_type().is_symlink(),
        is_dir: m.is_dir(),
        is_file: m.is_file(),
        uid: m.uid(),
        gid: m.gid(),
        mode: m.mode() & 0o7777,
        nlink: m.nlink(),
    };
    // SAFETY: geteuid and getegid have no failure mode.
    let (euid, egid) = unsafe { (libc::geteuid(), libc::getegid()) };
    match access(entry, kind, euid, egid, repair) {
        Access::Private => Ok(()),
        Access::Refuse(OUTSIDE_TREE) => anyhow::bail!(
            "{OUTSIDE_TREE}: {path:?}; make it private ({}) or keep drive state in a folder directly under {:?}",
            if kind == Kind::Dir {
                "chmod 700"
            } else {
                "chmod 600"
            },
            autodrive_state_root()
        ),
        Access::Refuse(reason) => anyhow::bail!("{reason}: {path:?}"),
        Access::Repair(mode) => {
            // fchmod on the opened descriptor: the entry checked is the entry changed.
            file.set_permissions(std::fs::Permissions::from_mode(mode))
                .with_context(|| format!("repair private state mode {path:?}"))?;
            // M3: a group-bit repair is a WARNING when the group may have
            // other members. A failed lookup only changes the log level.
            let (level, note) = if entry.mode & 0o070 == 0 {
                ("INFO", String::new())
            } else {
                match group_sharing(euid, egid) {
                    Ok((_, false)) => ("INFO", String::new()),
                    Ok((name, true)) => (
                        "WARNING",
                        format!(
                            " group {:?} has other members;",
                            String::from_utf8_lossy(&name)
                        ),
                    ),
                    Err(()) => ("WARNING", " group members could not be read;".to_string()),
                }
            };
            eprintln!(
                "{level}: auto-drive state {path:?} had mode {:04o} (gid {});{note} removed group and other access (now {:04o})",
                entry.mode, entry.gid, mode
            );
            Ok(())
        }
    }
}

#[cfg(not(unix))]
pub(crate) fn make_private(_: &File, _: &Path, _: Kind, _: Repair) -> Result<()> {
    anyhow::bail!("private auto-drive evidence requires Unix file ownership support")
}
