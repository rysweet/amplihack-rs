//! How a usage probe starts `claude` (issue #1974).
//!
//! Every probe passes [`NO_PERSIST`], so `claude` writes no session log for
//! it. On Linux, when `bwrap` is usable, the probe also runs under
//!
//! ```text
//! bwrap --dev-bind / / --tmpfs <canonical projects> -- claude <args>
//! ```
//!
//! so `/usage` finds an empty `projects` folder and does not read every
//! shared transcript (about 10k files, 47 s per probe on a busy host).
//!
//! `bwrap` here only hides that one folder. `--dev-bind / /` gives the probe
//! the whole filesystem, read and write, with the same user id: this is
//! not containment and not a security boundary.

use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// The `claude` flag that stops it writing a session log.
pub(super) const NO_PERSIST: &str = "--no-session-persistence";

/// Whether probes can run under `bwrap`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ProbeSandbox {
    /// The absolute path of a `bwrap` that ran the check successfully.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))] // Linux callers only
    Bwrap(PathBuf),
    Unavailable(Unavailable),
}

/// Why there is no sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Unavailable {
    /// No `bwrap` on `PATH`, or not Linux.
    NotFound,
    /// `bwrap` failed to start or its check exited non-zero.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))] // Linux callers only
    Refused,
    /// Unit tests: the host's `bwrap` is never run, and nothing is noted.
    /// Only `detected()` under `cfg(test)` gives it.
    #[cfg_attr(not(test), allow(dead_code))]
    Test,
}

/// Why one probe runs without the sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Fallback {
    NotFound,
    Refused,
    ProjectsUnresolved,
}

impl Fallback {
    /// The reason, as noted. It names no path and no environment value.
    pub(super) fn reason(self) -> &'static str {
        match self {
            Self::NotFound => "bwrap not found",
            Self::Refused => "bwrap refused",
            Self::ProjectsUnresolved => "projects folder not resolved",
        }
    }
}

/// Writes the fallback note at most once.
pub(super) struct Notice(AtomicBool);

/// The process-wide notice used by real probes.
pub(super) static NOTICE: Notice = Notice::new();

impl Notice {
    pub(super) const fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    /// Whether the note has been written.
    #[cfg(test)]
    pub(super) fn emitted(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// Writes the note to stderr, the first time only.
    pub(super) fn note(&self, fallback: Fallback) {
        self.note_to(fallback, &mut std::io::stderr().lock());
    }

    /// Writes the note to `out`, the first time only.
    pub(super) fn note_to(&self, fallback: Fallback, out: &mut impl Write) {
        if self.0.swap(true, Ordering::Relaxed) {
            return;
        }
        // An unwritable stderr has nowhere else to report to; the probe
        // itself is unaffected.
        let _ = writeln!(
            out,
            "note: usage probes run claude with {NO_PERSIST} only: {}",
            fallback.reason()
        );
    }
}

/// The sandbox for this process, found the first time it is needed and
/// kept until the process ends (no interval, no timeout, no retry).
pub(super) fn detected() -> &'static ProbeSandbox {
    static DETECTED: OnceLock<ProbeSandbox> = OnceLock::new();
    DETECTED.get_or_init(detect)
}

#[cfg(test)]
fn detect() -> ProbeSandbox {
    ProbeSandbox::Unavailable(Unavailable::Test)
}

#[cfg(all(not(test), not(target_os = "linux")))]
fn detect() -> ProbeSandbox {
    ProbeSandbox::Unavailable(Unavailable::NotFound)
}

/// Finds `bwrap` on `PATH` and runs `bwrap --dev-bind / / -- true` once,
/// with no shell and null stdio. Exit status 0 means usable.
#[cfg(all(not(test), target_os = "linux"))]
fn detect() -> ProbeSandbox {
    use std::process::Stdio;
    let Some(bwrap) = find_on_path(std::env::var_os("PATH").as_deref()) else {
        return ProbeSandbox::Unavailable(Unavailable::NotFound);
    };
    let ok = Command::new(&bwrap)
        .args(["--dev-bind", "/", "/", "--", "true"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        ProbeSandbox::Bwrap(bwrap)
    } else {
        ProbeSandbox::Unavailable(Unavailable::Refused)
    }
}

/// The first regular, executable `bwrap` in an absolute `PATH` entry.
/// Empty and relative entries are skipped.
#[cfg(all(not(test), target_os = "linux"))]
fn find_on_path(path: Option<&OsStr>) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path?)
        .filter(|d| d.is_absolute())
        .map(|d| d.join("bwrap"))
        .find(|p| {
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// Rewrites `cmd` to run under `sandbox`, hiding the probe's resolved
/// `projects` folder. Returns `cmd` unchanged, with the reason, when it
/// cannot. Pure: creates nothing and never changes this process's
/// environment.
pub(super) fn wrap(
    cmd: Command,
    sandbox: &ProbeSandbox,
    home: &Path,
) -> (Command, Option<Fallback>) {
    let bwrap = match sandbox {
        ProbeSandbox::Bwrap(b) => b,
        ProbeSandbox::Unavailable(Unavailable::Test) => return (cmd, None),
        ProbeSandbox::Unavailable(Unavailable::NotFound) => {
            return (cmd, Some(Fallback::NotFound));
        }
        ProbeSandbox::Unavailable(Unavailable::Refused) => {
            return (cmd, Some(Fallback::Refused));
        }
    };
    let Some(hidden) = hidden_folder(&cmd, home) else {
        return (cmd, Some(Fallback::ProjectsUnresolved));
    };
    let mut w = Command::new(bwrap);
    // Only these flags. `--die-with-parent` and `--new-session` are left
    // out on purpose: `bwrap` leads the probe's process group, so the group
    // kill at the end of each attempt ends bwrap, claude and its children;
    // and the probe's stdio is pipes or /dev/null, never a terminal, so the
    // TIOCSTI risk `--new-session` guards against does not arise for this
    // trusted binary.
    w.arg("--dev-bind")
        .arg("/")
        .arg("/")
        .arg("--tmpfs")
        .arg(&hidden)
        .arg("--")
        .arg(cmd.get_program())
        .args(cmd.get_args());
    for (key, value) in cmd.get_envs() {
        match value {
            Some(v) => w.env(key, v),
            None => w.env_remove(key),
        };
    }
    if let Some(dir) = cmd.get_current_dir() {
        w.current_dir(dir);
    }
    (w, None)
}

/// The config dir the probe's `claude` will use: `CLAUDE_CONFIG_DIR` as set
/// on `cmd`; `<home>/.claude` if `cmd` removes it; else this process's
/// value, or `<home>/.claude`.
fn config_dir(cmd: &Command, home: &Path) -> PathBuf {
    let on_cmd = cmd
        .get_envs()
        .find(|(k, _)| *k == OsStr::new("CLAUDE_CONFIG_DIR"))
        .map(|(_, v)| v.map(PathBuf::from));
    match on_cmd {
        Some(Some(dir)) => dir,
        Some(None) => home.join(".claude"),
        None => std::env::var_os("CLAUDE_CONFIG_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".claude")),
    }
}

/// `<config>/projects` with every link resolved, when it is an existing
/// absolute directory other than `/` or any folder containing `home` or the
/// config dir (hiding those would hide the credentials, not just the logs).
fn hidden_folder(cmd: &Command, home: &Path) -> Option<PathBuf> {
    let config = config_dir(cmd, home);
    if !config.is_absolute() {
        return None;
    }
    let canon = std::fs::canonicalize(config.join("projects")).ok()?;
    // `starts_with` also holds for equal paths. `canonicalize` always
    // returns an absolute path.
    let inside = |p: &Path| std::fs::canonicalize(p).is_ok_and(|c| c.starts_with(&canon));
    let refused = !canon.is_dir() || canon == Path::new("/") || inside(home) || inside(&config);
    (!refused).then_some(canon)
}
