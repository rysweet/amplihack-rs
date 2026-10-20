//! One session store for every profile (issue #1634): each profile's
//! `projects/`, `file-history/` and `session-env/` is an absolute link to the
//! same name in `~/.claude`, so `claude --resume <id>` works from any
//! profile. Logins are top-level files of the profile directory and are never
//! touched. A profile created before this change has its real directories
//! merged into the shared store first. Never fails its caller: every problem
//! is a warning on stderr. See docs/reference/profile-command.md, "Shared
//! session directories", whose message table is the wording used here.

#[cfg(unix)]
mod copy;
#[cfg(unix)]
mod link;
#[cfg(unix)]
mod merge;
#[cfg(all(test, unix))]
mod tests_source_scan;

use super::name::ProfileName;
use super::store::ProfileStore;
use std::path::{Path, PathBuf};

pub(super) const SHARED_SESSION_DIRS: [&str; 3] = ["projects", "file-history", "session-env"];
/// When present in a profile (any file type), nothing is linked or merged.
const OPT_OUT: &str = ".no-shared-sessions";
const UNSUPPORTED: &str =
    "shared session dirs not supported on this platform; profiles keep separate projects/";

/// Why a real directory was not merged on this call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Deferred {
    LiveSession,
    LivenessUnknown,
    Raced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Migrate,
    LinkOnly(Deferred),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Migrated {
    pub name: &'static str,
    pub moved: usize,
    pub identical: usize,
    pub conflicts: usize,
    pub backup: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Conflict {
    pub name: &'static str,
    /// Relative to the shared directory.
    pub path: PathBuf,
    /// Absolute.
    pub saved_as: PathBuf,
}

#[derive(Debug, Default)]
pub(super) struct Report {
    pub linked: Vec<&'static str>,
    pub already_ok: Vec<&'static str>,
    pub migrated: Vec<Migrated>,
    pub deferred: Vec<(&'static str, Deferred)>,
    pub conflicts: Vec<Conflict>,
    pub warnings: Vec<String>,
    pub unsupported: bool,
    /// Message lines in order, without the prefix; `true` when home-relative
    /// shortening applies.
    notes: Vec<(String, bool)>,
}

impl Report {
    /// Nothing changed and nothing needs saying.
    pub(super) fn is_noop(&self) -> bool {
        self.linked.is_empty()
            && self.migrated.is_empty()
            && self.deferred.is_empty()
            && self.notes.is_empty()
            && !self.unsupported
    }

    /// The stderr lines, prefix included, paths under `home` shown as `~`.
    #[cfg(test)]
    pub(super) fn render(&self, profile: &ProfileName, home: Option<&Path>) -> Vec<String> {
        self.lines(profile, home, true)
    }

    fn lines(&self, profile: &ProfileName, home: Option<&Path>, linked: bool) -> Vec<String> {
        let prefix = format!("amplihack: profile {profile}: ");
        let mut out = Vec::new();
        if linked && !self.linked.is_empty() {
            out.push(format!("{prefix}linked {}", self.linked.join(", ")));
        }
        for (text, shorten) in &self.notes {
            match home.filter(|_| *shorten) {
                Some(home) => out.push(format!("{prefix}{}", shorten_home(text, home))),
                None => out.push(format!("{prefix}{text}")),
            }
        }
        out
    }

    fn note(&mut self, text: String) {
        self.notes.push((text, true));
    }

    fn warn(&mut self, text: String) {
        self.warn_with(text, true);
    }

    fn warn_with(&mut self, text: String, shorten: bool) {
        self.warnings.push(text.clone());
        self.notes.push((text, shorten));
    }

    fn defer(&mut self, name: &'static str, why: Deferred) {
        self.deferred.push((name, why));
        self.note(match why {
            Deferred::LiveSession => format!("{name}: migration deferred: live session"),
            Deferred::LivenessUnknown => format!(
                "{name}: not merged: running sessions can't be checked on this system; merge by hand, see docs/howto/claude-profiles.md#merge-by-hand-on-macos"
            ),
            Deferred::Raced => format!(
                "{name}: link not created: the name appeared during the check; retried next launch"
            ),
        });
    }
}

fn shorten_home(text: &str, home: &Path) -> String {
    let home = home.display().to_string();
    if home.is_empty() || home == "/" {
        return text.to_string();
    }
    text.replace(&format!("{home}/"), "~/")
}

/// Links (and, in `Mode::Migrate`, merges) the shared session names of the
/// profile at `profile_dir`. `mode` is called at most once, and only when a
/// name is a real directory.
pub(super) fn ensure_shared_session_dirs(
    profile_dir: &Path,
    profile: &ProfileName,
    canonical_root: &Path,
    mode: impl FnOnce() -> Mode,
) -> Report {
    let mut r = Report::default();
    let marker = profile_dir.join(OPT_OUT);
    match std::fs::symlink_metadata(&marker) {
        Ok(_) => {
            tracing::debug!("profile {profile}: {OPT_OUT} present; sessions not shared");
            return r;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            r.warn(format!("{}: {e}; sessions not shared", marker.display()));
            return r;
        }
    }
    #[cfg(unix)]
    unix::ensure(profile_dir, canonical_root, profile, mode, &mut r);
    #[cfg(not(unix))]
    {
        let _ = (canonical_root, mode);
        r.unsupported = true;
    }
    r
}

/// The hook for switch, login, launch and rotate: picks the mode from the
/// liveness scan and prints the report to stderr. Never fails.
pub(super) fn ensure_for(store: &ProfileStore, name: &ProfileName) {
    let Some(canonical_root) = store.shared_assets() else {
        tracing::debug!("profile {name}: no HOME; sessions not shared");
        return;
    };
    let dir = store.root().join(name.as_str());
    let report =
        ensure_shared_session_dirs(&dir, name, canonical_root, || live_mode(store.root(), name));
    print(&report, name, canonical_root, false);
}

/// The hook in `ProfileStore::add`: a new directory, so nothing to migrate.
/// The `linked` line is not printed.
pub(super) fn after_add(dir: &Path, name: &ProfileName, canonical_root: &Path) {
    let report = ensure_shared_session_dirs(dir, name, canonical_root, || Mode::Migrate);
    print(&report, name, canonical_root, true);
}

fn print(r: &Report, name: &ProfileName, canonical_root: &Path, add: bool) {
    if r.is_noop() {
        return;
    }
    // `~` only when the canonical root is `$HOME/.claude`.
    let home = canonical_root
        .parent()
        .filter(|_| canonical_root.file_name() == Some(std::ffi::OsStr::new(".claude")));
    for line in r.lines(name, home, !add) {
        eprintln!("{line}");
    }
    if r.unsupported {
        if add {
            eprintln!("amplihack: profile {name}: {UNSUPPORTED}");
        } else {
            tracing::debug!("profile {name}: {UNSUPPORTED}");
        }
    }
}

#[cfg(unix)]
fn live_mode(root: &Path, name: &ProfileName) -> Mode {
    mode_from_liveness(&super::auto_switch::kit::scan_system(root), name)
}

#[cfg(not(unix))]
fn live_mode(_root: &Path, _name: &ProfileName) -> Mode {
    Mode::LinkOnly(Deferred::LivenessUnknown)
}

/// In doubt, nothing is migrated.
#[cfg(unix)]
pub(super) fn mode_from_liveness(
    live: &super::auto_switch::kit::Liveness,
    name: &ProfileName,
) -> Mode {
    use super::auto_switch::kit::Liveness;
    match live {
        Liveness::Known(set) if !set.contains(name.as_str()) => Mode::Migrate,
        Liveness::Known(_) | Liveness::Everything => Mode::LinkOnly(Deferred::LiveSession),
        // On Linux `Unsupported` means the /proc scan failed: retried next call.
        Liveness::Unsupported if cfg!(target_os = "linux") => {
            tracing::debug!("profile {name}: liveness scan failed; migration deferred");
            Mode::LinkOnly(Deferred::LiveSession)
        }
        Liveness::Unsupported => Mode::LinkOnly(Deferred::LivenessUnknown),
    }
}

#[cfg(unix)]
mod unix;

#[cfg(test)]
pub(super) mod test_hook {
    use std::cell::Cell;

    thread_local! {
        static FORCE_EXDEV: Cell<bool> = const { Cell::new(false) };
        static FAIL_LINK_RENAME: Cell<bool> = const { Cell::new(false) };
    }

    /// Treats every move into the shared store as cross-filesystem.
    pub fn set_force_exdev(on: bool) {
        FORCE_EXDEV.with(|c| c.set(on));
    }

    /// Fails the second rename of the two-rename link swap.
    pub fn set_fail_link_rename(on: bool) {
        FAIL_LINK_RENAME.with(|c| c.set(on));
    }

    pub(super) fn force_exdev() -> bool {
        FORCE_EXDEV.with(Cell::get)
    }

    pub(super) fn fail_link_rename() -> bool {
        FAIL_LINK_RENAME.with(Cell::get)
    }
}
