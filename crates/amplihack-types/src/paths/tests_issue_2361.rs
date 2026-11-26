//! Issue #2361: a Git checkout is never the hook state root, even when
//! `AMPLIHACK_HOME` points at one.
//!
//! - `is_git_checkout` probes only `<dir>/.git`, with `symlink_metadata`.
//! - `resolve_state_root(amplihack_home, home, amplihack_home_is_checkout)`
//!   skips `amplihack_home` when the caller says it is a checkout.
//! - `amplihack_state_root()` does the probe and falls through to
//!   `$HOME/.amplihack`, or `None` when `HOME` is unusable.
//!
//! Only the `amplihack_state_root` tests read the environment; they hold the
//! crate's single `env_lock()` throughout.

use super::{amplihack_state_root, is_git_checkout, resolve_state_root};
use crate::test_support::{env_lock, scratch_base};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

fn os(value: &str) -> Option<&OsStr> {
    Some(OsStr::new(value))
}

/// A fresh directory under the shared scratch base, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let base = scratch_base::scratch_base();
        let mut n = 0u32;
        loop {
            let path = base.join(format!(
                "amplihack-types-2361-{name}-{}-{n}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
                Err(e) => panic!("cannot create {}: {e}", path.display()),
            }
        }
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Restores one environment variable on drop. Callers hold `env_lock()`.
struct Restore {
    key: &'static str,
    previous: Option<OsString>,
}

impl Restore {
    fn set(key: &'static str, value: Option<&OsStr>) -> Self {
        let previous = std::env::var_os(key);
        match value {
            Some(value) => unsafe { std::env::set_var(key, value) },
            None => unsafe { std::env::remove_var(key) },
        }
        Self { key, previous }
    }
}

impl Drop for Restore {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => unsafe { std::env::set_var(self.key, value) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

// ---- is_git_checkout ----

#[test]
fn dot_git_directory_is_a_checkout() {
    let dir = Scratch::new("git-dir");
    fs::create_dir(dir.path().join(".git")).unwrap();
    assert!(is_git_checkout(dir.path()));
}

#[test]
fn dot_git_file_is_a_checkout() {
    // A linked worktree or a submodule has a `.git` file.
    let dir = Scratch::new("git-file");
    fs::write(
        dir.path().join(".git"),
        "gitdir: /elsewhere/.git/worktrees/x\n",
    )
    .unwrap();
    assert!(is_git_checkout(dir.path()));
}

#[test]
fn directory_without_dot_git_is_not_a_checkout() {
    let dir = Scratch::new("no-git");
    fs::create_dir(dir.path().join("amplifier-bundle")).unwrap();
    assert!(!is_git_checkout(dir.path()));
}

#[cfg(unix)]
#[test]
fn dangling_dot_git_symlink_is_a_checkout() {
    // Fail closed: the symlink is not followed, so a dangling one still counts.
    let dir = Scratch::new("git-dangling");
    std::os::unix::fs::symlink(dir.path().join("missing-target"), dir.path().join(".git")).unwrap();
    assert!(!dir.path().join(".git").exists(), "the symlink dangles");
    assert!(is_git_checkout(dir.path()));
}

#[test]
fn subdirectory_of_a_checkout_is_not_itself_a_checkout() {
    // The probe never climbs to parent directories.
    let dir = Scratch::new("git-subdir");
    fs::create_dir(dir.path().join(".git")).unwrap();
    let sub = dir.path().join("nested");
    fs::create_dir(&sub).unwrap();
    assert!(!is_git_checkout(&sub));
}

#[test]
fn missing_directory_is_not_a_checkout() {
    let dir = Scratch::new("git-missing");
    assert!(!is_git_checkout(&dir.path().join("does-not-exist")));
}

#[cfg(unix)]
#[test]
fn regular_file_is_treated_as_a_checkout() {
    // `<file>/.git` fails with ENOTDIR, not NotFound; the probe fails closed.
    let dir = Scratch::new("git-enotdir");
    let file = dir.path().join("not-a-dir");
    fs::write(&file, b"").unwrap();
    assert!(is_git_checkout(&file));
}

// ---- resolve_state_root: the checkout flag (pure) ----

#[test]
fn checkout_amplihack_home_falls_through_to_home_dot_amplihack() {
    assert_eq!(
        resolve_state_root(os("/srv/checkout"), os("/home/alice"), true),
        Some(PathBuf::from("/home/alice/.amplihack"))
    );
}

#[test]
fn checkout_amplihack_home_without_home_is_none() {
    // Fail closed: no fallback to the checkout, `.` or the working directory.
    assert_eq!(resolve_state_root(os("/srv/checkout"), None, true), None);
}

#[test]
fn checkout_amplihack_home_with_unusable_home_is_none() {
    for home in [os(""), os("relative/home"), os("/home/../root")] {
        assert_eq!(
            resolve_state_root(os("/srv/checkout"), home, true),
            None,
            "HOME {home:?} must not produce a state root"
        );
    }
}

#[test]
fn non_checkout_amplihack_home_still_wins() {
    assert_eq!(
        resolve_state_root(os("/srv/amplihack"), os("/home/alice"), false),
        Some(PathBuf::from("/srv/amplihack"))
    );
}

#[test]
fn checkout_flag_does_not_apply_to_home() {
    // `home` is never tested against the predicate.
    assert_eq!(
        resolve_state_root(None, os("/home/alice"), true),
        Some(PathBuf::from("/home/alice/.amplihack"))
    );
}

// ---- amplihack_state_root: probes AMPLIHACK_HOME ----

#[test]
fn amplihack_state_root_skips_a_checkout_with_a_git_directory() {
    let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let checkout = Scratch::new("env-git-dir");
    fs::create_dir(checkout.path().join(".git")).unwrap();
    let _amp = Restore::set("AMPLIHACK_HOME", Some(checkout.path().as_os_str()));
    let _home = Restore::set("HOME", os("/home/env-user"));
    assert_eq!(
        amplihack_state_root(),
        Some(PathBuf::from("/home/env-user/.amplihack"))
    );
}

#[test]
fn amplihack_state_root_skips_a_linked_worktree() {
    let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let worktree = Scratch::new("env-git-file");
    fs::write(worktree.path().join(".git"), "gitdir: /elsewhere\n").unwrap();
    let _amp = Restore::set("AMPLIHACK_HOME", Some(worktree.path().as_os_str()));
    let _home = Restore::set("HOME", os("/home/env-user"));
    assert_eq!(
        amplihack_state_root(),
        Some(PathBuf::from("/home/env-user/.amplihack"))
    );
}

#[test]
fn amplihack_state_root_is_none_for_a_checkout_without_home() {
    let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let checkout = Scratch::new("env-git-nohome");
    fs::create_dir(checkout.path().join(".git")).unwrap();
    let _amp = Restore::set("AMPLIHACK_HOME", Some(checkout.path().as_os_str()));
    let _home = Restore::set("HOME", None);
    assert_eq!(amplihack_state_root(), None);
}

#[test]
fn amplihack_state_root_keeps_a_non_checkout_amplihack_home() {
    let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let amp = Scratch::new("env-plain");
    let _amp = Restore::set("AMPLIHACK_HOME", Some(amp.path().as_os_str()));
    let _home = Restore::set("HOME", os("/home/env-user"));
    assert_eq!(amplihack_state_root(), Some(amp.path().to_path_buf()));
}

// ---- crate-root export ----

#[test]
fn is_git_checkout_is_reexported_from_crate_root() {
    let _: fn(&Path) -> bool = crate::is_git_checkout;
    let _: fn(Option<&OsStr>, Option<&OsStr>, bool) -> Option<PathBuf> = crate::resolve_state_root;
}
