//! Issue #2040: the shared running-session snapshot. One `/proc` scan per
//! profile root per 30 seconds, shared between processes through a private
//! file, with every acting path reading `/proc` afresh first. See
//! "Running-session snapshot" in docs/reference/profile-command.md.
//!
//! - [`Fresh::Cached`] serves a fresh snapshot without scanning. A stale or
//!   missing one is refreshed by whoever takes the snapshot lock with one
//!   non-blocking attempt; when another process holds the lock, the stale
//!   snapshot is served whatever its age, or `Everything` when there is no
//!   valid one.
//! - [`Fresh::Forced`] never reads the snapshot file. It takes the lock with
//!   one attempt, scans and writes; when the lock is held it is
//!   `Everything`, so the caller acts as if every profile were running.
//! - [`ProcSource::confirmed`] is a cached read followed, only when the
//!   cached answer would permit an action, by a forced read whose answer is
//!   returned.
//!
//! - A record served from the snapshot file whose process has since exited
//!   is dropped before the answer is drawn, so a session that ended never
//!   stays live for the rest of the freshness period. Only records that
//!   could change the answer are checked, one `status` read each.
//!
//! Nothing here blocks, sleeps or retries. A cache fault (a directory or
//! file that fails a check, a failed write) never changes an answer: the
//! caller scans for itself and nothing is kept.

pub mod record;
pub mod store;

use super::auto_switch::liveness;
pub use liveness::{Collected, Liveness, ProcRecord};
use record::Snapshot;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use store::{Found, Lock, Store};

/// How fresh a read must be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fresh {
    /// The shared snapshot may answer.
    Cached,
    /// Read `/proc` now; a busy snapshot lock fails closed.
    Forced,
}

/// Unix seconds.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;
/// `(proc_root, canonical_root, uid)` to the records of one scan.
pub type CollectFn = Arc<dyn Fn(&Path, &Path, u32) -> Collected + Send + Sync>;
/// `(proc_root, pid, uid)`: whether a snapshot record's process may still
/// be running; false only when it has certainly gone.
pub type RunningFn = Arc<dyn Fn(&Path, u32, u32) -> bool + Send + Sync>;
#[cfg(test)]
type FixedFn = dyn Fn(&Path) -> Liveness + Send + Sync;

/// Everything a [`ProcSource`] reads, injectable by tests.
#[derive(Clone)]
pub struct SourceDeps {
    pub proc_root: PathBuf,
    /// The base directory; the files go in `<runtime_dir>/amplihack/`.
    /// `None` keeps no snapshot at all.
    pub runtime_dir: Option<PathBuf>,
    pub clock: Clock,
    pub uid: u32,
    /// The reader's own pid, dropped from every answer.
    pub pid: u32,
    pub collect: CollectFn,
    pub may_be_running: RunningFn,
}

impl SourceDeps {
    /// Dependencies with the real scanner.
    pub fn new(
        proc_root: PathBuf,
        runtime_dir: Option<PathBuf>,
        clock: Clock,
        uid: u32,
        pid: u32,
    ) -> Self {
        Self {
            proc_root,
            runtime_dir,
            clock,
            uid,
            pid,
            collect: Arc::new(liveness::collect),
            may_be_running: Arc::new(liveness::may_be_running),
        }
    }
}

enum Inner {
    /// This host, resolved on each read.
    System,
    #[cfg(test)]
    Deps(SourceDeps),
    /// A fixed answer for every read; for tests of the callers.
    #[cfg(test)]
    Fixed(Box<FixedFn>),
}

/// Where running sessions are read from. Cheap to clone.
#[derive(Clone)]
pub struct ProcSource(Arc<Inner>);

impl ProcSource {
    /// This host: `/proc` (or the guarded test seam), the private runtime
    /// directory, the system clock, this process's euid and pid.
    pub fn system() -> Self {
        Self(Arc::new(Inner::System))
    }

    /// Injected dependencies; tests only.
    #[cfg(test)]
    pub fn with(deps: SourceDeps) -> Self {
        Self(Arc::new(Inner::Deps(deps)))
    }

    /// Every read, cached or forced, calls `f`; tests only.
    #[cfg(test)]
    pub fn from_fn(f: impl Fn(&Path) -> Liveness + Send + Sync + 'static) -> Self {
        Self(Arc::new(Inner::Fixed(Box::new(f))))
    }

    /// The running sessions under `root`.
    pub fn liveness(&self, root: &Path, fresh: Fresh) -> Liveness {
        match &*self.0 {
            #[cfg(test)]
            Inner::Fixed(f) => f(root),
            #[cfg(test)]
            Inner::Deps(d) => read(d, root, fresh),
            Inner::System => match system_deps() {
                Some(d) => read(&d, root, fresh),
                None => Liveness::Unsupported,
            },
        }
    }

    /// A cached read; when `permits` holds for it, a forced read decides.
    /// `Everything` and `Unsupported` must never permit an action.
    pub fn confirmed(&self, root: &Path, permits: impl FnOnce(&Liveness) -> bool) -> Liveness {
        self.confirmed_fresh(root, permits).0
    }

    /// [`confirmed`](Self::confirmed), also saying whether the answer came
    /// from a forced read.
    pub fn confirmed_fresh(
        &self,
        root: &Path,
        permits: impl FnOnce(&Liveness) -> bool,
    ) -> (Liveness, Fresh) {
        let cached = self.liveness(root, Fresh::Cached);
        if permits(&cached) {
            (self.liveness(root, Fresh::Forced), Fresh::Forced)
        } else {
            (cached, Fresh::Cached)
        }
    }
}

#[cfg(target_os = "linux")]
fn system_deps() -> Option<SourceDeps> {
    use amplihack_utils::secure_files::euid;
    let uid = euid();
    // Unit tests never keep a snapshot in the user's runtime directory; they
    // inject one through `ProcSource::with`.
    let runtime_dir = if cfg!(test) {
        None
    } else {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .and_then(|home| {
                let xdg = std::env::var_os("XDG_RUNTIME_DIR");
                let dir = store::runtime_dir(xdg.as_deref(), &home, uid);
                if dir.is_none() {
                    let tried = xdg.map_or_else(|| home.join(".amplihack/runtime"), PathBuf::from);
                    warn_no_cache(&tried, "no private runtime directory");
                }
                dir
            })
    };
    Some(SourceDeps::new(
        crate::commands::agent::procfs::proc_root(),
        runtime_dir,
        Arc::new(super::auto_switch::fsx::now),
        uid,
        std::process::id(),
    ))
}

/// `/proc` is Linux only: elsewhere every read is `Unsupported`.
#[cfg(not(target_os = "linux"))]
fn system_deps() -> Option<SourceDeps> {
    None
}

fn read(d: &SourceDeps, root: &Path, fresh: Fresh) -> Liveness {
    // An unusable root is never cached: it says nothing about processes.
    let Ok(root) = root.canonicalize() else {
        return Liveness::Unsupported;
    };
    let scan = || (d.collect)(&d.proc_root, &root, d.uid);
    let Some(store) = open_store(d, &root) else {
        return liveness::derive(&scan(), d.pid);
    };
    if fresh == Fresh::Cached
        && let Found::Fresh(snap) = store.read(d.uid, (d.clock)())
    {
        return derive(d, snap);
    }
    match store.try_lock() {
        Ok(Lock::Held(_lock)) => {
            let taken_at = (d.clock)();
            let collected = scan();
            let snap = Snapshot::from_collected(&collected, d.uid, store.key(), taken_at, d.pid);
            if let Err(e) = store.write(&snap)
                && let Some(runtime) = d.runtime_dir.as_deref()
            {
                warn_no_cache(runtime, &format!("write failed: {e}"));
            }
            liveness::derive(&collected, d.pid)
        }
        // Someone else is refreshing. A forced read must not trust the file.
        Ok(Lock::Busy) => match fresh {
            Fresh::Forced => Liveness::Everything,
            Fresh::Cached => match store.read(d.uid, (d.clock)()) {
                Found::Fresh(snap) | Found::Stale(snap) => derive(d, snap),
                Found::Missing => Liveness::Everything,
            },
        },
        Err(e) => {
            if let Some(runtime) = d.runtime_dir.as_deref() {
                warn_no_cache(runtime, &format!("lock unusable: {e}"));
            }
            liveness::derive(&scan(), d.pid)
        }
    }
}

/// Says once per process, at `warn`, that the snapshot cache cannot be
/// used, so every read scans `/proc` itself. The answers stay correct; only
/// the saving is lost. Faults in one snapshot file stay at `debug`.
fn warn_no_cache(path: &Path, why: &str) {
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        tracing::warn!(
            "proc snapshot cache unusable at {}: {why}; every read scans /proc",
            path.display()
        );
    });
}

/// The answer from a snapshot file, without the records of processes that
/// have exited since it was taken. A record that cannot change the answer
/// (no profile, and not an unreadable Claude-like environment) is kept
/// unchecked.
fn derive(d: &SourceDeps, snap: Snapshot) -> Liveness {
    let collected = match snap.into_collected() {
        Collected::Procs(procs) => Collected::Procs(
            procs
                .into_iter()
                .filter(|r| {
                    let matters = r.profile.is_some() || (r.claude_like && r.environ_unreadable);
                    !matters || r.pid == d.pid || (d.may_be_running)(&d.proc_root, r.pid, d.uid)
                })
                .collect(),
        ),
        other => other,
    };
    liveness::derive(&collected, d.pid)
}

/// The checked snapshot store for this proc root and canonical `root`, or
/// `None` when no snapshot can be kept.
fn open_store(d: &SourceDeps, root: &Path) -> Option<Store> {
    let runtime = d.runtime_dir.as_deref()?;
    let proc_root = d.proc_root.canonicalize().ok()?;
    match Store::open(runtime, store::key(&proc_root, root)) {
        Ok(s) => Some(s),
        Err(e) => {
            warn_no_cache(runtime, &format!("directory unusable: {e}"));
            None
        }
    }
}

#[cfg(test)]
mod tests_proc_snapshot;
