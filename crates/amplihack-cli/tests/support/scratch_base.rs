//! A scratch base for tests that no framework lookup can see past.
//!
//! Recipe runs set `TMPDIR` inside `~/.amplihack`, which holds a real
//! framework install (`amplifier-bundle/` and `.claude/`). A test that builds
//! its fixture under such a `TMPDIR` is then exposed in one of three ways, each
//! of which failed in the PR #1971 QA run (`cargo test --workspace --locked
//! --no-fail-fast`, `TMPDIR=~/.amplihack/work/runs/<id>/runtime/tmp`):
//!
//! - `env_builder` `find_bundle_root`: an unbounded ancestor scan for
//!   `amplifier-bundle/`, so `AMPLIHACK_HOME` resolves to `~/.amplihack`
//!   instead of the value the test set up.
//! - `amplihack_types::paths::framework_roots_from`: an unbounded ancestor
//!   scan for `.claude/` and `src/amplihack/.claude/`, so the real framework
//!   files are found ahead of the fixture's.
//! - `issue_1743_autodrive_budget`: no lookup at all; it fails on any file
//!   that names the real `~/.claude` or `~/.amplihack`, and every fixture path
//!   appears in its stub logs.
//!
//! [`scratch_base`] therefore rejects a base when any ancestor holds one of
//! [`FRAMEWORK_MARKERS`], or when the base lies under `$HOME/.claude` or
//! `$HOME/.amplihack`. It tries `TMPDIR`, `/var/tmp` and `/tmp` in that order
//! and panics, naming the reason for each, when none qualifies: a silent
//! fallback would bring back the misleading failure it exists to prevent.
//!
//! `install::clone`'s walk-up is not a reason for this rule any more: its
//! tests run the walk over a synthetic layout and read no real directory
//! (issue #2196), so neither a framework install nor a stray repository
//! marker above the base (`/tmp/.git`, issue #1937) can change them.
//!
//! A base is also rejected when the recipe runner's recovery store would stop
//! a work root under it: a symbolic link, or a group- or world-writable folder
//! without the sticky bit, at or above it (issue #2346).
//!
//! This file uses only `std`. `amplihack-cli`'s `test_support`, the
//! `cli_integration` target, `amplihack-hooks`' `user_prompt` tests and
//! `amplihack-types`' `paths` tests include it with `#[path]`, so there is one
//! copy of the rule.

use std::path::{Path, PathBuf};

/// Directories whose presence in an ancestor makes some framework lookup
/// resolve there. See the module documentation for which lookup uses which.
pub const FRAMEWORK_MARKERS: [&str; 3] = ["amplifier-bundle", ".claude", "src/amplihack/.claude"];

/// Why `base` may not hold test fixtures, or `None` when it may.
pub fn scratch_base_rejection(base: &Path, home: Option<&Path>) -> Option<String> {
    if !base.is_dir() {
        return Some("is not a directory".to_string());
    }
    if let Some(home) = home {
        for real in [home.join(".claude"), home.join(".amplihack")] {
            if base.starts_with(&real) {
                return Some(format!("lies under the real {}", real.display()));
            }
        }
    }
    for ancestor in base.ancestors() {
        for marker in FRAMEWORK_MARKERS {
            let found = ancestor.join(marker);
            if found.is_dir() {
                return Some(format!("has {} above it", found.display()));
            }
        }
    }
    recovery_ancestor_rejection(base)
}

/// Issue #2346: why the recipe runner's recovery store would stop a work root
/// under `base`, or `None` when it would not.
///
/// The runner (`recipe_runner_rs::recovery::private_fs::Directory::open`)
/// stops a work root when a folder on its path, as written, is a symbolic
/// link, or has a group or other write bit and no sticky bit. A test that runs
/// a recipe under such a base fails with "unsafe recovery ancestor" for a
/// reason that has nothing to do with what it tests, so the base is rejected
/// by name and the next candidate tried. Under umask 0002 a `TMPDIR` made by
/// a shell is `0775`, which is the usual cause.
#[cfg(unix)]
fn recovery_ancestor_rejection(base: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    for ancestor in base.ancestors() {
        let Ok(meta) = std::fs::symlink_metadata(ancestor) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            return Some(format!(
                "has symbolic link {} at or above it, which the recovery store's ancestor rule \
                 stops",
                ancestor.display()
            ));
        }
        let mode = meta.mode();
        if mode & 0o022 != 0 && mode & 0o1000 == 0 {
            let bits = match (mode & 0o020 != 0, mode & 0o002 != 0) {
                (true, true) => "group-writable and world-writable",
                (false, true) => "world-writable",
                _ => "group-writable",
            };
            return Some(format!(
                "has {bits} {} (mode {:04o}) at or above it, which the recovery store's ancestor \
                 rule stops",
                ancestor.display(),
                mode & 0o7777
            ));
        }
    }
    None
}

#[cfg(not(unix))]
fn recovery_ancestor_rejection(_base: &Path) -> Option<String> {
    None
}

/// The first of `TMPDIR`, `/var/tmp` and `/tmp` that no framework lookup can
/// see past (module documentation). Panics, naming each candidate's fault,
/// when none qualifies.
pub fn scratch_base() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let candidates = [
        std::env::temp_dir(),
        PathBuf::from("/var/tmp"),
        PathBuf::from("/tmp"),
    ];
    let mut faults = Vec::new();
    for base in candidates {
        match scratch_base_rejection(&base, home.as_deref()) {
            None => return base,
            Some(reason) => faults.push(format!("{} {reason}", base.display())),
        }
    }
    panic!(
        "no clean scratch base for test fixtures: {}. Set TMPDIR to a directory \
         with none of {FRAMEWORK_MARKERS:?} above it and outside $HOME/.claude \
         and $HOME/.amplihack.",
        faults.join("; ")
    );
}
