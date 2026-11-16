//! Issue #2589: the Codex execpolicy rules file amplihack owns.
//!
//! One file, `rules/amplihack-dangerous-commands.rules`, holding the whole
//! policy list rendered by [`crate::policy::codex`]. Every other file in that
//! directory belongs to the user and is neither read nor replaced.
//!
//! The write goes through `secure_files::write_private_atomic`, which refuses
//! a symlinked, non-regular or foreign-owned target and writes through a
//! temporary file created with `O_EXCL | O_NOFOLLOW` under an unpredictable
//! name. A helper that writes a predictable `<name>.tmp` with `fs::write`
//! would let a symlink planted there redirect the bytes on a shared machine
//! (SEC-C1).
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

/// Writes amplihack's rules file into `codex_home`, merging nothing and
/// replacing only its own file.
pub(in crate::commands::install) fn write_deny_rules(
    codex_home: &Path,
    instance: &[String],
) -> DenyRulesWrite {
    match fs::symlink_metadata(codex_home) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return DenyRulesWrite::Absent;
        }
        Err(error) => return DenyRulesWrite::Refused(reason(&error)),
        Ok(meta) if meta.file_type().is_symlink() => {
            return DenyRulesWrite::Refused("the Codex home is a symlink".to_string());
        }
        Ok(meta) if !meta.is_dir() => {
            return DenyRulesWrite::Refused("the Codex home is not a directory".to_string());
        }
        Ok(_) => {}
    }

    let dir = codex_home.join(RULES_DIR);
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
