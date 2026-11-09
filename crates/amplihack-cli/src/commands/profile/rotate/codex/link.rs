//! The default Codex home link: inspected with `lstat`, `readlink` and
//! `stat`, replaced with `symlink` and `rename`. No file is opened.

use super::{FaultPoint, account_of, fault_stop};
use std::io;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::{Path, PathBuf};

/// What the link path holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LinkState {
    /// A symlink to an account: its canonical target, label and inode.
    Link {
        target: PathBuf,
        label: String,
        ino: u64,
    },
    /// A real directory or a file.
    NotLink,
    /// The link path cannot be examined (for example, permission denied).
    Unreadable,
    /// A symlink whose target is not a direct child of the root.
    OutsideRoot,
    /// A symlink whose target does not exist.
    Dangling,
    /// Nothing at the link path.
    Missing,
}

/// The state of `link` against the profiles `root`. The target must,
/// made canonical, be a directory that is a direct child of the canonical
/// root (compared by component), named by one component not starting with
/// `.`.
pub(super) fn inspect(link: &Path, root: &Path) -> LinkState {
    match std::fs::symlink_metadata(link) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return LinkState::Missing,
        Err(_) => return LinkState::Unreadable,
        Ok(m) if !m.file_type().is_symlink() => return LinkState::NotLink,
        Ok(_) => {}
    }
    let Ok(abs) = target_of(link) else {
        return LinkState::Unreadable;
    };
    if std::fs::metadata(&abs).is_err() {
        return LinkState::Dangling;
    }
    let Ok(root_c) = std::fs::canonicalize(root) else {
        return LinkState::OutsideRoot;
    };
    match account_of(&abs, &root_c) {
        Some((label, target)) => match std::fs::metadata(&target) {
            Ok(m) => LinkState::Link {
                target,
                label,
                ino: m.ino(),
            },
            Err(_) => LinkState::Dangling,
        },
        None => LinkState::OutsideRoot,
    }
}

/// The symlink's target, made absolute against the link's parent.
fn target_of(link: &Path) -> io::Result<PathBuf> {
    let raw = std::fs::read_link(link)?;
    Ok(if raw.is_absolute() {
        raw
    } else {
        link.parent().unwrap_or(Path::new("/")).join(raw)
    })
}

/// `<link>.amplihack-swap`, beside the link.
pub(super) fn swap_tmp(link: &Path) -> PathBuf {
    let mut s = link.as_os_str().to_owned();
    s.push(".amplihack-swap");
    PathBuf::from(s)
}

/// Points `link` at `new_target` (absolute): a symlink at [`swap_tmp`] is
/// renamed over the link, then the link's parent is synced. A leftover
/// symlink at the temp path is replaced; anything else there is an error.
/// A `rename` error is returned with only the new temp symlink removed;
/// nothing else is deleted.
#[cfg(test)]
pub(super) fn relink(link: &Path, new_target: &Path, fault: Option<FaultPoint>) -> io::Result<()> {
    relink_inner(link, new_target, None, fault)
}

/// What the journal recorded about a swap: the link's target and inode
/// before it, and the new account's inode.
pub(super) struct Expect<'a> {
    pub old_target: &'a Path,
    pub old_ino: u64,
    pub new_ino: u64,
}

/// The error [`relink_checked`] returns when the link or an account no
/// longer matches the journal. Its text starts with `journal-mismatch`.
#[derive(Debug)]
pub(super) struct Mismatch(pub &'static str);

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "journal-mismatch: {}", self.0)
    }
}

impl std::error::Error for Mismatch {}

/// The `detail=` word of a [`Mismatch`] error, or `None` for any other
/// error.
pub(super) fn mismatch_detail(e: &io::Error) -> Option<&'static str> {
    e.get_ref()
        .and_then(|inner| inner.downcast_ref::<Mismatch>())
        .map(|m| m.0)
}

/// As [`relink`], but just before the rename the link must still be a
/// symlink to `expect.old_target` with inode `expect.old_ino`, and
/// `new_target` must still have inode `expect.new_ino`. Otherwise nothing
/// is renamed, the temp symlink is removed and a [`Mismatch`] error is
/// returned.
pub(super) fn relink_checked(
    link: &Path,
    expect: &Expect<'_>,
    new_target: &Path,
    fault: Option<FaultPoint>,
) -> io::Result<()> {
    relink_inner(link, new_target, Some(expect), fault)
}

/// `None` when the link and both accounts still match `expect`, or the
/// reason they do not.
fn recheck(link: &Path, expect: &Expect<'_>, new_target: &Path) -> Option<&'static str> {
    // `read_link` fails on anything that is not a symlink.
    match target_of(link).and_then(std::fs::canonicalize) {
        Ok(t) if t == expect.old_target => {}
        _ => return Some("link-changed"),
    }
    match std::fs::metadata(expect.old_target) {
        Ok(m) if m.ino() == expect.old_ino => {}
        _ => return Some("old-account-replaced"),
    }
    match std::fs::metadata(new_target) {
        Ok(m) if m.ino() == expect.new_ino => None,
        _ => Some("new-account-replaced"),
    }
}

fn relink_inner(
    link: &Path,
    new_target: &Path,
    expect: Option<&Expect<'_>>,
    fault: Option<FaultPoint>,
) -> io::Result<()> {
    if !new_target.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the new link target is not absolute",
        ));
    }
    let tmp = swap_tmp(link);
    match std::fs::symlink_metadata(&tmp) {
        Ok(m) if m.file_type().is_symlink() => std::fs::remove_file(&tmp)?,
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} exists and is not a symlink", tmp.display()),
            ));
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    symlink(new_target, &tmp)?;
    if let Some(reason) = expect.and_then(|x| recheck(link, x, new_target)) {
        std::fs::remove_file(&tmp)?;
        return Err(io::Error::other(Mismatch(reason)));
    }
    if let Err(e) = std::fs::rename(&tmp, link) {
        // Only the symlink just made is removed.
        if let Err(rm) = std::fs::remove_file(&tmp) {
            eprintln!("warning: could not remove {}: {rm}", tmp.display());
        }
        return Err(e);
    }
    fault_stop(fault, FaultPoint::AfterRename);
    let parent = link.parent().unwrap_or(Path::new("/"));
    crate::commands::profile::auto_switch::kit::fsync_dir(parent)
}
