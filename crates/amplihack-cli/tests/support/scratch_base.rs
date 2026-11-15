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
//! The choice is made once per test process and then reused, because fixtures
//! point `TMPDIR` at a directory they own. A later caller that read `TMPDIR`
//! again would put its fixture inside that private tree and lose it the moment
//! the owning test's `TempDir` dropped, taking an unrelated test with it. The
//! rule is about where fixtures may live, which cannot change mid-process;
//! code under test still reads whatever `TMPDIR` a fixture sets.
//!
//! This file uses only `std`. `amplihack-cli`'s `test_support`, the
//! `cli_integration` target, `amplihack-hooks`' `user_prompt` tests and
//! `amplihack-types`' `paths` tests include it with `#[path]`, so there is one
//! copy of the rule.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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
    None
}

/// The first of `TMPDIR`, `/var/tmp` and `/tmp` that no framework lookup can
/// see past (module documentation). Decided on the first call and reused for
/// the life of the process, so a fixture that repoints `TMPDIR` at a tree it
/// owns cannot send a concurrent test's fixtures into it. Panics, naming each
/// candidate's fault, when none qualifies.
pub fn scratch_base() -> PathBuf {
    static CHOSEN: OnceLock<PathBuf> = OnceLock::new();
    CHOSEN.get_or_init(choose_scratch_base).clone()
}

fn choose_scratch_base() -> PathBuf {
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
