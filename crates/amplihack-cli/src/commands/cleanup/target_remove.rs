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
use crate::commands::cleanup::targets::is_hook_target_name;
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
    /// A build holds a `.cargo-lock`.
    InUse,
    /// A `.cargo-lock` could not be checked. The text names the reason and
    /// the path: `linked lock: <path>`, `permission denied: <path>` or
    /// `lock check failed: <path>: <error>`. The target is kept, as for
    /// [`Refusal::InUse`].
    LockCheck(String),
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
            Self::InUse => f.write_str("in use: .cargo-lock is held"),
            Self::LockCheck(text) => f.write_str(text),
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
/// the user every ownership check compares with. `home` (canonical) is kept
/// so a folder derived from a host cargo wrapper link (see
/// [`is_wrapper_redirect`]) gets the same safety checks as a listed entry.
#[derive(Debug, Clone)]
pub(super) struct LinkRoots {
    allowed: Vec<PathBuf>,
    home: Option<PathBuf>,
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
    /// allowed.
    ///
    /// The variable is not the only way a second disk is reached: a link
    /// laid out by the host cargo wrapper is followed without it, see
    /// [`is_wrapper_redirect`].
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
        Self { allowed, home, uid }
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
    if !direct_child && !is_wrapper_redirect(&at, &dest, links) {
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

/// Whether the link at `at` is the host cargo wrapper's redirect of a hook
/// target to another disk, so its destination may be followed without
/// `AMPLIHACK_TARGET_LINK_ROOTS` (issue #2039, fix item 1).
///
/// The wrapper replaces a new target `<parent>/<name>` that would sit on the
/// root disk with a link to `<disk>/<basename of parent>-<name>-<key>`, where
/// `<key>` is 12 lowercase hex characters, for example
/// `~/.cache/amplihack/amplihack-precommit-target-0e047858ba2f ->
/// /d0/ryan/cargo-targets/amplihack-amplihack-precommit-target-0e047858ba2f-f5e95e09b016`.
///
/// All of these must hold:
/// - the entry name is a hook target name
///   (`amplihack-precommit-target-` plus 12 hex characters);
/// - the destination name is exactly `<basename of the entry's parent>-<entry
///   name>-<12 lowercase hex>`, so it carries the checkout's own hash;
/// - the destination's folder passes every check an entry of
///   `AMPLIHACK_TARGET_LINK_ROOTS` must pass: not `/`, not `$HOME` or one of
///   its parents, owned by the caller, not writable by others without the
///   sticky bit.
///
/// The caller has already required one link step, a real folder at the
/// destination, and still requires the caller's ownership and a Cargo-shaped
/// destination after this returns.
pub(super) fn is_wrapper_redirect(at: &Path, dest: &Path, links: &LinkRoots) -> bool {
    let Some(entry) = at.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    if !is_hook_target_name(entry) {
        return false;
    }
    let Some(base) = at
        .parent()
        .and_then(Path::file_name)
        .and_then(OsStr::to_str)
    else {
        return false;
    };
    let Some(dest_name) = dest.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    let Some(key) = dest_name
        .strip_prefix(base)
        .and_then(|rest| rest.strip_prefix('-'))
        .and_then(|rest| rest.strip_prefix(entry))
        .and_then(|rest| rest.strip_prefix('-'))
    else {
        return false;
    };
    if key.len() != WRAPPER_KEY_LEN || !key.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return false;
    }
    let Some(folder) = dest.parent() else {
        return false;
    };
    match accept_root(folder, links.home.as_deref(), links.uid) {
        Ok(root) => root == folder,
        Err(why) => {
            tracing::debug!(folder = %folder.display(), "wrapper link folder refused: {why}");
            false
        }
    }
}

/// The lock taken on each `.cargo-lock`: exclusive when removing, shared
/// when only checking at plan time. Both are non-blocking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LockMode {
    Exclusive,
    Shared,
}

impl LockMode {
    fn flags(self) -> libc::c_int {
        match self {
            Self::Exclusive => libc::LOCK_EX | libc::LOCK_NB,
            Self::Shared => libc::LOCK_SH | libc::LOCK_NB,
        }
    }
}

/// Length of the hex key the host cargo wrapper appends to a redirected
/// target's name.
const WRAPPER_KEY_LEN: usize = 12;

/// Why an `io::Error` met while checking `path` keeps the target.
fn lock_check_refusal(path: &Path, error: &std::io::Error) -> Refusal {
    let text = if error.raw_os_error() == Some(libc::ELOOP) {
        // O_NOFOLLOW on a link. Cargo never makes a linked lock, and the
        // link is never opened through.
        format!("linked lock: {}", path.display())
    } else if error.kind() == std::io::ErrorKind::PermissionDenied {
        format!("permission denied: {}", path.display())
    } else {
        format!("lock check failed: {}: {error}", path.display())
    };
    Refusal::LockCheck(text)
}

/// Locks `lock` with `mode` when it is a real file. `Ok(None)` when there is
/// no lock file. A lock held by another process is [`Refusal::InUse`]; any
/// other problem, including a linked lock, is [`Refusal::LockCheck`].
fn lock_one(lock: &Path, mode: LockMode) -> Result<Option<fs::File>, Refusal> {
    let file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(lock)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(lock_check_refusal(lock, &error)),
    };
    // SAFETY: the fd is valid for the life of `file`; closing it releases
    // the lock.
    let rc = unsafe { libc::flock(file.as_raw_fd(), mode.flags()) };
    if rc != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
            return Err(Refusal::InUse);
        }
        return Err(lock_check_refusal(lock, &error));
    }
    Ok(Some(file))
}

/// Locks, with `mode`, `dir/.cargo-lock` and the `.cargo-lock` of every
/// real subfolder, such as `debug/` and `release/`. A folder that cannot be
/// listed is a refusal: a lock in it would be unseen.
///
/// With [`LockMode::Exclusive`] the caller must keep the returned handles
/// until the delete is done; dropping them earlier lets a build start
/// between the check and the delete.
pub(super) fn lock_cargo_tree_with(dir: &Path, mode: LockMode) -> Result<Vec<fs::File>, Refusal> {
    let mut held = Vec::new();
    held.extend(lock_one(&dir.join(".cargo-lock"), mode)?);
    let entries = fs::read_dir(dir).map_err(|e| lock_check_refusal(dir, &e))?;
    for entry in entries {
        let entry = entry.map_err(|e| lock_check_refusal(dir, &e))?;
        // DirEntry::file_type does not follow a link.
        let kind = entry
            .file_type()
            .map_err(|e| lock_check_refusal(&entry.path(), &e))?;
        if kind.is_dir() {
            held.extend(lock_one(&entry.path().join(".cargo-lock"), mode)?);
        }
    }
    Ok(held)
}

/// [`lock_cargo_tree_with`] in [`LockMode::Exclusive`], for removal.
pub(super) fn lock_cargo_tree(dir: &Path) -> Result<Vec<fs::File>, Refusal> {
    lock_cargo_tree_with(dir, LockMode::Exclusive)
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
