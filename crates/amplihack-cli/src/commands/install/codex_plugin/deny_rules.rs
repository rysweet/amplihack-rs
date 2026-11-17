//! Issue #2589: the Codex execpolicy rules file amplihack owns.
//!
//! One file, `rules/amplihack-dangerous-commands.rules`, holding the whole
//! policy list rendered by [`crate::policy::codex`]. Every other file in that
//! directory belongs to the user and is neither read nor replaced, and so
//! does a file standing at amplihack's own path without amplihack's
//! ownership marker: the install leaves it as found and says why, exactly as
//! the uninstall keeps it (issue #2589, R7).
//!
//! The write goes through `secure_files::write_private_atomic`, which refuses
//! a symlinked, non-regular or foreign-owned target and writes through a
//! temporary file created with `O_EXCL | O_NOFOLLOW`. Those two flags, not the
//! temporary name, are what makes it safe: the name is predictable
//! (`.<name>.tmp.<pid>.<nanos>`), but a name already taken is an error rather
//! than an overwrite, and a symlink planted at that name is never followed. A
//! helper that writes `<name>.tmp` with `fs::write` has neither property and
//! would let such a link redirect the bytes on a shared machine (SEC-C1).
//!
//! Nothing here creates a Codex home that does not exist: a harness
//! amplihack does not manage is left alone (SEC-A3).

use crate::policy;
use amplihack_utils::secure_files::{
    ensure_private_directory, read_bounded_nofollow, write_private_atomic,
};
use std::fs;
use std::path::Path;

/// The directory inside a Codex home that holds execpolicy rules.
pub(in crate::commands::install) const RULES_DIR: &str = "rules";

/// amplihack's own rules file. The name carries the owner, so a rules file
/// written by anything else is never replaced or removed.
pub(in crate::commands::install) const RULES_FILE: &str = "amplihack-dangerous-commands.rules";

/// Largest existing file read for the content comparison. The path is
/// writable by the agents this policy constrains, so the read is bounded and
/// does not follow a symlink (SEC-V5). The bound also decides a file's fate:
/// a file too large to read is a file whose marker cannot be seen, and an
/// unseen marker means the file stays.
const MAX_RULES_BYTES: u64 = 64 * 1024;

/// Why a file standing at the rules path is neither replaced nor removed.
///
/// One constant for both ends, so `amplihack install` and `amplihack
/// uninstall` cannot drift into a state where one command deletes a file the
/// other refuses to touch (issue #2589, R7).
pub(in crate::commands::install) const UNMARKED_REASON: &str =
    "the file there does not carry amplihack's marker line";

/// Whether `bytes` begin with the line amplihack writes at the top of its own
/// rules file.
///
/// The ownership marker is a cooperative provenance marker, not a signature:
/// it says a file may be replaced, never that its contents can be trusted,
/// and nothing reads that file for any purpose but this check and a byte
/// comparison (SEC-A3). Matched on the leading bytes with `starts_with`, and
/// through `from_utf8` rather than `from_utf8_lossy`, so a file that is not
/// valid UTF-8 is not amplihack's. Extracted so the write and the removal
/// share one predicate (SEC-V1).
fn marked(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok_and(|text| text.starts_with(policy::codex::OWNERSHIP_MARKER))
}

/// What [`write_deny_rules`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::commands::install) enum DenyRulesWrite {
    Written,
    /// The file already holds exactly these rules.
    AlreadyCurrent,
    /// There is no Codex home at that path; none was created.
    Absent,
    /// The file was left as it was, for the reason given. The reason is a
    /// fixed phrase or an error kind, never file content (SEC-D3).
    Refused(String),
}

/// What [`remove_deny_rules`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::commands::install) enum DenyRulesRemoval {
    Removed,
    /// Nothing was there.
    Absent,
    /// Something is there that amplihack did not write; it stays.
    Kept(String),
}

/// An I/O error as a reason: the fixed phrase of a refusal raised by the
/// secure-file helpers, or the error kind.
fn reason(error: &std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::InvalidData {
        error.to_string()
    } else {
        error.kind().to_string()
    }
}

/// Whether `path` can hold a directory amplihack writes into: `Ok(true)` for
/// a directory, `Ok(false)` for nothing at all, and `Err(reason)` for a
/// symlink, a non-directory or a path that cannot be read. `what` names the
/// directory in the reason and never holds file content (SEC-D3).
///
/// Both directories on the way to the rules file need this. The write itself
/// is protected by `O_NOFOLLOW`, but `ensure_private_directory` below is
/// `create_dir_all` followed by `set_permissions`, and both of those follow a
/// link: without the check, a link planted at `<CODEX_HOME>/rules` by another
/// user on a shared machine has its target chmod-ed 0700 and the rules file
/// written inside it (SEC-D2).
fn directory_state(path: &Path, what: &str) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(reason(&error)),
        Ok(meta) if meta.file_type().is_symlink() => Err(format!("{what} is a symlink")),
        Ok(meta) if !meta.is_dir() => Err(format!("{what} is not a directory")),
        Ok(_) => Ok(true),
    }
}

/// Writes amplihack's rules file into `codex_home`, merging nothing and
/// replacing only its own file.
pub(in crate::commands::install) fn write_deny_rules(
    codex_home: &Path,
    instance: &[String],
) -> DenyRulesWrite {
    match directory_state(codex_home, "the Codex home") {
        Ok(true) => {}
        // Nothing here creates a Codex home that does not exist: a harness
        // amplihack does not manage is left alone (SEC-A3).
        Ok(false) => return DenyRulesWrite::Absent,
        Err(fault) => return DenyRulesWrite::Refused(fault),
    }

    // The rules directory gets the same check as the home above, because
    // `ensure_private_directory` would follow a link planted there and
    // chmod and write into its target instead (SEC-D2). An absent directory
    // is the ordinary case and is created below.
    let dir = codex_home.join(RULES_DIR);
    if let Err(fault) = directory_state(&dir, "the Codex rules directory") {
        return DenyRulesWrite::Refused(fault);
    }
    if let Err(error) = ensure_private_directory(&dir) {
        return DenyRulesWrite::Refused(reason(&error));
    }

    let text = policy::codex::rules_file(instance);
    let path = dir.join(RULES_FILE);
    // Issue #2589 (R7): what stands at the path decides, and the arm order
    // is load-bearing. `Err` comes first so a symlink, a non-regular file,
    // an oversized one or a permission fault reports as itself rather than
    // as an absent marker. Then the byte comparison, because equality with
    // what this run renders implies the marker is there. Then the marker,
    // which is the only thing that permits a replacement: without it the
    // file is someone else's and is left exactly as found, matching
    // `remove_deny_rules` and the promise in `codex_plugin/mod.rs` that
    // hand-written files are untouched.
    //
    // The read happens after `ensure_private_directory` above, so both the
    // comparison and the write run inside a directory already narrowed to
    // its owner (SEC-A2).
    match read_bounded_nofollow(&path, MAX_RULES_BYTES) {
        Err(error) => return DenyRulesWrite::Refused(reason(&error)),
        Ok(Some((current, _))) => {
            if current == text.as_bytes() {
                return DenyRulesWrite::AlreadyCurrent;
            }
            if !marked(&current) {
                return DenyRulesWrite::Refused(UNMARKED_REASON.to_string());
            }
        }
        // Nothing there: the write below creates it.
        Ok(None) => {}
    }
    match write_private_atomic(&dir, RULES_FILE, text.as_bytes()) {
        Ok(()) => DenyRulesWrite::Written,
        Err(error) => DenyRulesWrite::Refused(reason(&error)),
    }
}

/// Removes amplihack's rules file from `codex_home`, and nothing else: a
/// file without the ownership marker, or a symlink at that path, is kept.
/// The directory itself may hold the user's own rules and is never removed.
pub(in crate::commands::install) fn remove_deny_rules(codex_home: &Path) -> DenyRulesRemoval {
    let path = codex_home.join(RULES_DIR).join(RULES_FILE);
    let bytes = match read_bounded_nofollow(&path, MAX_RULES_BYTES) {
        Ok(Some((bytes, _))) => bytes,
        Ok(None) => return DenyRulesRemoval::Absent,
        Err(error) => return DenyRulesRemoval::Kept(reason(&error)),
    };
    if !marked(&bytes) {
        return DenyRulesRemoval::Kept(UNMARKED_REASON.to_string());
    }
    match fs::remove_file(&path) {
        Ok(()) => DenyRulesRemoval::Removed,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => DenyRulesRemoval::Absent,
        Err(error) => DenyRulesRemoval::Kept(reason(&error)),
    }
}
