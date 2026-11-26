//! Issue #2589 (TDD): the Codex execpolicy rules file amplihack owns.
//!
//! Expected API, in `codex_plugin/deny_rules.rs`:
//!
//! ```ignore
//! pub(super) const RULES_DIR: &str = "rules";
//! pub(super) const RULES_FILE: &str = "amplihack-dangerous-commands.rules";
//! pub(super) enum DenyRulesWrite { Written, AlreadyCurrent, Absent, Refused(String) }
//! pub(super) enum DenyRulesRemoval { Removed, Absent, Kept(String) }
//! pub(super) fn write_deny_rules(codex_home: &Path, instance: &[String]) -> DenyRulesWrite;
//! pub(super) fn remove_deny_rules(codex_home: &Path) -> DenyRulesRemoval;
//! /// Why a file at the rules path is neither replaced nor removed; one
//! /// constant, so install and uninstall cannot drift apart (R7).
//! pub(super) const UNMARKED_REASON: &str =
//!     "the file there does not carry amplihack's marker line";
//! ```
//!
//! The tests are grouped into submodules so each file stays inside the
//! installer's 300-line module boundary, which
//! `codex_frontdoor_contract::installer_modules_obey_the_accepted_size_boundary`
//! pins: `write` for what the writer puts on disk, `removal` for what
//! `amplihack uninstall` takes away, and `placement` for the two directories
//! on the way to the file. The fixtures below are shared by all three.

use super::deny_rules::{
    DenyRulesRemoval, DenyRulesWrite, RULES_DIR, RULES_FILE, UNMARKED_REASON, remove_deny_rules,
    write_deny_rules,
};
use crate::policy;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};

/// A Codex home under a temp directory. Never the developer's `~/.codex`.
struct Home {
    _tmp: tempfile::TempDir,
    path: PathBuf,
}

impl Home {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("codex");
        fs::create_dir_all(&path).unwrap();
        Self { _tmp: tmp, path }
    }

    fn rules_dir(&self) -> PathBuf {
        self.path.join(RULES_DIR)
    }

    fn rules_file(&self) -> PathBuf {
        self.rules_dir().join(RULES_FILE)
    }
}

/// Bytes, mtime and inode: a rewrite, even to the same bytes, changes one.
fn fingerprint(path: &Path) -> (Vec<u8>, std::time::SystemTime, u64) {
    let meta = fs::symlink_metadata(path).unwrap();
    (
        fs::read(path).unwrap(),
        meta.modified().unwrap(),
        meta.ino(),
    )
}

trait ExpectWritten {
    fn expect_written(self);
}

impl ExpectWritten for DenyRulesWrite {
    fn expect_written(self) {
        assert_eq!(self, DenyRulesWrite::Written);
    }
}

mod ownership;
mod placement;
mod removal;
mod write;
