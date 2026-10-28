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
//!
//! Issue #1699: a test running `amplihack install` under a temp HOME, with
//! the developer's `AMPLIHACK_CLAUDE_PROFILE_ROOT` inherited, rewrote the real
//! profiles with a hooks path inside that temp HOME. [`hook_path_refusal`]
//! now refuses a hooks binary that does not exist, and one under a temporary
//! directory when the profile root is not under HOME.

use super::name::ProfileName;
use super::store::ProfileStore;
use serde_json::Value;
use std::path::{Path, PathBuf};

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

/// Applies `update`, which registers `hooks_bin`, to the `settings.json` of
/// every profile under the configured root. Returns each profile's name and
/// result; empty when no root is configured. When [`hook_path_refusal`]
/// refuses `hooks_bin`, no file is opened and every profile with a
/// `settings.json` is `Skipped` with the reason.
pub(crate) fn refresh_profile_settings(
    hooks_bin: &Path,
    update: &dyn Fn(&mut Value),
) -> Vec<(String, Refresh)> {
    let Ok(store) = ProfileStore::from_env() else {
        return Vec::new();
    };
    let home = current_home();
    match hook_path_refusal(
        hooks_bin,
        store.root(),
        home.as_deref(),
        &production_temp_bases(),
    ) {
        Some(reason) => refused_all(&store, &reason),
        None => refresh_in(&store, update),
    }
}

/// Issue #1699: `None` when `hooks_bin` may be written into the profiles
/// under `root`, otherwise the reason it may not. Refused when `hooks_bin`
/// is not a regular file (after following symlinks), or when its canonical
/// path lies under one of `temp_bases` (expected canonical) and `root` does
/// not lie under `home`, or `home` is unknown. Comparisons are component-wise
/// on canonical paths, or on the absolute form of a path that does not exist.
pub(crate) fn hook_path_refusal(
    hooks_bin: &Path,
    root: &Path,
    home: Option<&Path>,
    temp_bases: &[PathBuf],
) -> Option<String> {
    let shown = hooks_bin.display();
    if !std::fs::metadata(hooks_bin).is_ok_and(|meta| meta.is_file()) {
        return Some(format!("hooks binary {shown} does not exist"));
    }
    let bin = match hooks_bin.canonicalize() {
        Ok(path) => path,
        Err(error) => return Some(format!("hooks binary {shown} cannot be resolved: {error}")),
    };
    let base = temp_bases.iter().find(|base| bin.starts_with(base))?;
    let under_temp = format!(
        "hooks binary {shown} is under temporary directory {}",
        base.display()
    );
    let Some(home) = home.and_then(resolved) else {
        return Some(format!("{under_temp}; HOME is unset"));
    };
    match resolved(root) {
        Some(root_path) if root_path.starts_with(&home) => None,
        _ => Some(format!(
            "{under_temp}; profile root {} is outside HOME",
            root.display()
        )),
    }
}

/// The canonical form of `path`, or its absolute form when it does not exist.
fn resolved(path: &Path) -> Option<PathBuf> {
    path.canonicalize()
        .ok()
        .or_else(|| std::path::absolute(path).ok())
}

/// The temporary directories a hooks binary must not come from: the canonical
/// forms of `temp_dir()`, an absolute `$TMPDIR` and `/tmp`, without
/// duplicates. A base that cannot be canonicalised is left out.
fn production_temp_bases() -> Vec<PathBuf> {
    let tmpdir = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());
    let mut bases: Vec<PathBuf> = Vec::new();
    for candidate in [
        Some(std::env::temp_dir()),
        tmpdir,
        Some(PathBuf::from("/tmp")),
    ]
    .into_iter()
    .flatten()
    {
        if let Ok(base) = candidate.canonicalize()
            && !bases.contains(&base)
        {
            bases.push(base);
        }
    }
    bases
}

/// `HOME` from the process environment; unset or empty is `None`.
fn current_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// Every profile's result when the hooks path is refused. Only stats
/// `settings.json`; nothing is opened.
fn refused_all(store: &ProfileStore, reason: &str) -> Vec<(String, Refresh)> {
    let Ok(names) = store.list() else {
        return Vec::new();
    };
    names
        .into_iter()
        .filter_map(|name| {
            let parsed = ProfileName::parse(&name).ok()?;
            let result = match store.resolve(&parsed) {
                Ok(dir) => match std::fs::symlink_metadata(dir.join("settings.json")) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Refresh::Missing,
                    _ => Refresh::Skipped(reason.to_string()),
                },
                Err(error) => Refresh::Skipped(error.to_string()),
            };
            Some((name, result))
        })
        .collect()
}

/// Unchecked: it does not consult [`hook_path_refusal`]. Production code
/// calls [`refresh_profile_settings`]; tests call this with a fixture store.
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
fn refresh_one(dir: &Path, update: &dyn Fn(&mut Value)) -> Refresh {
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
fn refresh_one(_dir: &Path, _update: &dyn Fn(&mut Value)) -> Refresh {
    Refresh::Skipped("profile settings refresh is Unix-only".into())
}
