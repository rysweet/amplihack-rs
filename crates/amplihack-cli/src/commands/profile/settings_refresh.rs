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
//! now refuses a hooks binary that does not exist; any hooks path when HOME
//! is not the account's home (from the passwd database) and the profile root
//! is outside HOME, wherever the binary lies; and one under a temporary
//! directory unless the profile root is under HOME and itself under a
//! temporary directory.

use super::name::ProfileName;
use super::store::ProfileStore;
use crate::commands::install::hook_block::{FileMerge, update_file};
use amplihack_utils::secure_files::{passwd_home, temp_bases};
use serde_json::Value;
use std::path::{Path, PathBuf};

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
    let account_home = passwd_home();
    match hook_path_refusal(
        hooks_bin,
        store.root(),
        home.as_deref(),
        account_home.as_deref(),
        &temp_bases(),
    ) {
        Some(reason) => refused_all(&store, &reason),
        None => refresh_in(&store, update),
    }
}

/// [`hook_path_refusal`] for this process: HOME from the environment, the
/// account's home from the passwd database and the host's temp bases.
pub(crate) fn hook_path_refusal_here(hooks_bin: &Path, root: &Path) -> Option<String> {
    hook_path_refusal(
        hooks_bin,
        root,
        current_home().as_deref(),
        passwd_home().as_deref(),
        &temp_bases(),
    )
}

/// Issue #1699: `None` when `hooks_bin` may be written into the profiles
/// under `root`, otherwise the reason it may not. Refused when `hooks_bin`
/// is not a regular file (after following symlinks); when `home` is a
/// throwaway HOME, that is it differs from `account_home` (the passwd home),
/// and `root` lies outside it, wherever `hooks_bin` lies, because a HOME
/// under `~/.amplihack/work` is as disposable as one under `/tmp`; or when
/// `hooks_bin` lies under
/// one of `temp_bases` (expected canonical) unless `root` lies both under
/// `home` and under one of `temp_bases`, which is the `isolated(..)` test
/// layout. An unknown `home` refuses a temp hooks path; an unknown
/// `account_home` skips the throwaway-HOME check. "Lies under" is
/// checked twice: on the fully resolved path, and on the path as written with
/// only its directory resolved, because the path as written is what the
/// profiles store.
/// Comparisons are component-wise on canonical paths, or on the absolute form
/// of a path that does not exist.
pub(crate) fn hook_path_refusal(
    hooks_bin: &Path,
    root: &Path,
    home: Option<&Path>,
    account_home: Option<&Path>,
    temp_bases: &[PathBuf],
) -> Option<String> {
    let shown = hooks_bin.display();
    if !std::fs::metadata(hooks_bin).is_ok_and(|meta| meta.is_file()) {
        return Some(format!(
            "hooks binary {shown} does not exist or is not a regular file"
        ));
    }
    if let Some(reason) = throwaway_home_refusal(root, home, account_home) {
        return Some(reason);
    }
    let bin = match hooks_bin.canonicalize() {
        Ok(path) => path,
        Err(error) => return Some(format!("hooks binary {shown} cannot be resolved: {error}")),
    };
    let written = written_form(hooks_bin);
    let base = temp_bases.iter().find(|base| {
        bin.starts_with(base) || written.as_ref().is_some_and(|path| path.starts_with(base))
    })?;
    let under_temp = format!(
        "hooks binary {shown} is under temporary directory {}",
        base.display()
    );
    let Some(home) = home.and_then(resolved) else {
        return Some(format!("{under_temp}; HOME is unset"));
    };
    let Some(root_path) = resolved(root) else {
        return Some(format!(
            "{under_temp}; profile root {} cannot be resolved",
            root.display()
        ));
    };
    if !root_path.starts_with(&home) {
        return Some(format!(
            "{under_temp}; profile root {} is outside HOME",
            root.display()
        ));
    }
    // HOME may be the real one, with the hooks binary taken from a temporary
    // build (install falls back to the source binary when deploying fails).
    // Only a profile root that is itself temporary may receive a temp path.
    if !temp_bases.iter().any(|base| root_path.starts_with(base)) {
        return Some(format!(
            "{under_temp}; profile root {} is not under a temporary directory",
            root.display()
        ));
    }
    None
}

/// Issue #1699 review: `Some` when `home` is not `account_home` and `root`
/// lies outside `home`. Such a HOME is a scratch HOME (a test, or a
/// `HOME=... amplihack install` run) and the profiles under `root` belong to
/// the real account, so any hooks path it would write points into a
/// directory that can be removed.
fn throwaway_home_refusal(
    root: &Path,
    home: Option<&Path>,
    account_home: Option<&Path>,
) -> Option<String> {
    let home_path = resolved(home?)?;
    let account = resolved(account_home?)?;
    if home_path == account {
        return None;
    }
    let root_path = resolved(root)?;
    if root_path.starts_with(&home_path) {
        return None;
    }
    Some(format!(
        "HOME {} is not the account's home {}; profile root {} is outside HOME",
        home_path.display(),
        account.display(),
        root.display()
    ))
}

/// `path` with its directory resolved and its last component kept as written,
/// so a symlink placed in a temporary directory is judged by where it sits.
fn written_form(path: &Path) -> Option<PathBuf> {
    let absolute = std::path::absolute(path).ok()?;
    let name = absolute.file_name()?;
    Some(absolute.parent()?.canonicalize().ok()?.join(name))
}

/// The canonical form of `path`, or its absolute form when it does not exist.
fn resolved(path: &Path) -> Option<PathBuf> {
    path.canonicalize()
        .ok()
        .or_else(|| std::path::absolute(path).ok())
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
    each_profile(
        store,
        &|dir| match std::fs::symlink_metadata(dir.join("settings.json")) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Refresh::Missing,
            _ => Refresh::Skipped(reason.to_string()),
        },
    )
}

/// Unchecked: it does not consult [`hook_path_refusal`]. Production code
/// calls [`refresh_profile_settings`]; tests call this with a fixture store.
pub(super) fn refresh_in(
    store: &ProfileStore,
    update: &dyn Fn(&mut Value),
) -> Vec<(String, Refresh)> {
    each_profile(store, &|dir| refresh_one(dir, update))
}

/// `act` applied to each listed profile's directory; a profile whose name
/// does not parse is left out, and one that does not resolve is `Skipped`.
fn each_profile(store: &ProfileStore, act: &dyn Fn(&Path) -> Refresh) -> Vec<(String, Refresh)> {
    let Ok(names) = store.list() else {
        return Vec::new();
    };
    names
        .into_iter()
        .filter_map(|name| {
            let parsed = ProfileName::parse(&name).ok()?;
            let result = match store.resolve(&parsed) {
                Ok(dir) => act(&dir),
                Err(error) => Refresh::Skipped(error.to_string()),
            };
            Some((name, result))
        })
        .collect()
}

/// Issue #2279 (C6): one profile's file through `hook_block::update_file`,
/// which refuses symlinks, non-regular, hard-linked, foreign-owned and
/// oversized files, never creates a file, and writes (0600) only on change.
fn refresh_one(dir: &Path, update: &dyn Fn(&mut Value)) -> Refresh {
    match update_file(&dir.join("settings.json"), update) {
        FileMerge::Written => Refresh::Updated,
        FileMerge::AlreadyCurrent => Refresh::Unchanged,
        FileMerge::Absent => Refresh::Missing,
        FileMerge::Unparseable(reason) | FileMerge::Refused(reason) => Refresh::Skipped(reason),
    }
}
