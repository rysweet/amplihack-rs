//! Issue #1710 (D8): where auto-drive keeps its state, for readers in Rust.
//!
//! The writer is `amplifier-bundle/tools/autodrive_state.sh` and the round
//! recipes beside it. This is the only Rust module that knows those paths;
//! `tests_autodrive_paths.rs` runs the shell functions and checks that both
//! give the same key and root, and that every name here is one the writers
//! use. Every reader here fails closed and quietly: an unreadable, linked or
//! loosely permitted entry counts as absent, and no path is ever printed,
//! because the key is derived from an origin URL that may hold a token.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use super::run_record::{is_owned_dir, is_owned_file};

/// Subdirectory of the state directory that holds every auto-drive key.
pub(crate) const STATE_SUBDIR: &str = "auto-drive";
/// Phases recorded as done by `autodrive_mark_phase_done`. No reader needs
/// it yet; it is named here so the parity test pins it with the others.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const PHASES_FILE: &str = "phases.tsv";
/// `crusty-round-<N>.*`: the files of review round N.
pub(crate) const CRUSTY_ROUND_PREFIX: &str = "crusty-round-";
/// `merge-ready-round-<N>.*`: the files of merge-ready round N.
pub(crate) const MERGE_READY_ROUND_PREFIX: &str = "merge-ready-round-";
/// A copy of the last review round's record, with its `concern_count`.
pub(crate) const CRUSTY_LATEST: &str = "crusty-latest.json";

/// Directory entries read when counting rounds.
const MAX_ROUND_ENTRIES: usize = 500;
/// Mode bits a directory may not have: group or world write.
const LOOSE_DIR_BITS: u32 = 0o022;
/// Mode bits a state file may not have: any group or world access.
const LOOSE_FILE_BITS: u32 = 0o077;

/// `autodrive_state_key`: the origin without one trailing `.git` (`local`
/// without an origin), `/`, the ident, with every byte outside
/// `A-Za-z0-9._-` replaced by `_`, as `tr -c` does in the C locale.
pub(crate) fn state_key(origin: Option<&str>, ident: &str) -> String {
    let slug = origin.unwrap_or("local");
    let slug = slug.strip_suffix(".git").unwrap_or(slug);
    format!("{slug}/{ident}")
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-') {
                b as char
            } else {
                '_'
            }
        })
        .collect()
}

/// `autodrive_state_root` given `AMPLIHACK_STATE_DIR` and `HOME`: an empty
/// state directory counts as unset. `None` without either; the shell falls
/// back to `/tmp` there, which nothing here reads.
pub(crate) fn state_root_with(state_dir: Option<&OsStr>, home: Option<&Path>) -> Option<PathBuf> {
    let base = match state_dir.filter(|d| !d.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => home?.join(".amplihack").join("state"),
    };
    Some(base.join(STATE_SUBDIR))
}

/// [`state_root_with`] over this process's environment.
pub(crate) fn state_root() -> Option<PathBuf> {
    let state_dir = std::env::var_os("AMPLIHACK_STATE_DIR");
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|h| h.is_absolute());
    state_root_with(state_dir.as_deref(), home.as_deref())
}

/// The key's directory under `root`, when both are real directories owned
/// by this user with no group or world write bit.
pub(crate) fn state_dir(root: &Path, key: &str) -> Option<PathBuf> {
    if key.is_empty() || key == "." || key == ".." || key.contains('/') {
        return None;
    }
    let dir = root.join(key);
    (is_owned_dir(root, LOOSE_DIR_BITS) && is_owned_dir(&dir, LOOSE_DIR_BITS)).then_some(dir)
}

/// The number of distinct rounds `N` among regular files named
/// `<prefix>N` followed by `.` or the end, skipping `health` and `before`
/// files. Reads at most [`MAX_ROUND_ENTRIES`] entries; 0 when unreadable.
pub(crate) fn round_count(dir: &Path, prefix: &str) -> u32 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut rounds = BTreeSet::new();
    for entry in entries.take(MAX_ROUND_ENTRIES).filter_map(Result::ok) {
        // `file_type` does not follow a symlink.
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.contains("health") || name.contains("before") {
            continue;
        }
        if let Some(n) = round_number(name, prefix) {
            rounds.insert(n);
        }
    }
    u32::try_from(rounds.len()).unwrap_or(u32::MAX)
}

fn round_number(name: &str, prefix: &str) -> Option<u64> {
    let rest = name.strip_prefix(prefix)?;
    let digits = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    let (number, tail) = rest.split_at(digits);
    if number.is_empty() || !(tail.is_empty() || tail.starts_with('.')) {
        return None;
    }
    number.parse().ok()
}

/// `concern_count` from [`CRUSTY_LATEST`], when the file is a private
/// regular file of this user and the value is a whole non-negative number.
pub(crate) fn concern_count(dir: &Path) -> Option<u64> {
    let path = dir.join(CRUSTY_LATEST);
    if !is_owned_file(&path, LOOSE_FILE_BITS) {
        return None;
    }
    let text = crate::user_config::safe::read_bounded(&path).ok()??;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value.get("concern_count")?.as_u64()
}
