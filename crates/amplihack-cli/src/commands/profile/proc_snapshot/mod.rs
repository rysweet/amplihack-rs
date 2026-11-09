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
//! Nothing here blocks, sleeps or retries. A cache fault (a directory or
//! file that fails a check, a failed write) never changes an answer: the
//! caller scans for itself and nothing is kept.

pub mod record;
pub mod store;

pub use super::auto_switch::liveness::{Collected, Liveness, ProcRecord};
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
            collect: Arc::new(super::auto_switch::liveness::collect),
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
        let cached = self.liveness(root, Fresh::Cached);
        if permits(&cached) {
            self.liveness(root, Fresh::Forced)
        } else {
            cached
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
                store::runtime_dir(std::env::var_os("XDG_RUNTIME_DIR").as_deref(), &home, uid)
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
        return super::auto_switch::liveness::derive(&scan(), d.pid);
    };
    if fresh == Fresh::Cached
        && let Found::Fresh(snap) = store.read(d.uid, (d.clock)())
    {
        return derive(&snap, d.pid);
    }
    match store.try_lock() {
        Ok(Lock::Held(_lock)) => {
            let taken_at = (d.clock)();
            let collected = scan();
            let snap = Snapshot::from_collected(&collected, d.uid, store.key(), taken_at, d.pid);
            if let Err(e) = store.write(&snap) {
                tracing::debug!("proc snapshot not kept: {e}");
            }
            super::auto_switch::liveness::derive(&collected, d.pid)
        }
        // Someone else is refreshing. A forced read must not trust the file.
        Ok(Lock::Busy) => match fresh {
            Fresh::Forced => Liveness::Everything,
            Fresh::Cached => match store.read(d.uid, (d.clock)()) {
                Found::Fresh(snap) | Found::Stale(snap) => derive(&snap, d.pid),
                Found::Missing => Liveness::Everything,
            },
        },
        Err(e) => {
            tracing::debug!("proc snapshot lock unusable: {e}; scanning without the cache");
            super::auto_switch::liveness::derive(&scan(), d.pid)
        }
    }
}

fn derive(snap: &Snapshot, pid: u32) -> Liveness {
    super::auto_switch::liveness::derive(&snap.collected(), pid)
}

/// The checked snapshot store for this proc root and canonical `root`, or
/// `None` when no snapshot can be kept.
fn open_store(d: &SourceDeps, root: &Path) -> Option<Store> {
    let runtime = d.runtime_dir.as_deref()?;
    let proc_root = d.proc_root.canonicalize().ok()?;
    match Store::open(runtime, store::key(&proc_root, root)) {
        Ok(s) => Some(s),
        Err(e) => {
            tracing::debug!("proc snapshot directory unusable: {e}; no cache");
            None
        }
    }
}

#[cfg(test)]
mod tests_proc_snapshot;
