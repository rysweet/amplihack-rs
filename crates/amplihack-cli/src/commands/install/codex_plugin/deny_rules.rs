//! Issue #2589: the Codex execpolicy rules file amplihack owns.
//!
//! One file, `rules/amplihack-dangerous-commands.rules`, holding the whole
//! policy list rendered by [`crate::policy::codex`]. Every other file in that
//! directory belongs to the user and is neither read nor replaced.
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
/// does not follow a symlink (SEC-V5).
const MAX_RULES_BYTES: u64 = 64 * 1024;

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
    // An unreadable, oversized, symlinked or absent file is not current, so
    // the write below decides what happens to it.
    if let Ok(Some((current, _))) = read_bounded_nofollow(&path, MAX_RULES_BYTES)
        && current == text.as_bytes()
    {
        return DenyRulesWrite::AlreadyCurrent;
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
    let marked = std::str::from_utf8(&bytes)
        .is_ok_and(|text| text.starts_with(policy::codex::OWNERSHIP_MARKER));
    if !marked {
        return DenyRulesRemoval::Kept("it was not written by amplihack".to_string());
    }
    match fs::remove_file(&path) {
        Ok(()) => DenyRulesRemoval::Removed,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => DenyRulesRemoval::Absent,
        Err(error) => DenyRulesRemoval::Kept(reason(&error)),
    }
}
