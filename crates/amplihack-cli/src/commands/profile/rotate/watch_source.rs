//! Issue #2029: the one watch abstraction the rotator blocks on.
//!
//! No such abstraction existed before this issue: `limit_wait::fingerprint`
//! is a value comparator polled on a timer, not kernel notification. One
//! trait with two platform backends is added here, over `libc` alone, and
//! no third-party watcher crate is introduced.
//!
//! Observation and ledger directories retain notifications across atomic
//! replacement: a per-file-only subscription would be lost on rename.
//! macOS additionally watches existing login/control files for in-place
//! writes, which directory vnodes do not report. Descriptors remain bounded
//! by the login count, not the number of publications.
//!
//! Events are coarse: [`Event`] carries no path, no name and no mask. A
//! decision is taken from re-read, validated state, so a planted filename
//! can never reach a decision as data.
//!
//! Every descriptor opened here is close-on-exec. The rotator spawns
//! `claude` on every probe, and a leaked descriptor on a login's directory
//! would hand that child a handle to the credential directory that
//! `CLAUDE_CONFIG_DIR` pinning otherwise keeps it away from.

use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::STATE_DIR;
use crate::commands::profile::name::ProfileName;
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
#[path = "watch_inotify.rs"]
mod backend;
#[cfg(not(target_os = "linux"))]
#[path = "watch_kqueue.rs"]
mod backend;

#[path = "watch_wake.rs"]
mod wake;

pub(crate) use wake::WakeHandle;
pub(crate) use wake::WakePipe;

/// What a wait reports. Coarse by design: no path, name or mask reaches a
/// decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Something under a watched directory changed. Which thing is never
    /// said, because a decision re-reads the state rather than trusting an
    /// event.
    Changed,
    /// The kernel dropped events (`IN_Q_OVERFLOW` on Linux). The reader has
    /// missed changes, so the next pass reads every state file again.
    Overflow,
    /// Completed jobs are available in the authoritative queue.
    Completion,
    /// SIGTERM or SIGINT reached the process. Always the last event of the
    /// batch that carries it, so a reading written as the rotator stops is
    /// decided before it ends.
    Stop,
}

/// Kernel file-change notification, as the rotator uses it.
pub trait Events {
    /// Reconciles the watch set with `dirs`: a directory already watched
    /// keeps its registration, a new one is added and one no longer named
    /// is dropped. A directory that cannot be watched is reported with its
    /// reason, never swallowed: partial coverage looks like a working
    /// rotator that never rotates.
    fn watch(&mut self, dirs: &[PathBuf]) -> Result<(), ProfileError>;

    /// The directories currently watched. Read by the tests that check
    /// `watch` reconciles rather than re-registering wholesale.
    #[cfg(test)]
    fn watched(&self) -> Vec<PathBuf>;

    /// Blocks without a deadline until filesystem or completion/shutdown readiness.
    fn wait(&mut self) -> Result<Vec<Event>, ProfileError>;

    fn wake_handle(&self) -> WakeHandle {
        panic!("scripted source has no wake handle")
    }
    #[cfg(test)]
    fn raw_fds_for_test(&self) -> Vec<std::os::fd::RawFd> {
        Vec::new()
    }

    /// What is pending now. Never blocks.
    fn drain(&mut self) -> Result<Vec<Event>, ProfileError>;
}

/// A directory that could not be watched, and whether leaving it out is an
/// outcome to report or a fault that ends the rotator.
///
/// The design is that a directory that fails is not watched and the refusal
/// is reported. Two cases are ordinary rather than faults: a directory
/// removed between the listing and the open, and a symlink at the path,
/// which `O_NOFOLLOW` refuses and which a dotfile manager's `~/.claude` is.
/// Everything else — a permission fault, the inotify limit — would present
/// as a rotator that runs and never rotates, so it still ends the process.
pub(crate) struct DirFail {
    pub(crate) err: ProfileError,
    pub(crate) skip: bool,
}

impl DirFail {
    /// From the errno of an `open` or an `inotify_add_watch`.
    pub(crate) fn io(path: &Path, e: &std::io::Error) -> Self {
        let skip = matches!(
            e.raw_os_error(),
            Some(libc::ENOENT) | Some(libc::ELOOP) | Some(libc::ENOTDIR)
        );
        Self {
            err: dir_err(path, e),
            skip,
        }
    }

    /// A refusal with no errno behind it.
    pub(crate) fn fault(path: &Path, reason: impl std::fmt::Display) -> Self {
        Self {
            err: dir_err(path, reason),
            skip: false,
        }
    }
}

pub(crate) fn dir_err(path: &Path, reason: impl std::fmt::Display) -> ProfileError {
    ProfileError::RotateDaemon(format!("watch: {}: {reason}", path.display()))
}

/// The directories left out of the watch set, so a standing refusal is
/// reported when it starts and when it clears rather than once per block:
/// the set is reconciled before every wait.
#[derive(Default)]
pub(crate) struct Skipped(std::collections::BTreeSet<PathBuf>);

impl Skipped {
    /// Reports `dir` as not watched, once until it is watched again.
    pub(crate) fn note(&mut self, dir: &Path, err: &ProfileError) {
        if self.0.insert(dir.to_path_buf()) {
            eprintln!("warning: rotate: {err}; it is not watched");
        }
    }

    /// Notes that `dir` is watched again.
    pub(crate) fn clear(&mut self, dir: &Path) {
        if self.0.remove(dir) {
            eprintln!("rotate: {} is watched again", dir.display());
        }
    }
}

/// Puts `Stop` last, whatever order the batch arrived in.
pub(crate) fn stop_last(mut batch: Vec<Event>) -> Vec<Event> {
    if let Some(i) = batch.iter().position(|e| *e == Event::Stop) {
        let stop = batch.remove(i);
        batch.push(stop);
    }
    batch
}

/// The platform backend over `dirs`, with SIGTERM and SIGINT waking a
/// blocked wait through a self-pipe polled alongside the event source.
/// Without that wake a blocked read would not see the signal and
/// `rotate stop` would start failing against its ten-second notice.
pub fn open(dirs: &[PathBuf]) -> Result<Box<dyn Events>, ProfileError> {
    Ok(Box::new(backend::Backend::open(dirs)?))
}

/// Every directory a decision's inputs live under.
///
/// `~/.claude.json` has no separate subscription: credential and ledger
/// changes provide login events. Startup discovery may consult its account
/// metadata, but editing that metadata cannot establish a login generation.
pub fn watch_dirs(root: &Path, home: &Path, profiles: &[ProfileName]) -> Vec<PathBuf> {
    let state = root.join(STATE_DIR);
    let mut dirs = vec![
        // The readings, which a status line writes on every turn.
        state.join("observations"),
        // The auto-switch ledger, the journal and the toggle's directory.
        state.clone(),
        // The rotate ledger, the pin and the journal the rotator keeps.
        state.join("rotate"),
        state.join(crate::commands::profile::expected::SUB),
        // Parent subscriptions survive atomic replacement of watched roots.
        home.to_path_buf(),
        // The login list: a profile added, removed or re-logged.
        root.to_path_buf(),
        // The live login, and a hand-run `claude /login`.
        home.join(".claude"),
    ];
    if let Some(parent) = root.parent() {
        dirs.push(parent.to_path_buf());
    }
    dirs.extend(profiles.iter().map(|p| root.join(p.as_str())));
    dirs.sort();
    dirs.dedup();
    dirs
}
