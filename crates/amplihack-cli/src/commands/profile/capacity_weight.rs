//! Issue #1797: capacity weights and the usable-now session threshold.
//!
//! Weights live in `<root>/.auto-switch.d/capacity-weights.json`, an object
//! of profile name to a number in (0, 1000], edited by hand. A profile has
//! no settings file of its own, so the weights share one file. A missing
//! file means every weight is 1.0, silently; a file that cannot be trusted
//! or parsed means every weight is 1.0 with one fixed warning; a bad value
//! means that weight is 1.0 with one warning naming the profile. The file is
//! only read, never written, and its keys are used only for lookup.
//!
//! The threshold is `--usable-session-pct`, else
//! `AMPLIHACK_PROFILE_USAGE_USABLE_SESSION_PCT`, else 95.

use super::usage_cmd::sanitize_display;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::Path;

/// The weights file, inside `<root>/.auto-switch.d`.
pub(super) const FILE: &str = "capacity-weights.json";
/// The variable that sets the usable-now threshold.
pub(super) const ENV: &str = "AMPLIHACK_PROFILE_USAGE_USABLE_SESSION_PCT";
/// The usable-now threshold when neither the flag nor the variable sets one.
pub(super) const DEFAULT_THRESHOLD: u8 = 95;
/// The largest weights file read.
#[cfg_attr(not(unix), allow(dead_code))]
const MAX_BYTES: u64 = 64 * 1024;
/// The largest weight accepted.
const MAX_WEIGHT: f64 = 1000.0;
/// Characters of a bad variable value shown in its warning.
const MAX_SHOWN_VALUE: usize = 32;
/// Characters of a profile name shown in a weight warning.
const MAX_SHOWN_NAME: usize = 64;
const STATE_DIR: &str = ".auto-switch.d";

/// Per-profile capacity weights; a profile not listed weighs 1.0.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Weights(BTreeMap<String, f64>);

impl Weights {
    /// Weights from name and value pairs, unchecked.
    #[cfg(test)]
    pub(super) fn from_pairs(pairs: &[(&str, f64)]) -> Self {
        Self(pairs.iter().map(|(n, w)| (n.to_string(), *w)).collect())
    }

    /// The weight of `name`: its value from the file, else 1.0.
    pub(super) fn of(&self, name: &str) -> f64 {
        self.0.get(name).copied().unwrap_or(1.0)
    }

    /// Whether any weight is other than 1.0.
    pub(super) fn any_differs(&self) -> bool {
        self.0.values().any(|w| *w != 1.0)
    }
}

fn file_warning(reason: &str) -> String {
    format!("warning: {FILE} ignored ({reason}); every weight is 1.0")
}

fn value_warning(name: &str) -> String {
    let shown: String = sanitize_display(name, false)
        .chars()
        .take(MAX_SHOWN_NAME)
        .collect();
    format!(
        "warning: {FILE}: weight for \"{shown}\" ignored \
         (use a number above 0 and at most 1000); using 1.0"
    )
}

/// Parses the body of the weights file. Never echoes the body: warnings
/// carry fixed reasons and cleaned, cut profile names only.
pub(super) fn parse(bytes: &[u8]) -> (Weights, Vec<String>) {
    let value: serde_json::Value = match serde_json::from_slice(bytes) {
        Ok(v) => v,
        Err(_) => return (Weights::default(), vec![file_warning("invalid JSON")]),
    };
    let Some(map) = value.as_object() else {
        return (Weights::default(), vec![file_warning("not a JSON object")]);
    };
    let mut weights = BTreeMap::new();
    let mut warnings = Vec::new();
    for (name, v) in map {
        match v
            .as_f64()
            .filter(|w| w.is_finite() && *w > 0.0 && *w <= MAX_WEIGHT)
        {
            Some(w) => {
                weights.insert(name.clone(), w);
            }
            None => warnings.push(value_warning(name)),
        }
    }
    (Weights(weights), warnings)
}

/// Reads `<root>/.auto-switch.d/capacity-weights.json`. No file, or no state
/// directory, is silent. A symlink, a non-regular file, a file over 64 KiB,
/// one owned by another user or writable by group or others is refused with
/// one warning. Never creates or writes anything.
#[cfg(unix)]
pub(super) fn load(root: &Path) -> (Weights, Vec<String>) {
    use amplihack_utils::secure_files::{owned_by_euid, read_bounded_nofollow};
    use std::os::unix::fs::MetadataExt;

    let path = root.join(STATE_DIR).join(FILE);
    let refused = |reason: &str| (Weights::default(), vec![file_warning(reason)]);
    let meta = match std::fs::symlink_metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Weights::default(), vec![]),
        Err(_) => return refused("unreadable"),
    };
    if !meta.is_file() {
        return refused("unsafe file");
    }
    if meta.len() > MAX_BYTES {
        return refused("too large");
    }
    // The checks above are repeated on the opened file, which cannot be
    // swapped for a symlink in between: the open does not follow one.
    let (bytes, meta) = match read_bounded_nofollow(&path, MAX_BYTES) {
        Ok(Some(read)) => read,
        Ok(None) => return (Weights::default(), vec![]),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => return refused("unsafe file"),
        Err(_) => return refused("unreadable"),
    };
    if !owned_by_euid(&meta) || meta.mode() & 0o022 != 0 {
        return refused("unsafe file");
    }
    parse(&bytes)
}

/// Without unix ownership checks the file is not read: every weight is 1.0.
#[cfg(not(unix))]
pub(super) fn load(_root: &Path) -> (Weights, Vec<String>) {
    let _ = STATE_DIR;
    (Weights::default(), vec![])
}

/// The usable-now threshold and at most one warning: the flag (checked by
/// clap to 1-100), else the variable when it is a whole number from 1 to
/// 100 (empty counts as unset; anything else warns), else 95.
pub(super) fn usable_threshold(flag: Option<u8>, env: Option<&OsStr>) -> (u8, Option<String>) {
    if let Some(pct) = flag {
        return (pct, None);
    }
    let Some(raw) = env.filter(|v| !v.is_empty()) else {
        return (DEFAULT_THRESHOLD, None);
    };
    let parsed = raw
        .to_str()
        .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| s.parse::<u8>().ok())
        .filter(|p| (1..=100).contains(p));
    match parsed {
        Some(pct) => (pct, None),
        None => {
            let shown: String = sanitize_display(&raw.to_string_lossy(), false)
                .chars()
                .take(MAX_SHOWN_VALUE)
                .collect();
            (
                DEFAULT_THRESHOLD,
                Some(format!(
                    "warning: ignored {ENV} \"{shown}\" (use 1 to 100); using {DEFAULT_THRESHOLD}"
                )),
            )
        }
    }
}
