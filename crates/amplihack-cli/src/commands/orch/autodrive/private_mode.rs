//! The shared rule for private auto-drive state entries (#2546).
//!
//! `access` decides and `make_private` acts. The evidence store
//! (`autodrive::state`) and the completion checker
//! (`autodrive_completion::secure_fs`) both use this rule, and both decide
//! once, from the state root, whether repair is allowed (`repairable_root`).
#[cfg(unix)]
use anyhow::Context;
use anyhow::Result;
use std::fs::File;
use std::path::Path;

/// The name of the state folder the runner creates by default
/// (`<run dir>/autodrive-state` or `<launch home>/autodrive-state`).
pub(crate) const RUNNER_STATE_DIR: &str = "autodrive-state";

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

/// A state root may be repaired only if it is the runner's own folder
/// (`autodrive-state`) or this call has just created it. An operator's
/// explicit `autodrive_state_dir` with any other name is checked strictly, as
/// it was before #2546, and must already be 0700.
pub(crate) fn repairable_root(canonical_root: &Path, created_now: bool) -> bool {
    created_now || canonical_root.file_name() == Some(std::ffi::OsStr::new(RUNNER_STATE_DIR))
}

/// Decides access for `entry`. Reads no files and changes nothing.
///
/// The first match wins: symlink, wrong type, foreign owner, world-writable,
/// group bits on a group other than `egid`, hard links, then extra bits. A
/// file with more than one link is refused for `Kind::SingleLinkFile`, and is
/// never repaired (its other names would change too). Extra bits are repaired
/// only with `Repair::Allowed`; otherwise they are refused.
///
/// Remaining risk: when `egid` is a primary group shared with other users, a
/// member of that group could have written a group-writable entry before it
/// was repaired. Umask 002 cannot produce a world-writable bit, so that bit is
/// always refused, and a foreign or inherited (setgid) group is refused.
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
            "{OUTSIDE_TREE}: {path:?}; make it private ({}) or use the runner's default {RUNNER_STATE_DIR} folder",
            if kind == Kind::Dir {
                "chmod 700"
            } else {
                "chmod 600"
            }
        ),
        Access::Refuse(reason) => anyhow::bail!("{reason}: {path:?}"),
        Access::Repair(mode) => {
            // fchmod on the opened descriptor: the entry checked is the entry changed.
            file.set_permissions(std::fs::Permissions::from_mode(mode))
                .with_context(|| format!("repair private state mode {path:?}"))?;
            eprintln!(
                "INFO: auto-drive state {path:?} had mode {:04o} (gid {}); removed group and other access (now {:04o})",
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
