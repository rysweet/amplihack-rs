//! Issue #2279 (C10, D9): every settings file that carries amplihack's
//! hooks, one line each: `~/.claude/settings.json`, each profile's
//! `settings.json` (sorted by name) and the launch `--settings` file.
//!
//! Read-only. Lines hold the path and a state built from event names, fixed
//! phrases and JSON positions, never file content (SEC-WS2-04).

use crate::commands::install::hook_block::{FileDrift, HookTarget, drift_file};
use crate::commands::profile::ProfileStore;
use std::path::{Path, PathBuf};

/// One file's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileReport {
    pub path: PathBuf,
    pub state: FileDrift,
}

/// The files to check, in order: `home`'s `.claude/settings.json`, each
/// profile under `profile_root` by name, then `launch_file`.
pub(crate) fn report(
    home: &Path,
    profile_root: Option<&Path>,
    launch_file: Option<&Path>,
    target: &HookTarget,
) -> Vec<FileReport> {
    let mut paths = vec![home.join(".claude").join("settings.json")];
    if let Some(root) = profile_root {
        // `list` returns real directories directly inside the root, sorted.
        let names = ProfileStore::new(root.to_path_buf())
            .list()
            .unwrap_or_default();
        paths.extend(names.iter().map(|n| root.join(n).join("settings.json")));
    }
    paths.extend(launch_file.map(Path::to_path_buf));
    paths
        .into_iter()
        .map(|path| {
            let state = drift_file(&path, target);
            FileReport { path, state }
        })
        .collect()
}

/// One line per file, and whether the check passes: the home file must be
/// clean; a profile or launch file must be clean or absent.
pub(crate) fn render(reports: &[FileReport]) -> (bool, String) {
    let mut ok = true;
    let mut text = String::new();
    for (index, r) in reports.iter().enumerate() {
        let state = match &r.state {
            FileDrift::Clean => "clean".to_string(),
            FileDrift::Absent => "absent".to_string(),
            FileDrift::Unparseable(reason) => format!("unparseable ({reason})"),
            FileDrift::Refused(reason) => format!("refused ({reason})"),
            FileDrift::Drift(d) => format!("drift {}", d.summary()),
        };
        let passes = match r.state {
            FileDrift::Clean => true,
            FileDrift::Absent => index > 0,
            _ => false,
        };
        ok &= passes;
        text.push_str(&format!("{}: {state}\n", r.path.display()));
    }
    (ok, text)
}
