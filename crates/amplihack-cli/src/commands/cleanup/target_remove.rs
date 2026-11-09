//! The only code that deletes a pre-commit target (issue #2039, D5, D7, D8).
//!
//! A target is a folder directly under a managed root (`<work root>/targets`
//! or the hook's cache base), or a link there to a folder on another disk.
//! [`inspect`] decides what is at a target path and whether cleanup may act on
//! it; [`remove`] inspects again, requires the same answer, holds exclusive
//! non-blocking locks on every `.cargo-lock` and only then deletes.
//!
//! Everything fails closed: an error, an unreadable folder or an unknown state
//! is a refusal, never a reason to delete. There are no timers here: a held
//! lock is reported and the target is left for a later sweep.

use super::*;
use std::ffi::OsStr;
use std::fmt;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Component;

/// The variable naming extra folders a linked target may point into.
pub(super) const LINK_ROOTS_ENV: &str = "AMPLIHACK_TARGET_LINK_ROOTS";

/// What is at a target path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Shape {
    /// A real folder.
    Dir,
    /// A link to a real folder; `dest` is canonical.
    Link { dest: PathBuf },
    /// A link whose destination does not exist.
    DanglingLink,
}

/// Why a link was not followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LinkReason {
    NotCargoTarget,
    OutsideAllowedRoots,
    LinkChain,
    ForeignOwner,
}

impl LinkReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::NotCargoTarget => "not a cargo target",
            Self::OutsideAllowedRoots => "outside allowed roots",
            Self::LinkChain => "link chain",
            Self::ForeignOwner => "foreign owner",
        }
    }
}

/// Why a target is left alone. One `Display` holds every report text.
#[derive(Debug)]
pub(super) enum Refusal {
    /// A build holds a `.cargo-lock`, or a lock could not be checked.
    InUse,
    /// The shape or the link destination differs from the plan.
    ChangedSincePlan,
    Link(LinkReason),
    /// The path is not directly inside a managed root.
    NotManaged,
    /// A real folder that does not look like a Cargo target.
    NotCargoTarget,
    /// A real folder owned by another user.
    ForeignOwner,
    /// A dangling link where none was expected.
    Dangling,
    /// The path is not a folder or a link.
    NotAFolder,
    /// The path could not be examined.
    Unreadable(String),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InUse => f.write_str("in use: .cargo-lock held"),
            Self::ChangedSincePlan => f.write_str("changed since plan"),
            Self::Link(reason) => write!(f, "link not followed: {}", reason.as_str()),
            Self::NotManaged => f.write_str("outside the folders cleanup manages"),
            Self::NotCargoTarget => f.write_str("not a cargo target"),
            Self::ForeignOwner => f.write_str("owned by another user"),
            Self::Dangling => f.write_str("dangling link; not followed"),
            Self::NotAFolder => f.write_str("not a folder"),
            Self::Unreadable(error) => write!(f, "cannot examine: {error}"),
        }
    }
}

impl std::error::Error for Refusal {}

/// A removal that was refused, or attempted and failed.
#[derive(Debug)]
pub(super) enum RemoveError {
    Refused(Refusal),
    Io(std::io::Error),
}

impl fmt::Display for RemoveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(refusal) => refusal.fmt(f),
            Self::Io(error) => write!(f, "cannot remove: {error}"),
        }
    }
}

impl std::error::Error for RemoveError {}

impl From<Refusal> for RemoveError {
    fn from(refusal: Refusal) -> Self {
        Self::Refused(refusal)
    }
}

/// The folders a linked target may point into: the managed roots plus the
/// accepted entries of `AMPLIHACK_TARGET_LINK_ROOTS`, all canonical. `uid` is
/// the user every ownership check compares with.
#[derive(Debug, Clone)]
pub(super) struct LinkRoots {
    allowed: Vec<PathBuf>,
    uid: u32,
}

impl LinkRoots {
    /// Reads `AMPLIHACK_TARGET_LINK_ROOTS`, `$HOME` and the effective uid.
    pub(super) fn from_env(managed: &ManagedRoots) -> Self {
        let value = std::env::var_os(LINK_ROOTS_ENV);
        let home = crate::user_config::home_dir();
        Self::parse(managed, value.as_deref(), home.as_deref(), euid())
    }

    /// The managed roots (`targets`, then `hook_base` when set), followed by
    /// each `:`-separated entry of `value` that is safe to delete inside. An
    /// entry is ignored, with a warning, when it is empty, relative, cannot be
    /// resolved, is `/`, is `home` or one of its ancestors, is not owned by
    /// `uid`, or is writable by others without the sticky bit. Group write is
    /// allowed. There is no built-in default.
    pub(super) fn parse(
        managed: &ManagedRoots,
        value: Option<&OsStr>,
        home: Option<&Path>,
        uid: u32,
    ) -> Self {
        let mut allowed = vec![managed.targets.clone()];
        if let Some(base) = &managed.hook_base {
            allowed.push(base.clone());
        }
        let home = home.map(canonical_or_self);
        for raw in value
            .map(|v| v.as_bytes().split(|b| *b == b':').collect::<Vec<_>>())
            .unwrap_or_default()
        {
            let entry = Path::new(OsStr::from_bytes(raw));
            match accept_root(entry, home.as_deref(), uid) {
                Ok(root) => {
                    if !allowed.contains(&root) {
                        allowed.push(root);
                    }
                }
                Err(why) => tracing::warn!(
                    entry = %entry.display(),
                    "ignoring {LINK_ROOTS_ENV} entry: {why}"
                ),
            }
        }
        Self { allowed, uid }
    }

    pub(super) fn allowed(&self) -> &[PathBuf] {
        &self.allowed
    }
}

/// The canonical form of one `AMPLIHACK_TARGET_LINK_ROOTS` entry, or why it
/// is ignored.
fn accept_root(entry: &Path, home: Option<&Path>, uid: u32) -> Result<PathBuf, String> {
    if entry.as_os_str().is_empty() {
        return Err("empty".into());
    }
    if !entry.is_absolute() {
        return Err("not an absolute path".into());
    }
    let root = fs::canonicalize(entry).map_err(|e| format!("cannot resolve: {e}"))?;
    if root == Path::new("/") {
        return Err("is /".into());
    }
    if home.is_some_and(|home| home.starts_with(&root)) {
        return Err("is $HOME or one of its parents".into());
    }
    let meta = fs::symlink_metadata(&root).map_err(|e| format!("cannot examine: {e}"))?;
    if !meta.file_type().is_dir() {
        return Err("not a folder".into());
    }
    if meta.uid() != uid {
        return Err("owned by another user".into());
    }
    if meta.mode() & 0o002 != 0 && meta.mode() & 0o1000 == 0 {
        return Err("writable by others without the sticky bit".into());
    }
    Ok(root)
}

/// `path` with `.` and `..` resolved by text alone. For a path with no link
/// in it this is its canonical form, so a difference reveals a link.
fn lexical_normal(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Where the target entry itself sits: its canonical parent joined with its
/// name. The entry is not followed.
pub(super) fn location(path: &Path) -> Result<PathBuf, Refusal> {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(Refusal::NotManaged);
    };
    let parent = fs::canonicalize(parent)
        .map_err(|e| Refusal::Unreadable(format!("{}: {e}", parent.display())))?;
    Ok(parent.join(name))
}

/// The path removal acts on for `shape`: where a folder or dangling link
/// sits, or a link's canonical destination.
pub(super) fn resolved(path: &Path, shape: &Shape) -> Result<PathBuf, Refusal> {
    match shape {
        Shape::Link { dest } => Ok(dest.clone()),
        Shape::Dir | Shape::DanglingLink => location(path),
    }
}

/// Decides what is at `path` and whether cleanup may remove it.
///
/// The entry must sit directly inside a managed root and be owned by the
/// caller's uid. A real folder must look like a Cargo target. A link must
/// point in one step at a real folder that is a direct child of an allowed
/// root, owned by the caller and Cargo-shaped. A dangling link is accepted
/// only with `allow_dangling`.
pub(super) fn inspect(
    path: &Path,
    roots: &ManagedRoots,
    links: &LinkRoots,
    allow_dangling: bool,
) -> Result<Shape, Refusal> {
    inspect_entry(path, roots, links, allow_dangling, true)
}

/// [`inspect`], with the Cargo-shape check of a real folder optional. A
/// legacy (#1808) `owned-paths` line was removed whatever its folder held, so
/// that check is skipped for it; a link always needs a Cargo-shaped
/// destination.
pub(super) fn inspect_entry(
    path: &Path,
    roots: &ManagedRoots,
    links: &LinkRoots,
    allow_dangling: bool,
    cargo_dir_required: bool,
) -> Result<Shape, Refusal> {
    let meta = fs::symlink_metadata(path)
        .map_err(|e| Refusal::Unreadable(format!("{}: {e}", path.display())))?;
    let at = location(path)?;
    if !is_contained(OwnedKind::Target, &at, roots, None) {
        return Err(Refusal::NotManaged);
    }
    let kind = meta.file_type();
    if kind.is_dir() {
        if meta.uid() != links.uid {
            return Err(Refusal::ForeignOwner);
        }
        if cargo_dir_required && !is_cargo_target(&at) {
            return Err(Refusal::NotCargoTarget);
        }
        return Ok(Shape::Dir);
    }
    if !kind.is_symlink() {
        return Err(Refusal::NotAFolder);
    }
    if meta.uid() != links.uid {
        return Err(Refusal::Link(LinkReason::ForeignOwner));
    }
    let pointed =
        fs::read_link(path).map_err(|e| Refusal::Unreadable(format!("{}: {e}", path.display())))?;
    let dest = match fs::canonicalize(path) {
        Ok(dest) => dest,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return if allow_dangling {
                Ok(Shape::DanglingLink)
            } else {
                Err(Refusal::Dangling)
            };
        }
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
            return Err(Refusal::Link(LinkReason::LinkChain));
        }
        Err(error) => {
            return Err(Refusal::Unreadable(format!("{}: {error}", path.display())));
        }
    };
    // One step only: the destination the link names, read as text, must be
    // the canonical destination. Any link on the way makes them differ.
    let named = match at.parent() {
        Some(parent) => lexical_normal(&parent.join(&pointed)),
        None => return Err(Refusal::NotManaged),
    };
    if named != dest {
        return Err(Refusal::Link(LinkReason::LinkChain));
    }
    let dest_meta = fs::symlink_metadata(&dest)
        .map_err(|e| Refusal::Unreadable(format!("{}: {e}", dest.display())))?;
    if !dest_meta.file_type().is_dir() {
        return Err(Refusal::Link(LinkReason::NotCargoTarget));
    }
    let direct_child = dest
        .parent()
        .is_some_and(|parent| links.allowed().iter().any(|root| root == parent));
    if !direct_child {
        return Err(Refusal::Link(LinkReason::OutsideAllowedRoots));
    }
    if dest_meta.uid() != links.uid {
        return Err(Refusal::Link(LinkReason::ForeignOwner));
    }
    if !is_cargo_target(&dest) {
        return Err(Refusal::Link(LinkReason::NotCargoTarget));
    }
    Ok(Shape::Link { dest })
}

/// Takes `LOCK_EX | LOCK_NB` on `lock` when it is a real file. `Ok(None)`
/// when there is no lock file, or it is a link (cargo never makes one, and
/// it is never opened through). Anything that stops the check is `InUse`.
fn lock_one(lock: &Path) -> Result<Option<fs::File>, Refusal> {
    let file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(lock)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => return Ok(None),
        Err(_) => return Err(Refusal::InUse),
    };
    // SAFETY: the fd is valid for the life of `file`; closing it releases
    // the lock.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        return Err(Refusal::InUse);
    }
    Ok(Some(file))
}

/// Exclusive locks on `dir/.cargo-lock` and on the `.cargo-lock` of every
/// real subfolder, such as `debug/` and `release/`. A folder that cannot be
/// listed counts as held: a lock in it would be unseen.
///
/// The caller must keep the returned handles until the delete is done;
/// dropping them earlier lets a build start between the check and the
/// delete.
pub(super) fn lock_cargo_tree(dir: &Path) -> Result<Vec<fs::File>, Refusal> {
    let mut held = Vec::new();
    held.extend(lock_one(&dir.join(".cargo-lock"))?);
    let entries = fs::read_dir(dir).map_err(|_| Refusal::InUse)?;
    for entry in entries {
        let entry = entry.map_err(|_| Refusal::InUse)?;
        // DirEntry::file_type does not follow a link.
        let kind = entry.file_type().map_err(|_| Refusal::InUse)?;
        if kind.is_dir() {
            held.extend(lock_one(&entry.path().join(".cargo-lock"))?);
        }
    }
    Ok(held)
}

/// Removes the target at `path`, which was planned as `expected`.
///
/// Inspects again and requires the same shape and destination, otherwise
/// `changed since plan`. A dangling link loses only the link. Otherwise every
/// `.cargo-lock` is locked exclusively, the folder (the canonical destination
/// for a link) is removed without following links inside it, and then the
/// link itself.
pub(super) fn remove(
    path: &Path,
    expected: &Shape,
    roots: &ManagedRoots,
    links: &LinkRoots,
    allow_dangling: bool,
) -> Result<(), RemoveError> {
    let now = inspect(path, roots, links, true)?;
    if now != *expected {
        return Err(Refusal::ChangedSincePlan.into());
    }
    let dir = match &now {
        Shape::DanglingLink => {
            if !allow_dangling {
                return Err(Refusal::Dangling.into());
            }
            fs::remove_file(path).map_err(RemoveError::Io)?;
            tracing::info!(path = %path.display(), shape = "dangling link", "removed pre-commit target link");
            return Ok(());
        }
        Shape::Dir => location(path)?,
        Shape::Link { dest } => dest.clone(),
    };
    let held = lock_cargo_tree(&dir)?;
    // std::fs::remove_dir_all does not follow links inside the tree
    // (CVE-2022-21658 is fixed in the pinned toolchain).
    let removed = fs::remove_dir_all(&dir).map_err(RemoveError::Io);
    let result = removed.and_then(|()| match &now {
        Shape::Link { .. } => fs::remove_file(path).map_err(RemoveError::Io),
        _ => Ok(()),
    });
    drop(held);
    result?;
    match &now {
        Shape::Link { dest } => tracing::info!(
            path = %path.display(),
            dest = %dest.display(),
            shape = "link",
            "removed pre-commit target and its link"
        ),
        _ => tracing::info!(path = %path.display(), shape = "folder", "removed pre-commit target"),
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests_target_remove.rs"]
mod tests;
