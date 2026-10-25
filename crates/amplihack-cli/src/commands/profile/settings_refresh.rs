//! Issue #1661: a profile's `settings.json` is a filtered copy of
//! `~/.claude/settings.json` made once, when the profile is added, so hooks a
//! later `amplihack install` registers never reached sessions started with
//! that profile. `amplihack install` now applies its hook update to every
//! profile's existing `settings.json` too.
//!
//! Only real profile directories directly inside the root are visited, the
//! file must be a regular file owned by the user (never a symlink), and it is
//! replaced atomically with mode 0600. A missing file is left missing, and a
//! file that cannot be read, parsed or written is skipped with a warning.

use super::name::ProfileName;
use super::store::ProfileStore;
use serde_json::Value;

/// Larger profile settings files are skipped, as when seeding.
const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;

/// One profile's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refresh {
    Updated,
    Unchanged,
    /// No `settings.json` in the profile.
    Missing,
    /// Not a regular private file, unreadable, not a JSON object, or the
    /// write failed; the reason is for the warning.
    Skipped(String),
}

/// Applies `update` to the `settings.json` of every profile under the
/// configured root. Returns each profile's name and result; empty when no
/// root is configured.
pub(crate) fn refresh_profile_settings(update: &dyn Fn(&mut Value)) -> Vec<(String, Refresh)> {
    match ProfileStore::from_env() {
        Ok(store) => refresh_in(&store, update),
        Err(_) => Vec::new(),
    }
}

pub(crate) fn refresh_in(
    store: &ProfileStore,
    update: &dyn Fn(&mut Value),
) -> Vec<(String, Refresh)> {
    let Ok(names) = store.list() else {
        return Vec::new();
    };
    names
        .into_iter()
        .filter_map(|name| {
            let parsed = ProfileName::parse(&name).ok()?;
            let result = match store.resolve(&parsed) {
                Ok(dir) => refresh_one(&dir, update),
                Err(error) => Refresh::Skipped(error.to_string()),
            };
            Some((name, result))
        })
        .collect()
}

#[cfg(unix)]
fn refresh_one(dir: &std::path::Path, update: &dyn Fn(&mut Value)) -> Refresh {
    use amplihack_utils::secure_files::{read_bounded_nofollow, write_private_atomic};
    use std::os::unix::fs::MetadataExt;

    let path = dir.join("settings.json");
    let (raw, meta) = match read_bounded_nofollow(&path, MAX_SETTINGS_BYTES) {
        Ok(Some(found)) => found,
        Ok(None) => return Refresh::Missing,
        Err(error) => return Refresh::Skipped(error.to_string()),
    };
    // SAFETY: geteuid has no preconditions and cannot fail.
    if meta.uid() != unsafe { libc::geteuid() } {
        return Refresh::Skipped("not owned by the current user".into());
    }
    let mut settings: Value = match serde_json::from_slice(&raw) {
        Ok(value @ Value::Object(_)) => value,
        Ok(_) => return Refresh::Skipped("not a JSON object".into()),
        Err(error) => return Refresh::Skipped(format!("invalid JSON: {error}")),
    };
    let before = settings.clone();
    update(&mut settings);
    if settings == before {
        return Refresh::Unchanged;
    }
    let mut contents = match serde_json::to_vec_pretty(&settings) {
        Ok(bytes) => bytes,
        Err(error) => return Refresh::Skipped(error.to_string()),
    };
    contents.push(b'\n');
    match write_private_atomic(dir, "settings.json", &contents) {
        Ok(()) => Refresh::Updated,
        Err(error) => Refresh::Skipped(error.to_string()),
    }
}

#[cfg(not(unix))]
fn refresh_one(_dir: &std::path::Path, _update: &dyn Fn(&mut Value)) -> Refresh {
    Refresh::Skipped("profile settings refresh is Unix-only".into())
}
