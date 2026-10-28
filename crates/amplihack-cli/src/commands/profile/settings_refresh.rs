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
//! Issue #1699: a test that ran `amplihack install` with a temp HOME and an
//! inherited `AMPLIHACK_CLAUDE_PROFILE_ROOT` wrote the temp HOME's
//! `amplihack-hooks` path into the user's real profiles. Install's refresh
//! goes through [`refresh_profile_hooks_checked`], which refuses, before any
//! profile file is opened, a hook binary that does not exist, or one under a
//! temp dir or HOME while the root is outside HOME.

use super::name::ProfileName;
use super::store::ProfileStore;
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

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

/// Install's refresh: applies `update` to the `settings.json` of every
/// profile under the configured root, behind [`hook_path_refusal`] for this
/// process's HOME and temp dirs. Returns each profile's name and result;
/// empty when no root is configured. Every profile is `Skipped` with the same
/// reason when the rule refuses.
pub(crate) fn refresh_profile_hooks_checked(
    hooks_bin: &Path,
    update: &dyn Fn(&mut Value),
) -> Vec<(String, Refresh)> {
    let Ok(store) = ProfileStore::from_env() else {
        return Vec::new();
    };
    let home = std::env::var_os("HOME").map(PathBuf::from);
    refresh_in_checked(
        &store,
        hooks_bin,
        home.as_deref(),
        &default_temp_dirs(),
        update,
    )
}

/// [`refresh_in`] when `hooks_bin` may be written into the profiles under
/// `store`'s root. Otherwise every profile `refresh_in` would visit gets
/// `Skipped(reason)`, and no profile file is opened, locked or written.
pub(crate) fn refresh_in_checked(
    store: &ProfileStore,
    hooks_bin: &Path,
    home: Option<&Path>,
    temp_dirs: &[PathBuf],
    update: &dyn Fn(&mut Value),
) -> Vec<(String, Refresh)> {
    let Some(reason) = hook_path_refusal(hooks_bin, store.root(), home, temp_dirs) else {
        return refresh_in(store, update);
    };
    let Ok(names) = store.list() else {
        return Vec::new();
    };
    names
        .into_iter()
        .filter(|name| ProfileName::parse(name).is_ok())
        .map(|name| (name, Refresh::Skipped(reason.clone())))
        .collect()
}

/// Why `hooks_bin` must not be written into the profiles under `root`, or
/// `None` when it may. Refused: an unusable HOME (unset, relative or `/`); a
/// relative binary; a binary that is not an existing file; and a binary under
/// a temp dir or HOME while the root is outside HOME. Paths are compared in
/// both their given and canonical forms, by whole components.
pub(crate) fn hook_path_refusal(
    hooks_bin: &Path,
    root: &Path,
    home: Option<&Path>,
    temp_dirs: &[PathBuf],
) -> Option<String> {
    let Some(home) = home.filter(|h| h.is_absolute() && norm(h) != Path::new("/")) else {
        return Some("cannot determine HOME; refusing to write hook paths".into());
    };
    let bin = shown(hooks_bin);
    if !hooks_bin.is_absolute() {
        return Some(format!("hook binary {bin} is not an absolute path"));
    }
    if !std::fs::metadata(hooks_bin).is_ok_and(|m| m.is_file()) {
        return Some(format!("hook binary {bin} does not exist or is not a file"));
    }
    let homes = forms(home);
    let root_inside_home = forms(root)
        .iter()
        .all(|r| homes.iter().any(|h| r.starts_with(h)));
    if root_inside_home {
        return None;
    }
    let bins = forms(hooks_bin);
    let under = |bases: &[PathBuf]| bins.iter().any(|b| bases.iter().any(|t| b.starts_with(t)));
    let (home_shown, root_shown) = (shown(home), shown(root));
    if let Some(temp) = temp_dirs.iter().find(|t| under(&forms(t))) {
        return Some(format!(
            "hook binary {bin} is under temp dir {} but profile root {root_shown} is outside HOME {home_shown}",
            shown(temp)
        ));
    }
    if under(&homes) {
        return Some(format!(
            "hook binary {bin} is under HOME {home_shown} but profile root {root_shown} is outside HOME"
        ));
    }
    None
}

/// The temp dirs a test HOME usually lives in: `std::env::temp_dir()`,
/// `$TMPDIR` and, on Unix, `/tmp`. Empty, relative and `/` values are
/// dropped; the rest are canonicalised and deduplicated.
pub(crate) fn default_temp_dirs() -> Vec<PathBuf> {
    let mut candidates = vec![std::env::temp_dir()];
    candidates.extend(std::env::var_os("TMPDIR").map(PathBuf::from));
    if cfg!(unix) {
        candidates.push(PathBuf::from("/tmp"));
    }
    temp_dirs_from(candidates)
}

/// The usable entries of `candidates`, canonicalised, in order, once each.
pub(crate) fn temp_dirs_from(candidates: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for dir in candidates {
        if !dir.is_absolute() {
            continue;
        }
        let dir = norm(&dir);
        if dir != Path::new("/") && !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

/// `path` as given (made lexical) and canonicalised.
fn forms(path: &Path) -> [PathBuf; 2] {
    [lexical(path), norm(path)]
}

/// `path` canonicalised; when it does not exist, its nearest existing
/// ancestor canonicalised with the rest appended.
fn norm(path: &Path) -> PathBuf {
    let path = lexical(path);
    let mut existing = path.as_path();
    let mut rest = Vec::new();
    loop {
        if let Ok(canonical) = std::fs::canonicalize(existing) {
            return rest.iter().rev().fold(canonical, |p, c| p.join(c));
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                existing = parent;
            }
            _ => return path,
        }
    }
}

/// `path` made absolute with `.` dropped; `..` is kept, never resolved
/// against the disk. Callers pass absolute paths; relative input is refused
/// upstream.
fn lexical(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    absolute
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect()
}

/// `path` for a warning, with control characters removed.
fn shown(path: &Path) -> String {
    path.display()
        .to_string()
        .chars()
        .filter(|c| !c.is_control())
        .collect()
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
