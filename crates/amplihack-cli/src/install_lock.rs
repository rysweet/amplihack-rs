//! The install lock (issues #502 and #1954).
//!
//! Every piece of install work — `amplihack install`, the post-update install
//! child, `update`'s binary replacement, startup self-heal and `uninstall` —
//! runs while holding an exclusive `flock` on `~/.amplihack/.install.lock`.
//! Two installers therefore never stage, switch or clean up at the same time
//! (issue #1954 cause B: an install and a self-heal collided on the shared
//! staging directory).
//!
//! Rules:
//! - Blocking, with no timeout. After about two seconds of waiting one line
//!   naming the lock file is printed to stderr ([`wait_message`]).
//! - Re-entrant on the owning thread only (S19). Self-heal calls the install
//!   path, which takes the lock again; that nests. Any other thread of the
//!   same process waits, exactly as another process does.
//! - The lock file is opened `O_NOFOLLOW` and `O_CLOEXEC`, created 0600 and
//!   never truncated (S17, S20). A child started while the lock is held does
//!   not inherit it, so `update` can drop it and start the install child.
//! - No signal is ever sent to anyone (issue #1954 R8).

use anyhow::{Context, Result, anyhow, bail};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock, mpsc};
use std::thread::ThreadId;
use std::time::Duration;

/// File name of the lock, next to the install stamp in `~/.amplihack/`.
pub const LOCK_FILE: &str = ".install.lock";

/// How long a wait lasts before [`wait_message`] is printed.
const WAIT_NOTICE_AFTER: Duration = Duration::from_secs(2);

/// `<home>/.amplihack/.install.lock`.
pub fn lock_path(home: &Path) -> PathBuf {
    home.join(".amplihack").join(LOCK_FILE)
}

/// The one line printed when an install has waited for the lock for about two
/// seconds. The text is part of the documented output.
pub fn wait_message(path: &Path) -> String {
    format!(
        "⏳ waiting for another amplihack install to finish (lock: {})",
        path.display()
    )
}

/// In-process ownership of one lock path.
struct Entry {
    owner: Option<ThreadId>,
    depth: usize,
    file: Option<File>,
}

struct Registry {
    entries: Mutex<HashMap<PathBuf, Entry>>,
    released: Condvar,
}

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| Registry {
        entries: Mutex::new(HashMap::new()),
        released: Condvar::new(),
    })
}

fn entries() -> MutexGuard<'static, HashMap<PathBuf, Entry>> {
    // A panic while the registry mutex was held cannot leave an entry half
    // updated in a way that matters: the flock is released by the kernel when
    // its `File` drops. Recover the map rather than poisoning every install.
    registry()
        .entries
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Held while install work runs. Dropping the outermost guard releases the
/// `flock`.
#[must_use = "the install lock is released when the guard is dropped"]
pub struct InstallLockGuard {
    path: PathBuf,
}

impl std::fmt::Debug for InstallLockGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstallLockGuard")
            .field("path", &self.path)
            .finish()
    }
}

impl Drop for InstallLockGuard {
    fn drop(&mut self) {
        let mut map = entries();
        let Some(entry) = map.get_mut(&self.path) else {
            return;
        };
        entry.depth = entry.depth.saturating_sub(1);
        if entry.depth == 0 {
            if let Some(file) = entry.file.take()
                && let Err(e) = file.unlock()
            {
                // The kernel releases the lock when `file` closes just below.
                tracing::debug!("install lock: explicit unlock failed: {e}");
            }
            map.remove(&self.path);
            registry().released.notify_all();
        }
    }
}

/// Whether the current thread holds any install lock.
pub fn held_by_current_thread() -> bool {
    let me = std::thread::current().id();
    entries().values().any(|e| e.owner == Some(me))
}

/// Takes the install lock for the current `HOME`.
pub fn acquire() -> Result<InstallLockGuard> {
    let home = crate::commands::install::paths::home_dir()?;
    acquire_at(&lock_path(&home))
}

/// Runs `body` while holding the install lock for the current `HOME`.
pub fn with_install_lock<T, F: FnOnce() -> Result<T>>(body: F) -> Result<T> {
    let _guard = acquire()?;
    body()
}

/// Takes the install lock at `path`, waiting as long as it takes.
pub fn acquire_at(path: &Path) -> Result<InstallLockGuard> {
    let me = std::thread::current().id();
    let path = path.to_path_buf();
    {
        let mut map = entries();
        loop {
            let owned_by_me = map.get(&path).map(|entry| entry.owner == Some(me));
            match owned_by_me {
                Some(true) => {
                    if let Some(entry) = map.get_mut(&path) {
                        entry.depth += 1;
                    }
                    return Ok(InstallLockGuard { path });
                }
                Some(false) => {
                    map = registry()
                        .released
                        .wait(map)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
                None => {
                    // Reserve the path for this thread before the slow flock,
                    // so other threads queue on the condvar, not on the file.
                    map.insert(
                        path.clone(),
                        Entry {
                            owner: Some(me),
                            depth: 0,
                            file: None,
                        },
                    );
                    break;
                }
            }
        }
    }

    match open_and_lock(&path) {
        Ok(file) => {
            let mut map = entries();
            let entry = map.get_mut(&path).ok_or_else(|| {
                anyhow!("install lock reservation for {} vanished", path.display())
            })?;
            entry.file = Some(file);
            entry.depth = 1;
            Ok(InstallLockGuard { path })
        }
        Err(error) => {
            let mut map = entries();
            map.remove(&path);
            registry().released.notify_all();
            Err(error)
        }
    }
}

fn open_lock_file(path: &Path) -> Result<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // std already opens with O_CLOEXEC; O_NOFOLLOW refuses a symlink at
        // the lock path, so the lock never creates or locks another file.
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .with_context(|| format!("failed to open install lock {}", path.display()))?;
    let meta = file
        .metadata()
        .with_context(|| format!("failed to stat install lock {}", path.display()))?;
    if !meta.is_file() {
        bail!("install lock {} is not a regular file", path.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let euid = crate::commands::install::release_layout::euid();
        if meta.uid() != euid {
            bail!(
                "install lock {} is owned by uid {}, not by this user",
                path.display(),
                meta.uid()
            );
        }
        if meta.mode() & 0o077 != 0 {
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("failed to chmod install lock {}", path.display()))?;
        }
    }
    Ok(file)
}

fn open_and_lock(path: &Path) -> Result<File> {
    let file = open_lock_file(path)?;
    match file.try_lock() {
        Ok(()) => return Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => {}
        Err(std::fs::TryLockError::Error(e)) => {
            return Err(e)
                .with_context(|| format!("failed to acquire install lock {}", path.display()));
        }
    }
    // Another process holds it. Wait without a timeout, and say why after a
    // couple of seconds so a blocked command is not a mystery.
    let (done, waiting) = mpsc::channel::<()>();
    let message = wait_message(path);
    let notifier = std::thread::spawn(move || {
        if let Err(mpsc::RecvTimeoutError::Timeout) = waiting.recv_timeout(WAIT_NOTICE_AFTER) {
            eprintln!("{message}");
        }
    });
    let locked = file.lock();
    drop(done);
    if notifier.join().is_err() {
        tracing::debug!("install lock: the wait-notice thread panicked");
    }
    locked.with_context(|| format!("failed to acquire install lock {}", path.display()))?;
    Ok(file)
}
