//! The one definition of the environment variables that redirect a Claude
//! settings write away from the `HOME` a test gave it, and of the guards that
//! keep a test's writes inside its own temporary folder (issue #2922).
//!
//! # The fault this prevents
//!
//! `install::launch_repair` writes `$CLAUDE_CONFIG_DIR/settings.json` when that
//! variable is set, and falls back to `$HOME/.claude/settings.json` when it is
//! not. A test that points `HOME` at a temporary folder but leaves an
//! inherited `CLAUDE_CONFIG_DIR` in place therefore writes its fixture's
//! settings into whatever profile the inherited value names. On 21 November
//! 2026 a workspace test run inside an agent launch replaced a developer's
//! live profile that way: every hook in it then pointed at a temporary folder
//! that no longer existed, and the next session on that login ran with none of
//! the owner's guards.
//!
//! # What this file does about it
//!
//! [`CLAUDE_PROFILE_ENV_VARS`] names the five variables that can redirect such
//! a write. Nothing here takes a caller-supplied list, so no call site can
//! narrow the set.
//!
//! - [`purge_inherited`] removes all five from this process, once and for all,
//!   with no restore. A test binary fires it at its earliest serialised point
//!   so that a value inherited from the launching session never reaches any
//!   test. It is the backstop for a test that reaches a settings writer
//!   without taking any of the guards below.
//! - [`ClaudeProfileIsolation`] clears all five for as long as it is held and
//!   restores the prior values on drop, panics included. The `HOME` primitives
//!   of each crate's `test_support` hold one, or clear the same five under
//!   their own restore, so isolation follows from setting `HOME` at all rather
//!   than from remembering to ask for it.
//! - [`with_config_dir`] is the only way a test may keep `CLAUDE_CONFIG_DIR`
//!   set, and it accepts a directory inside the temporary `HOME` only.
//! - [`remove_from_command`] applies the same list to a child process.
//!
//! The default is to clear, never to redirect: a cleared `CLAUDE_CONFIG_DIR`
//! leaves the HOME-derived path in force, which is already inside the
//! temporary home, and cannot change what any existing assertion observes.
//!
//! # Soundness
//!
//! Every function here mutates the process environment, which `std` treats as
//! unsafe from edition 2024 onwards because it is unsound while another thread
//! reads it. Apart from [`purge_inherited`], each one requires the caller to
//! hold the test binary's single environment lock (`env_lock()`, or the quiet
//! child of `cli_integration`, where the calling test is the only test).
//! [`purge_inherited`] is documented at its own definition.
//!
//! This file is shared by `#[path]` include. It depends on `std` only, so no
//! crate that includes it gains a dependency, and it lives under `tests/`, so
//! it compiles into no release binary.

#![allow(dead_code)]

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

/// The variables that can send a settings write outside the `HOME` a test was
/// given, sorted by name.
///
/// * `AMPLIHACK_CLAUDE_AUTO_SWITCH` (`rotator_status::TOGGLE_ENV`) turns the
///   login auto-switch on, which selects a profile and exports the two below.
/// * `AMPLIHACK_CLAUDE_PROFILE` (`commands::profile::PROFILE_ENV`) names the
///   login profile a launch is using.
/// * `AMPLIHACK_CLAUDE_PROFILE_ROOT` (`rotator_status::ROOT_ENV`) names the
///   directory those profiles live in.
/// * `AMPLIHACK_HOME` moves amplihack's own state out of `$HOME/.amplihack`.
/// * `CLAUDE_CONFIG_DIR` is read directly by the launch repair, which writes
///   `$CLAUDE_CONFIG_DIR/settings.json` whenever it is set.
///
/// `crate::test_support::claude_profile_isolation` has a test in
/// `amplihack-cli` asserting that this list still holds the product's own
/// exported constants, so a rename there fails the build rather than leaving
/// this list clearing a name nothing reads.
pub const CLAUDE_PROFILE_ENV_VARS: [&str; 5] = [
    "AMPLIHACK_CLAUDE_AUTO_SWITCH",
    "AMPLIHACK_CLAUDE_PROFILE",
    "AMPLIHACK_CLAUDE_PROFILE_ROOT",
    "AMPLIHACK_HOME",
    "CLAUDE_CONFIG_DIR",
];

/// Remove every [`CLAUDE_PROFILE_ENV_VARS`] entry from this process for good.
///
/// There is no restore: a test process has no business carrying a login
/// profile inherited from the session that launched it, and a value put back
/// at the end of a test run would only reach the next thing this process
/// starts. Removing a variable that is already absent does nothing, so calling
/// this twice is the same as calling it once.
///
/// # Where to call it
///
/// At the earliest point in a test binary at which only one thread is running
/// the tests' own code: the initialiser of the binary's environment lock
/// (`env_lock()`), or, in a binary with no such lock, the start of each quiet
/// child. Those are the places the binary already treats as the serialisation
/// point for environment mutation.
///
/// # Safety of the mutation
///
/// This is the one function here that does not require the caller to hold a
/// lock, because its callers are the lock's own initialiser and a process in
/// which a single test runs alone. A caller that can hold the binary's
/// environment lock should do so.
pub fn purge_inherited() {
    for name in CLAUDE_PROFILE_ENV_VARS {
        // SAFETY: called from a test binary's environment-lock initialiser or
        // from a quiet child in which the calling test is the only test, as
        // the documentation above requires.
        unsafe { std::env::remove_var(name) };
    }
}

/// Clears every [`CLAUDE_PROFILE_ENV_VARS`] entry while it is held, and puts
/// the prior values back on drop, including when the test panics.
///
/// The caller must hold the test binary's environment lock for the whole
/// lifetime of the guard. `HomeGuard::set` holds one of these, and the other
/// `HOME` primitives clear the same five, so a test that sets `HOME` through a
/// crate's own primitive is isolated without naming this type.
///
/// A variable that was absent is left absent on drop. Values are restored
/// newest first, so a guard nested inside another restores exactly the state
/// the outer guard had established.
pub struct ClaudeProfileIsolation {
    saved: Vec<(&'static str, Option<OsString>)>,
}

impl ClaudeProfileIsolation {
    /// Clear all five, remembering what each one held.
    pub fn new() -> Self {
        let mut saved = Vec::with_capacity(CLAUDE_PROFILE_ENV_VARS.len());
        for name in CLAUDE_PROFILE_ENV_VARS {
            saved.push((name, std::env::var_os(name)));
            // SAFETY: the caller holds the binary's environment lock, as this
            // type's documentation requires.
            unsafe { std::env::remove_var(name) };
        }
        Self { saved }
    }
}

impl Drop for ClaudeProfileIsolation {
    fn drop(&mut self) {
        for (name, prior) in self.saved.drain(..).rev() {
            // SAFETY: the caller still holds the environment lock; this guard
            // is dropped before the lock is released. Nothing here can panic,
            // so an unwinding test restores the same state a passing one does.
            unsafe {
                match prior {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }
}

/// Isolate the five as [`ClaudeProfileIsolation::new`] does, then point
/// `CLAUDE_CONFIG_DIR` at `dir`, which must be inside `home`.
///
/// This is the opt-in for a test that must exercise a set `CLAUDE_CONFIG_DIR`,
/// and it is the only supported way to keep that variable set in a test. Call
/// it after `HOME` has been pointed at `home`; the returned guard restores the
/// prior value of every one of the five on drop, panics included.
///
/// # Panics
///
/// If `dir` does not lie inside `home`, before anything is changed. Both paths
/// are resolved first (`dir` through its nearest existing ancestor when it
/// does not exist yet) and compared by path component, so neither a `..`
/// segment nor a symlink planted inside `home` nor a sibling whose name merely
/// starts with `home`'s can slip a write out of the temporary folder.
pub fn with_config_dir(home: &Path, dir: &Path) -> ClaudeProfileIsolation {
    let resolved_home = resolve_for_containment(home);
    let resolved_dir = resolve_for_containment(dir);
    assert!(
        resolved_dir.starts_with(&resolved_home),
        "CLAUDE_CONFIG_DIR {} resolves to {}, which is outside the test's HOME {}; \
         a test may only point it inside its own temporary home",
        dir.display(),
        resolved_dir.display(),
        resolved_home.display()
    );
    // `new` has already saved `CLAUDE_CONFIG_DIR` with the other four, so the
    // value set below is undone on drop.
    let isolation = ClaudeProfileIsolation::new();
    // SAFETY: as in `ClaudeProfileIsolation::new`; the caller holds the lock.
    unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", dir) };
    isolation
}

/// `path` with every symlink and `..` segment resolved, so that two paths can
/// be compared for containment.
///
/// A path that does not exist yet cannot be canonicalised, so its nearest
/// existing ancestor is resolved and the remaining components are applied to
/// it one by one. That keeps `<home>/not-created-yet` usable while still
/// resolving anything real on the way to it. A `..` among those remaining
/// components removes the last component of the resolved prefix, so
/// `<home>/missing/../../other` resolves outside `<home>` as the kernel would
/// resolve it once `missing` exists. The prefix holds no symlinks at that
/// point, so removing a component lexically is exact.
fn resolve_for_containment(path: &Path) -> PathBuf {
    if let Ok(resolved) = path.canonicalize() {
        return resolved;
    }
    let Some(parent) = path.parent() else {
        // A root or a relative path with no parent left: nothing to resolve.
        return path.to_path_buf();
    };
    match path.components().next_back() {
        Some(Component::Normal(name)) => resolve_for_containment(parent).join(name),
        Some(Component::ParentDir) => {
            let mut resolved = resolve_for_containment(parent);
            resolved.pop();
            resolved
        }
        _ => path.to_path_buf(),
    }
}

/// Remove every [`CLAUDE_PROFILE_ENV_VARS`] entry from `command`'s
/// environment, so a child cannot inherit a profile from this process.
///
/// A command built with `env_clear()` and an explicit allowlist is the
/// stronger form and should be preferred where the call site permits it; this
/// is the removal to add where the command keeps the parent's environment, and
/// the second line of defence where the allowlist might later grow.
pub fn remove_from_command(command: &mut Command) -> &mut Command {
    for name in CLAUDE_PROFILE_ENV_VARS {
        command.env_remove(name);
    }
    command
}
