//! Unit tests for `target_remove.rs` (issue #2039, D5, D7, D8, SR1, SR2, SR10).
//!
//! `target_remove.rs` is a submodule of `sweep.rs`, loaded like `execute.rs`:
//! `#[path = "target_remove.rs"] mod target_remove;` in `sweep.rs`, and at the
//! end of `target_remove.rs`:
//! `#[cfg(test)] #[path = "tests_target_remove.rs"] mod tests;`
//!
//! Contract:
//! - All types below derive `Debug`.
//! - `pub(super) enum Shape { Dir, Link { dest: PathBuf }, DanglingLink }`
//!   (`Debug, Clone, PartialEq, Eq`); `dest` is canonical.
//! - `pub(super) struct LinkRoots` with
//!   - `fn from_env(managed: &ManagedRoots) -> Self`, which reads
//!     `AMPLIHACK_TARGET_LINK_ROOTS`, `$HOME` and `geteuid()` and calls
//!   - `fn parse(managed: &ManagedRoots, value: Option<&OsStr>,
//!     home: Option<&Path>, uid: u32) -> Self`. Entries are `:`-separated and
//!     canonicalized. An entry is ignored (with a `tracing::warn`) when it is
//!     empty, relative, unresolvable, `/`, `$HOME` or an ancestor of `$HOME`,
//!     not owned by `uid`, or writable by others without the sticky bit.
//!     Group write is allowed. There is no built-in default.
//!   - `fn allowed(&self) -> &[PathBuf]`: the canonical managed roots
//!     (`targets`, then `hook_base` when set) followed by the accepted entries.
//!   - Every ownership check in `inspect` and `remove` compares with `uid`.
//! - `pub(super) enum LinkReason` and `pub(super) enum Refusal` whose single
//!   `Display` gives every report text:
//!   `in use: .cargo-lock is held`, `changed since plan`, and
//!   `link not followed: <not a cargo target|outside allowed roots|link chain|foreign owner>`.
//! - `pub(super) enum RemoveError { Refused(Refusal), Io(std::io::Error) }`.
//! - `pub(super) fn inspect(path: &Path, roots: &ManagedRoots, links: &LinkRoots,
//!   allow_dangling: bool) -> Result<Shape, Refusal>`.
//! - `pub(super) fn remove(path: &Path, expected: &Shape, roots: &ManagedRoots,
//!   links: &LinkRoots, allow_dangling: bool) -> Result<(), RemoveError>`:
//!   inspects again, requires the same shape and destination, holds
//!   `LOCK_EX|LOCK_NB` on every `.cargo-lock` (root and one level down)
//!   through the delete, then removes the folder and, for a link, the link.
//!
//! Liveness is shown by holding a real `flock` in this process (a second
//! open file description conflicts), never by sleeping.

use super::super::*;
// Named explicitly: `sweep` also has a `Refusal` (from `execute.rs`).
use super::{LinkRoots, Refusal, RemoveError, Shape, inspect, is_wrapper_redirect, remove};
use std::ffi::OsString;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{PermissionsExt, symlink};

const NAME: &str = "amplihack-precommit-target-0123456789ab";
const OTHER: &str = "amplihack-precommit-target-ba9876543210";
const IN_USE: &str = "in use: .cargo-lock is held";
const CHANGED: &str = "changed since plan";

struct Fx {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    targets: PathBuf,
    hook_base: PathBuf,
    /// A "second disk" folder, given to `LinkRoots` as an extra root.
    disk: PathBuf,
    settings: Settings,
}

impl Fx {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap();
        let work = WorkRoot {
            path: root.join("work"),
            explicit: false,
        };
        let hook_base = root.join("cache/amplihack");
        let home = root.join("home");
        let disk = root.join("disk/cargo-targets");
        for dir in [
            work.runs_dir(),
            work.targets_dir(),
            hook_base.clone(),
            home.clone(),
            disk.clone(),
        ] {
            fs::create_dir_all(&dir).unwrap();
        }
        fs::set_permissions(&disk, fs::Permissions::from_mode(0o755)).unwrap();
        let settings = Settings {
            work_root: work.clone(),
            keep: Duration::ZERO,
            log_age: Duration::MAX,
            archive_dir: work.path.join("archive"),
            categories: [Category::RunTarget].into_iter().collect(),
            apply: true,
            report_unrecorded: false,
            deadline: None,
            recipe_log_dir: None,
            codex_sessions_dir: None,
            claude_projects_dir: None,
            hook_cache_base: Some(hook_base.clone()),
        };
        Self {
            _tmp: tmp,
            targets: work.targets_dir(),
            root,
            home,
            hook_base,
            disk,
            settings,
        }
    }

    fn roots(&self) -> ManagedRoots {
        ManagedRoots::new(&self.settings)
    }

    fn links_with(&self, value: &[&Path], uid: u32) -> LinkRoots {
        let joined = std::env::join_paths(value).unwrap();
        LinkRoots::parse(
            &self.roots(),
            Some(joined.as_os_str()),
            Some(self.home.as_path()),
            uid,
        )
    }

    /// The second disk is allowed.
    fn links(&self) -> LinkRoots {
        self.links_with(&[self.disk.as_path()], euid())
    }

    /// No extra roots.
    fn no_links(&self) -> LinkRoots {
        LinkRoots::parse(&self.roots(), None, Some(self.home.as_path()), euid())
    }

    fn inspect(
        &self,
        path: &Path,
        links: &LinkRoots,
        allow_dangling: bool,
    ) -> Result<Shape, Refusal> {
        inspect(path, &self.roots(), links, allow_dangling)
    }

    fn remove(
        &self,
        path: &Path,
        expected: &Shape,
        links: &LinkRoots,
        allow_dangling: bool,
    ) -> Result<(), RemoveError> {
        remove(path, expected, &self.roots(), links, allow_dangling)
    }

    /// `<parent>/<name>` linked the way the host cargo wrapper links it: to
    /// `<folder>/<basename of parent>-<name>-<12 hex>`, a cargo-shaped folder.
    fn wrapper_linked(&self, parent: &Path, folder: &Path, name: &str) -> (PathBuf, PathBuf) {
        let base = parent.file_name().unwrap().to_str().unwrap();
        let dest = cargo_dir(&folder.join(format!("{base}-{name}-f5e95e09b016")));
        let link = parent.join(name);
        symlink(&dest, &link).unwrap();
        (link, dest)
    }

    /// `targets/<name>` linked to `disk/<name>`, a cargo-shaped folder.
    fn linked(&self, name: &str) -> (PathBuf, PathBuf) {
        let dest = cargo_dir(&self.disk.join(name));
        let link = self.targets.join(name);
        symlink(&dest, &link).unwrap();
        (link, dest)
    }
}

/// A cargo-shaped folder with a profile, a lock file and some output.
fn cargo_dir(path: &Path) -> PathBuf {
    fs::create_dir_all(path.join("debug/.fingerprint")).unwrap();
    fs::write(path.join(".rustc_info.json"), "{}").unwrap();
    fs::write(path.join(".cargo-lock"), "").unwrap();
    fs::write(path.join("debug/.cargo-lock"), "").unwrap();
    fs::write(path.join("debug/blob"), vec![7u8; 128]).unwrap();
    path.to_path_buf()
}

fn exists_no_follow(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// Holds an exclusive `flock`; use `release` before asserting unlocked state.
fn hold(file: &Path) -> fs::File {
    let handle = fs::OpenOptions::new().read(true).open(file).unwrap();
    // SAFETY: the fd is valid for the life of `handle`.
    let rc = unsafe { libc::flock(handle.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(rc, 0, "{}", std::io::Error::last_os_error());
    handle
}

fn release(handle: fs::File) {
    // A concurrent fork may still hold a duplicate of this open description.
    // Unlock explicitly rather than waiting for every inherited fd to close.
    // SAFETY: `handle` owns this valid descriptor.
    let rc = unsafe { libc::flock(handle.as_raw_fd(), libc::LOCK_UN) };
    assert_eq!(rc, 0, "{}", std::io::Error::last_os_error());
}

fn refusal_text(result: Result<(), RemoveError>) -> String {
    match result {
        Err(RemoveError::Refused(r)) => r.to_string(),
        Err(RemoveError::Io(e)) => panic!("expected a refusal, got io error {e}"),
        Ok(()) => panic!("expected a refusal, got Ok"),
    }
}

fn link_reason(result: Result<Shape, Refusal>) -> String {
    let text = result.expect_err("expected a refusal").to_string();
    assert!(
        text.starts_with("link not followed: "),
        "every link refusal starts with `link not followed: `: {text}"
    );
    text
}

// ----- shapes ------------------------------------------------------------------

#[test]
fn real_folder_under_each_managed_root_is_a_dir() {
    let fx = Fx::new();
    for dir in [fx.targets.join(NAME), fx.hook_base.join(NAME)] {
        cargo_dir(&dir);
        assert_eq!(fx.inspect(&dir, &fx.no_links(), false).unwrap(), Shape::Dir);
    }
}

#[test]
fn link_to_a_cargo_folder_on_an_allowed_root_is_a_link() {
    let fx = Fx::new();
    let (link, dest) = fx.linked(NAME);
    assert_eq!(
        fx.inspect(&link, &fx.links(), false).unwrap(),
        Shape::Link { dest: dest.clone() }
    );
}

#[test]
fn dangling_link_needs_allow_dangling() {
    let fx = Fx::new();
    let link = fx.targets.join(NAME);
    symlink(fx.disk.join(NAME), &link).unwrap();
    assert!(fx.inspect(&link, &fx.links(), false).is_err());
    assert_eq!(
        fx.inspect(&link, &fx.links(), true).unwrap(),
        Shape::DanglingLink
    );
}

// ----- refusals ------------------------------------------------------------------

#[test]
fn folders_outside_the_managed_roots_are_refused() {
    let fx = Fx::new();
    let outside = cargo_dir(&fx.root.join("elsewhere").join(NAME));
    assert!(fx.inspect(&outside, &fx.links(), false).is_err());
    // Not a direct child of targets/.
    let nested = cargo_dir(&fx.targets.join("sub").join(NAME));
    assert!(fx.inspect(&nested, &fx.links(), false).is_err());
    // Under the hook base without the prefix.
    let unprefixed = cargo_dir(&fx.hook_base.join("some-cache"));
    assert!(fx.inspect(&unprefixed, &fx.links(), false).is_err());
    // A root itself.
    assert!(fx.inspect(&fx.targets, &fx.links(), false).is_err());
    // A link that sits outside the managed roots, even to an allowed root.
    let dest = cargo_dir(&fx.disk.join(NAME));
    let stray_link = fx.root.join(NAME);
    symlink(&dest, &stray_link).unwrap();
    assert!(fx.inspect(&stray_link, &fx.links(), false).is_err());
}

#[test]
fn real_folder_that_is_not_cargo_shaped_is_refused() {
    let fx = Fx::new();
    let dir = fx.targets.join(NAME);
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/lib.rs"), "").unwrap();
    assert!(fx.inspect(&dir, &fx.links(), false).is_err());
}

#[test]
fn link_outside_allowed_roots_is_refused() {
    let fx = Fx::new();
    let (link, _) = fx.linked(NAME);
    assert_eq!(
        link_reason(fx.inspect(&link, &fx.no_links(), false)),
        "link not followed: outside allowed roots",
        "without AMPLIHACK_TARGET_LINK_ROOTS a second disk is not allowed"
    );
}

#[test]
fn link_to_a_folder_that_is_not_a_direct_child_is_refused() {
    let fx = Fx::new();
    let dest = cargo_dir(&fx.disk.join("nested").join(NAME));
    let link = fx.targets.join(NAME);
    symlink(&dest, &link).unwrap();
    assert_eq!(
        link_reason(fx.inspect(&link, &fx.links(), false)),
        "link not followed: outside allowed roots"
    );
    // The allowed root itself is not a destination.
    let at_root = fx.targets.join(OTHER);
    symlink(&fx.disk, &at_root).unwrap();
    assert!(fx.inspect(&at_root, &fx.links(), false).is_err());
}

#[test]
fn link_chains_are_refused() {
    let fx = Fx::new();
    // link -> disk/hop (a link) -> disk/real
    let real = cargo_dir(&fx.disk.join(OTHER));
    let hop = fx.disk.join(NAME);
    symlink(&real, &hop).unwrap();
    let link = fx.targets.join(NAME);
    symlink(&hop, &link).unwrap();
    assert_eq!(
        link_reason(fx.inspect(&link, &fx.links(), false)),
        "link not followed: link chain"
    );

    // link -> alias/<name>, where alias is a link to the allowed root.
    let alias = fx.root.join("alias");
    symlink(&fx.disk, &alias).unwrap();
    let via_alias = fx.targets.join(OTHER);
    symlink(alias.join(OTHER), &via_alias).unwrap();
    assert_eq!(
        link_reason(fx.inspect(&via_alias, &fx.links(), false)),
        "link not followed: link chain"
    );
}

#[test]
fn link_to_a_folder_that_is_not_cargo_shaped_is_refused() {
    let fx = Fx::new();
    let dest = fx.disk.join(NAME);
    fs::create_dir_all(dest.join("photos")).unwrap();
    fs::write(dest.join("photos/precious.jpg"), "x").unwrap();
    let link = fx.targets.join(NAME);
    symlink(&dest, &link).unwrap();
    assert_eq!(
        link_reason(fx.inspect(&link, &fx.links(), false)),
        "link not followed: not a cargo target"
    );
    assert!(
        fx.remove(
            &link,
            &Shape::Link { dest: dest.clone() },
            &fx.links(),
            false
        )
        .is_err()
    );
    assert!(dest.join("photos/precious.jpg").exists());
}

#[test]
fn foreign_owner_is_refused_for_links_and_folders() {
    let fx = Fx::new();
    let foreign = euid().wrapping_add(1);
    // Managed roots stay allowed whoever the caller claims to be; the link's
    // own owner is what fails.
    let dest = cargo_dir(&fx.hook_base.join(OTHER));
    let link = fx.targets.join(NAME);
    symlink(&dest, &link).unwrap();
    let links = fx.links_with(&[], foreign);
    assert_eq!(
        link_reason(fx.inspect(&link, &links, false)),
        "link not followed: foreign owner"
    );
    let dir = cargo_dir(&fx.targets.join(OTHER));
    assert!(fx.inspect(&dir, &links, false).is_err());
    assert!(fx.remove(&dir, &Shape::Dir, &links, false).is_err());
    assert!(dir.join("debug/blob").exists());
}

// ----- LinkRoots (D5, SR2) ----------------------------------------------------------

#[test]
fn link_roots_reject_unsafe_entries_and_accept_group_writable() {
    let fx = Fx::new();
    let make = |name: &str, mode: u32| {
        let p = fx.root.join(name);
        fs::create_dir_all(&p).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(mode)).unwrap();
        p
    };
    let group = make("group-0775", 0o775);
    let sticky = make("sticky-1777", 0o1777);
    let world = make("world-0777", 0o777);
    let alias = fx.root.join("alias-to-group");
    symlink(&group, &alias).unwrap();

    let missing = fx.root.join("missing");
    let entries: Vec<PathBuf> = vec![
        PathBuf::new(),
        PathBuf::from("relative/dir"),
        PathBuf::from("/"),
        fx.home.clone(),
        fx.root.clone(), // an ancestor of $HOME
        missing.clone(),
        world.clone(),
        sticky.clone(),
        group.clone(),
        alias.clone(),
    ];
    let mut value = OsString::new();
    for (i, entry) in entries.iter().enumerate() {
        if i > 0 {
            value.push(":");
        }
        value.push(entry.as_os_str());
    }
    let links = LinkRoots::parse(
        &fx.roots(),
        Some(value.as_os_str()),
        Some(fx.home.as_path()),
        euid(),
    );
    let allowed = links.allowed().to_vec();

    assert!(
        allowed.contains(&group),
        "group write is allowed: {allowed:?}"
    );
    assert!(
        allowed.contains(&sticky),
        "sticky world-writable is allowed: {allowed:?}"
    );
    for rejected in [
        PathBuf::from("/"),
        fx.home.clone(),
        fx.root.clone(),
        world,
        missing,
    ] {
        assert!(!allowed.contains(&rejected), "{rejected:?} in {allowed:?}");
    }
    assert!(allowed.iter().all(|a| a.is_absolute()), "{allowed:?}");
    assert!(
        !allowed.contains(&alias),
        "entries are canonical: {allowed:?}"
    );
    assert!(allowed.contains(&fs::canonicalize(&fx.targets).unwrap()));
    assert!(allowed.contains(&fs::canonicalize(&fx.hook_base).unwrap()));
}

#[test]
fn link_roots_without_the_variable_are_the_managed_roots_and_reject_foreign_owned_entries() {
    let fx = Fx::new();
    let none = LinkRoots::parse(&fx.roots(), None, Some(fx.home.as_path()), euid());
    assert_eq!(
        none.allowed().len(),
        2,
        "managed roots only: {:?}",
        none.allowed()
    );
    // Exactly the managed roots, whatever disk the test folder is on (a
    // check for "/d0" failed when TMPDIR was on /d0).
    let managed = [
        fs::canonicalize(&fx.targets).unwrap(),
        fs::canonicalize(&fx.hook_base).unwrap(),
    ];
    assert!(
        none.allowed().iter().all(|a| managed.contains(a)),
        "managed roots only: {:?}",
        none.allowed()
    );

    let foreign = fx.links_with(&[fx.disk.as_path()], euid().wrapping_add(1));
    assert!(
        !foreign.allowed().contains(&fx.disk),
        "a root owned by another uid is ignored"
    );
    assert!(fx.links().allowed().contains(&fx.disk));
}

// ----- removal ---------------------------------------------------------------

#[test]
fn removes_a_real_folder() {
    let fx = Fx::new();
    let dir = cargo_dir(&fx.targets.join(NAME));
    let shape = fx.inspect(&dir, &fx.no_links(), false).unwrap();
    fx.remove(&dir, &shape, &fx.no_links(), false).unwrap();
    assert!(!exists_no_follow(&dir));
}

#[test]
fn removes_a_linked_target_destination_then_link() {
    let fx = Fx::new();
    let (link, dest) = fx.linked(NAME);
    let shape = fx.inspect(&link, &fx.links(), false).unwrap();
    fx.remove(&link, &shape, &fx.links(), false).unwrap();
    assert!(!exists_no_follow(&dest), "destination removed");
    assert!(!exists_no_follow(&link), "link removed");
    assert!(fx.disk.is_dir(), "the allowed root stays");
}

#[test]
fn removes_only_the_link_when_dangling() {
    let fx = Fx::new();
    let link = fx.targets.join(NAME);
    let sibling = cargo_dir(&fx.disk.join(OTHER));
    symlink(fx.disk.join(NAME), &link).unwrap();
    fx.remove(&link, &Shape::DanglingLink, &fx.links(), true)
        .unwrap();
    assert!(!exists_no_follow(&link));
    assert!(sibling.join("debug/blob").exists());
}

#[test]
fn held_cargo_lock_at_any_level_means_in_use_and_nothing_is_deleted() {
    let fx = Fx::new();
    for lock in [".cargo-lock", "debug/.cargo-lock"] {
        let dir = cargo_dir(&fx.targets.join(NAME));
        let held = hold(&dir.join(lock));
        assert_eq!(
            refusal_text(fx.remove(&dir, &Shape::Dir, &fx.no_links(), false)),
            IN_USE,
            "{lock}"
        );
        assert!(dir.join("debug/blob").exists(), "{lock}: nothing deleted");
        let _inherited = held.try_clone().unwrap();
        release(held);
        fx.remove(&dir, &Shape::Dir, &fx.no_links(), false).unwrap();
        assert!(!exists_no_follow(&dir), "{lock}: removed once released");
    }
}

#[test]
fn held_cargo_lock_in_a_linked_destination_keeps_link_and_destination() {
    let fx = Fx::new();
    let (link, dest) = fx.linked(NAME);
    let shape = Shape::Link { dest: dest.clone() };
    let held = hold(&dest.join("debug/.cargo-lock"));
    assert_eq!(
        refusal_text(fx.remove(&link, &shape, &fx.links(), false)),
        IN_USE
    );
    assert!(dest.join("debug/blob").exists());
    assert!(exists_no_follow(&link));
    release(held);
    fx.remove(&link, &shape, &fx.links(), false).unwrap();
    assert!(!exists_no_follow(&dest) && !exists_no_follow(&link));
}

#[test]
fn a_profile_folder_that_cannot_be_read_is_refused() {
    if euid() == 0 {
        return;
    }
    let fx = Fx::new();
    let dir = cargo_dir(&fx.targets.join(NAME));
    fs::create_dir(dir.join("release")).unwrap();
    fs::set_permissions(dir.join("release"), fs::Permissions::from_mode(0o000)).unwrap();
    let result = fx.remove(&dir, &Shape::Dir, &fx.no_links(), false);
    fs::set_permissions(dir.join("release"), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        matches!(result, Err(RemoveError::Refused(_))),
        "fail closed: {result:?}"
    );
    assert!(dir.join("debug/blob").exists());
}

#[test]
fn a_linked_cargo_lock_is_never_opened_through_or_deleted() {
    let fx = Fx::new();
    let outside = fx.root.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("lockfile"), "precious").unwrap();
    let dir = cargo_dir(&fx.targets.join(NAME));
    fs::remove_file(dir.join("debug/.cargo-lock")).unwrap();
    symlink(outside.join("lockfile"), dir.join("debug/.cargo-lock")).unwrap();
    // Hold the outside file: a lock taken through the link would conflict.
    let _held = hold(&outside.join("lockfile"));

    // Cargo never makes a linked lock, so the target is kept, with its own
    // reason (review of PR #2152).
    let result = fx.remove(&dir, &Shape::Dir, &fx.no_links(), false);
    let text = refusal_text(result);
    assert!(
        text.starts_with("linked lock: ") && text.ends_with("debug/.cargo-lock"),
        "{text}"
    );
    assert!(dir.join("debug/blob").exists(), "the target must be kept");
    assert_eq!(
        fs::read_to_string(outside.join("lockfile")).unwrap(),
        "precious"
    );
}

/// A lock that cannot be opened keeps the target with `permission denied`,
/// not `in use` (review of PR #2152).
#[test]
fn an_unreadable_cargo_lock_keeps_the_target_with_its_own_reason() {
    if euid() == 0 {
        return;
    }
    let fx = Fx::new();
    let dir = cargo_dir(&fx.targets.join(NAME));
    let lock = dir.join("debug/.cargo-lock");
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o000)).unwrap();
    let text = refusal_text(fx.remove(&dir, &Shape::Dir, &fx.no_links(), false));
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        text.starts_with("permission denied: ") && text.ends_with("debug/.cargo-lock"),
        "{text}"
    );
    assert!(dir.join("debug/blob").exists(), "the target must be kept");
}

#[test]
fn links_inside_the_destination_are_not_followed() {
    let fx = Fx::new();
    let canary_dir = fx.root.join("canary");
    fs::create_dir(&canary_dir).unwrap();
    fs::write(canary_dir.join("canary"), "alive").unwrap();

    let dir = cargo_dir(&fx.targets.join(NAME));
    symlink(&canary_dir, dir.join("debug/escape")).unwrap();
    fx.remove(&dir, &Shape::Dir, &fx.no_links(), false).unwrap();

    let (link, dest) = fx.linked(OTHER);
    symlink(&canary_dir, dest.join("escape")).unwrap();
    fx.remove(&link, &Shape::Link { dest }, &fx.links(), false)
        .unwrap();

    assert_eq!(
        fs::read_to_string(canary_dir.join("canary")).unwrap(),
        "alive"
    );
}

#[test]
fn shape_change_since_plan_is_refused() {
    let fx = Fx::new();
    // Dir at plan time, a link at removal.
    let dir = cargo_dir(&fx.targets.join(NAME));
    let moved = cargo_dir(&fx.disk.join(NAME));
    fs::remove_dir_all(&dir).unwrap();
    symlink(&moved, &dir).unwrap();
    assert_eq!(
        refusal_text(fx.remove(&dir, &Shape::Dir, &fx.links(), false)),
        CHANGED
    );
    assert!(moved.join("debug/blob").exists());

    // A link redirected to another valid destination.
    let first = cargo_dir(&fx.disk.join(OTHER));
    let second = cargo_dir(&fx.disk.join("amplihack-precommit-target-000000000002"));
    let link = fx.targets.join(OTHER);
    symlink(&second, &link).unwrap();
    assert_eq!(
        refusal_text(fx.remove(
            &link,
            &Shape::Link {
                dest: first.clone()
            },
            &fx.links(),
            false
        )),
        CHANGED
    );
    assert!(first.join("debug/blob").exists() && second.join("debug/blob").exists());

    // A link that resolved at plan time and is dangling now.
    let gone = fx.disk.join("amplihack-precommit-target-000000000003");
    let dangling = fx.targets.join("amplihack-precommit-target-000000000003");
    symlink(&gone, &dangling).unwrap();
    assert_eq!(
        refusal_text(fx.remove(&dangling, &Shape::Link { dest: gone }, &fx.links(), false)),
        CHANGED
    );
    assert!(exists_no_follow(&dangling));
}

#[test]
fn refusal_display_texts_are_the_documented_ones() {
    let fx = Fx::new();
    let dir = cargo_dir(&fx.targets.join(NAME));
    let _held = hold(&dir.join(".cargo-lock"));
    let text = refusal_text(fx.remove(&dir, &Shape::Dir, &fx.no_links(), false));
    assert_eq!(text, "in use: .cargo-lock is held");
    assert!(!text.contains('\n'));
}

// ----- no timers (SR10, AC9) -------------------------------------------------

#[test]
fn target_remove_has_no_timers_retries_or_blocking_locks() {
    let source = include_str!("target_remove.rs");
    for (n, line) in source.lines().enumerate() {
        let code = line.trim_start();
        if code.starts_with("//") {
            continue;
        }
        let lower = code.to_ascii_lowercase();
        for banned in ["sleep", "duration", "retry", "retries", "instant::"] {
            assert!(
                !lower.contains(banned),
                "target_remove.rs:{}: `{banned}` is not allowed: {line}",
                n + 1
            );
        }
        if code.contains("LOCK_EX") {
            assert!(
                code.contains("LOCK_NB"),
                "target_remove.rs:{}: every exclusive lock is non-blocking: {line}",
                n + 1
            );
        }
    }
}

// ----- host cargo wrapper links (issue #2039, fix item 1) -------------------------

/// The layout candide's cargo wrapper makes is followed with no
/// `AMPLIHACK_TARGET_LINK_ROOTS`, from either managed root, and removal takes
/// the destination and the link.
#[test]
fn wrapper_link_is_followed_and_removed_without_link_roots() {
    let fx = Fx::new();
    for parent in [fx.targets.clone(), fx.hook_base.clone()] {
        let (link, dest) = fx.wrapper_linked(&parent, &fx.disk, NAME);
        let shape = fx.inspect(&link, &fx.no_links(), false).unwrap();
        assert_eq!(shape, Shape::Link { dest: dest.clone() });
        fx.remove(&link, &shape, &fx.no_links(), false).unwrap();
        assert!(!exists_no_follow(&dest), "destination removed");
        assert!(!exists_no_follow(&link), "link removed");
        assert!(fx.disk.is_dir(), "the folder holding it stays");
    }
}

/// A held `.cargo-lock` still stops removal of a wrapper link.
#[test]
fn wrapper_link_in_use_is_kept() {
    let fx = Fx::new();
    let (link, dest) = fx.wrapper_linked(&fx.targets, &fx.disk, NAME);
    let shape = fx.inspect(&link, &fx.no_links(), false).unwrap();
    let _held = hold(&dest.join("debug/.cargo-lock"));
    assert_eq!(
        refusal_text(fx.remove(&link, &shape, &fx.no_links(), false)),
        IN_USE
    );
    assert!(dest.join("debug/blob").exists() && exists_no_follow(&link));
}

/// Only the wrapper's exact name is derived: the entry's own name between the
/// parent's basename and a 12-hex key. Anything else needs the variable.
#[test]
fn names_that_are_not_the_wrapper_layout_are_not_followed() {
    let fx = Fx::new();
    let base = fx
        .targets
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let wrong = [
        // Another checkout's hash.
        format!("{base}-{OTHER}-f5e95e09b016"),
        // Same name as the entry, no key.
        NAME.to_string(),
        // Key too short, too long, upper case.
        format!("{base}-{NAME}-f5e95e09b01"),
        format!("{base}-{NAME}-f5e95e09b0160"),
        format!("{base}-{NAME}-F5E95E09B016"),
        // Wrong parent basename.
        format!("amplihack-{NAME}-f5e95e09b016"),
    ];
    for (i, dest_name) in wrong.iter().enumerate() {
        let dest = cargo_dir(&fx.disk.join(dest_name));
        let link = fx.targets.join(NAME);
        symlink(&dest, &link).unwrap();
        assert_eq!(
            link_reason(fx.inspect(&link, &fx.no_links(), false)),
            "link not followed: outside allowed roots",
            "case {i}: {dest_name}"
        );
        fs::remove_file(&link).unwrap();
    }
    // An entry that is not a hook target name is never derived.
    let odd = "some-cache";
    let dest = cargo_dir(&fx.disk.join(format!("{base}-{odd}-f5e95e09b016")));
    assert!(!is_wrapper_redirect(
        &fx.targets.join(odd),
        &dest,
        &fx.no_links()
    ));
}

/// The derived folder must pass the same checks as an entry of
/// `AMPLIHACK_TARGET_LINK_ROOTS`.
#[test]
fn wrapper_link_into_an_unsafe_folder_is_not_followed() {
    let fx = Fx::new();
    // $HOME itself.
    let (link, dest) = fx.wrapper_linked(&fx.targets, &fx.home, NAME);
    assert_eq!(
        link_reason(fx.inspect(&link, &fx.no_links(), false)),
        "link not followed: outside allowed roots"
    );
    assert!(dest.join("debug/blob").exists());
    fs::remove_file(&link).unwrap();

    // Writable by others without the sticky bit.
    let open = fx.root.join("open");
    fs::create_dir_all(&open).unwrap();
    fs::set_permissions(&open, fs::Permissions::from_mode(0o777)).unwrap();
    let (link, dest) = fx.wrapper_linked(&fx.targets, &open, NAME);
    assert_eq!(
        link_reason(fx.inspect(&link, &fx.no_links(), false)),
        "link not followed: outside allowed roots"
    );
    assert!(dest.join("debug/blob").exists());
    fs::remove_file(&link).unwrap();

    // Group-writable is accepted, as for a listed entry (`/d0/ryan/cargo-targets`
    // is 0775 on candide).
    let group = fx.root.join("group");
    fs::create_dir_all(&group).unwrap();
    fs::set_permissions(&group, fs::Permissions::from_mode(0o775)).unwrap();
    let (link, dest) = fx.wrapper_linked(&fx.targets, &group, NAME);
    assert_eq!(
        fx.inspect(&link, &fx.no_links(), false).unwrap(),
        Shape::Link { dest }
    );

    // A folder owned by another uid: checked with a different caller uid.
    let other = LinkRoots::parse(
        &fx.roots(),
        None,
        Some(fx.home.as_path()),
        euid().wrapping_add(1),
    );
    let dest = fs::canonicalize(fs::read_link(&link).unwrap()).unwrap();
    assert!(!is_wrapper_redirect(
        &fs::canonicalize(&fx.targets).unwrap().join(NAME),
        &dest,
        &other
    ));
}

/// A wrapper-named destination that is not Cargo-shaped is still refused.
#[test]
fn wrapper_link_to_a_folder_that_is_not_cargo_shaped_is_refused() {
    let fx = Fx::new();
    let base = fx.targets.file_name().unwrap().to_str().unwrap();
    let dest = fx.disk.join(format!("{base}-{NAME}-f5e95e09b016"));
    fs::create_dir_all(dest.join("src")).unwrap();
    fs::write(dest.join("src/lib.rs"), "").unwrap();
    let link = fx.targets.join(NAME);
    symlink(&dest, &link).unwrap();
    assert_eq!(
        link_reason(fx.inspect(&link, &fx.no_links(), false)),
        "link not followed: not a cargo target"
    );
    assert!(dest.join("src/lib.rs").exists());
}
