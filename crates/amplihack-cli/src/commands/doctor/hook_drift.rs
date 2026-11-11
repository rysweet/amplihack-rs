//! Issue #2279 (C10, D9): every settings file that carries amplihack's
//! hooks, one line each: `~/.claude/settings.json`, each profile's
//! `settings.json` (sorted by name) and the launch `--settings` file, plus
//! one line for each of those sources that cannot be located or listed.
//!
//! Read-only. Lines hold the path and a state built from event names, fixed
//! phrases and JSON positions, never file content (SEC-WS2-04).

use crate::commands::install::hook_block::{FileDrift, HookTarget, drift_file, reason};
use crate::commands::profile::{ProfileError, ProfileStore};
use std::path::{Path, PathBuf};

/// One file's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileReport {
    pub path: PathBuf,
    pub state: FileDrift,
}

/// Where a group of files is, or why it is not known. A source that is not
/// known is one failing row, never a file quietly left out.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Source<'a> {
    At(&'a Path),
    /// Could not be resolved; the reason is an error kind or fixed phrase.
    Unknown(&'a str),
}

/// A source that could not be resolved or listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Unlisted {
    pub source: &'static str,
    pub reason: String,
}

/// Every file checked, then every source that could not be listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Report {
    pub files: Vec<FileReport>,
    pub unlisted: Vec<Unlisted>,
    /// The home file is absent and the repo-local file is clean: a
    /// repo-local install (#1088), so the home row passes.
    pub repo_local_clean: bool,
}

/// The files to check, in order: `home`'s `.claude/settings.json`, each
/// profile under `profiles` by name, then `launch_file`. `repo_local`, the
/// current repository's `.claude/settings.json`, is read only when the home
/// file is absent, and gets no row of its own.
pub(crate) fn report(
    home: &Path,
    repo_local: Option<&Path>,
    profiles: Source<'_>,
    launch_file: Source<'_>,
    target: &HookTarget,
) -> Report {
    let mut paths = vec![home.join(".claude").join("settings.json")];
    let mut unlisted = Vec::new();
    match profiles {
        Source::Unknown(why) => unlisted.push(Unlisted {
            source: "profiles",
            reason: why.to_string(),
        }),
        // `list` returns real directories directly inside the root, sorted.
        Source::At(root) => match ProfileStore::new(root.to_path_buf()).list() {
            Ok(names) => paths.extend(names.iter().map(|n| root.join(n).join("settings.json"))),
            Err(error) => unlisted.push(Unlisted {
                source: "profiles",
                reason: profile_error_reason(&error),
            }),
        },
    }
    match launch_file {
        Source::At(path) => paths.push(path.to_path_buf()),
        Source::Unknown(why) => unlisted.push(Unlisted {
            source: "launch file",
            reason: why.to_string(),
        }),
    }
    let files: Vec<FileReport> = paths
        .into_iter()
        .map(|path| {
            let state = drift_file(&path, target);
            FileReport { path, state }
        })
        .collect();
    let repo_local_clean = files[0].state == FileDrift::Absent
        && repo_local.is_some_and(|path| drift_file(path, target) == FileDrift::Clean);
    Report {
        files,
        unlisted,
        repo_local_clean,
    }
}

/// A profile-store error as a reason: the I/O error kind, or the error's
/// own message, which names paths and never file content.
pub(crate) fn profile_error_reason(error: &ProfileError) -> String {
    match error {
        ProfileError::Io(io) => reason(io),
        other => other.to_string(),
    }
}

/// One line per file and per unlisted source, and whether the check passes:
/// the home file must be clean, or absent beside a clean repo-local file; a
/// profile or launch file must be clean or absent; every source must be
/// listed.
pub(crate) fn render(report: &Report) -> (bool, String) {
    let mut ok = report.unlisted.is_empty();
    let mut text = String::new();
    for (index, r) in report.files.iter().enumerate() {
        let home_row = index == 0;
        let state = match &r.state {
            FileDrift::Clean => "clean".to_string(),
            FileDrift::Absent if home_row && report.repo_local_clean => {
                "absent; repo-local install clean".to_string()
            }
            FileDrift::Absent => "absent".to_string(),
            FileDrift::Unparseable(reason) => format!("unparseable ({reason})"),
            FileDrift::Refused(reason) => {
                format!("refused ({reason}); replace it with a regular file you own")
            }
            FileDrift::Drift(d) => format!("drift {}", d.summary()),
        };
        let passes = match r.state {
            FileDrift::Clean => true,
            FileDrift::Absent => !home_row || report.repo_local_clean,
            _ => false,
        };
        ok &= passes;
        text.push_str(&format!("{}: {state}\n", r.path.display()));
    }
    for u in &report.unlisted {
        text.push_str(&format!("{}: cannot list ({})\n", u.source, u.reason));
    }
    (ok, text)
}
