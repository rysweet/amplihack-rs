//! Issue #1551: auto-drive runs with a `gh` shim first on `PATH` that refuses
//! every merge not made by `autodrive_merge_gate.sh`.
//!
//! This module decides when the shim applies and finds it in the bundle. The
//! shim itself is `amplifier-bundle/tools/gh-merge-guard/gh`.

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

/// Set to `1` for a guarded run. Any non-empty inherited value keeps a nested
/// run guarded; no value switches the guard off.
pub(crate) const MERGE_GUARD_ENV: &str = "AMPLIHACK_MERGE_GUARD";

/// The shim's directory, relative to `AMPLIHACK_HOME`.
pub(crate) const GUARD_REL: &str = "amplifier-bundle/tools/gh-merge-guard";

/// Whether a run of the recipe `name`, read from `path`, gets the guard.
///
/// True for `auto-drive-to-merge` and any `autodrive-*` recipe, matched on
/// the `name:` or the file stem, and for any run that inherits a non-empty
/// [`MERGE_GUARD_ENV`].
pub(crate) fn guard_applies(name: &str, path: &Path, inherited: Option<&str>) -> bool {
    if inherited.is_some_and(|value| !value.is_empty()) {
        return true;
    }
    let is_auto_drive = |s: &str| s == "auto-drive-to-merge" || s.starts_with("autodrive-");
    is_auto_drive(name)
        || path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(is_auto_drive)
}

/// The guard directory under `home`, provided it holds an executable `gh`
/// file (symlinks followed). The directory is returned as given, never
/// canonicalised, so `PATH` de-duplication still recognises it.
///
/// There is no fallback location: a missing shim, or a `home` that is not an
/// absolute path (which would resolve against the working directory), stops
/// the run.
pub(crate) fn locate_guard_dir(home: &Path) -> Result<PathBuf> {
    let dir = home.join(GUARD_REL);
    if !home.is_absolute() || !is_executable_file(&dir.join("gh")) {
        bail!(
            "amplihack: merge guard not found at {}; refusing to run auto-drive without it (#1551)",
            dir.display()
        );
    }
    Ok(dir)
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file())
}
