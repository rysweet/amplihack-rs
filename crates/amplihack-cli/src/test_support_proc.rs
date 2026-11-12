//! A synthetic process table for one lib test (issue #2196).
//!
//! [`fake_proc_scope`] builds a private fake `/proc` with `cli_integration`'s
//! `FakeProc` (the one builder, shared through `#[path]`) and points
//! `procfs::proc_root()` at it on the calling thread until the scope drops.
//! [`proc_scope_at`] installs an existing tree, for a hand-built one or for
//! a thread the test spawns: the override does not pass to new threads.
//!
//! Lib tests never set the seam's environment variable; the override is
//! `cfg(test)` only and reaches no child process.

#[path = "../tests/support/fake_proc.rs"]
// `fake_proc.rs` allows dead code itself; the lib does not use every re-export.
#[allow(unused_imports)]
mod fake_proc;

pub(crate) use fake_proc::{FAKE_PID_BASE, FakeEntry, FakeProc, SELF_ENVIRON};

use crate::commands::agent::procfs::{ProcRootOverride, checked_fake_tree, override_proc_root};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The one fake process table `procfs::proc_root()` returns in lib tests
/// when no override is set and the seam's variable is unset or refused
/// (issue #2196, D1 step 3). It holds only `self/environ`, so a test that
/// installs nothing sees no processes and never the host table.
///
/// Created once per test process: a randomly named directory, mode 0700
/// at creation, under the temp dir (the seam accepts trees only there),
/// seeded with `seed_proc_tree` and checked once with the seam's own tree
/// checks. It is kept for the whole run and left behind afterwards; it is
/// read-only to callers, who get a `&'static Path` and no builder.
/// Panics, naming the path and reason, when it cannot be created or fails
/// the checks.
pub(crate) fn shared_default_root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let dir = tempfile::Builder::new()
            .prefix("amplihack-fake-proc-shared-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(std::env::temp_dir())
            .unwrap_or_else(|e| {
                panic!(
                    "create the shared fake process table under {}: {e}",
                    std::env::temp_dir().display()
                )
            })
            .keep();
        fake_proc::seed_proc_tree(&dir);
        checked_fake_tree("shared_default_root", &dir)
    })
}

/// A fake process table installed for the calling thread. The guard is the
/// first field, so the override is removed before the tree is deleted.
pub(crate) struct FakeProcScope {
    _guard: ProcRootOverride,
    pub(crate) fake: FakeProc,
}

/// A new empty table (only `self/environ`), installed for this thread.
pub(crate) fn fake_proc_scope() -> FakeProcScope {
    let fake = FakeProc::new();
    FakeProcScope {
        _guard: override_proc_root(fake.path()),
        fake,
    }
}

/// Install the existing fake table `fake` for this thread until the guard
/// drops, for a thread the test spawns or a fixture that owns its
/// `FakeProc`. Only a tree built by `FakeProc` can be installed.
pub(crate) fn proc_scope_at(fake: &FakeProc) -> ProcRootOverride {
    override_proc_root(fake.path())
}

impl FakeProcScope {
    /// The tree's root.
    pub(crate) fn path(&self) -> &Path {
        self.fake.path()
    }

    /// A running `claude` of this user at fake `pid` whose
    /// `CLAUDE_CONFIG_DIR` is `config_dir`. No `exe` link. Panics on a pid
    /// below `FAKE_PID_BASE`, so the entry can never name a host process.
    pub(crate) fn running_session(&self, pid: u32, config_dir: &Path) -> FakeEntry {
        let config = config_dir
            .to_str()
            .unwrap_or_else(|| panic!("config dir {} is not UTF-8", config_dir.display()));
        let entry = self.fake.process(pid);
        entry
            .cmdline(&["claude"])
            .status('S')
            .environ(&[("CLAUDE_CONFIG_DIR", config)]);
        entry
    }
}

#[cfg(test)]
#[path = "test_support_proc_tests.rs"]
mod tests;
