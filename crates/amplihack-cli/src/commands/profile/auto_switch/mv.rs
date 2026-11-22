//! Primitives that move a login file, shared by auto-switch and `profile
//! rotate` (issue #1592). A login file is only ever renamed, never opened:
//! with `RENAME_NOREPLACE`, or exchanged with another login atomically.
//! Claude's refresh lock is taken and released by path and inode.

use super::fsx;
use crate::commands::profile::store::rename_noreplace;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

pub const REFRESH_LOCK: &str = ".oauth_refresh.lock";

#[cfg(debug_assertions)]
const TEST_ABORT_ENV: &str = "AMPLIHACK_AUTO_SWITCH_TEST_ABORT_AFTER";

/// Debug builds only: stop the process at a named step, as a crash would.
pub fn abort_point(step: &str) {
    #[cfg(debug_assertions)]
    if std::env::var_os(TEST_ABORT_ENV).is_some_and(|v| v == step) {
        std::process::exit(86);
    }
    let _ = step;
}

pub enum MoveError {
    /// The destination exists: two copies, and amplihack cannot tell which is
    /// current.
    Exists,
    /// The filesystem or platform cannot rename between these paths.
    Unsupported,
    Other(String),
}

fn unsupported(e: &std::io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc::EXDEV | libc::EINVAL | libc::ENOSYS)
    )
}

/// Syncs the directories holding `a` and `b` after a rename.
fn sync_parents(a: &Path, b: &Path) -> std::io::Result<()> {
    let pa = a.parent().unwrap_or(Path::new("."));
    fsx::fsync_dir(pa)?;
    match b.parent() {
        Some(pb) if pb != pa => fsx::fsync_dir(pb),
        _ => Ok(()),
    }
}

/// Renames one login file. The source must be a regular file owned by the
/// current user with one hard link; the destination must not exist. `show`
/// turns a path into the text used in messages.
pub fn move_login(src: &Path, dst: &Path, show: &dyn Fn(&Path) -> String) -> Result<(), MoveError> {
    if !fsx::movable_login(src) {
        return Err(MoveError::Other(format!(
            "{} is not a private regular file with one link",
            show(src)
        )));
    }
    match rename_noreplace(src, dst) {
        Ok(()) => {}
        Err(e) if e.raw_os_error() == Some(libc::EEXIST) => return Err(MoveError::Exists),
        Err(e) if unsupported(&e) => return Err(MoveError::Unsupported),
        Err(e) => return Err(MoveError::Other(format!("{}: {e}", show(src)))),
    }
    sync_parents(dst, src).map_err(|e| MoveError::Other(format!("sync after move: {e}")))
}

/// The exchange of two login files in one atomic step (issue #2935). The
/// swap takes one of these, so a test of the swap's decisions supplies its
/// own and calls nothing that only one system has.
pub trait LoginExchange: Send + Sync {
    /// Whether this exchange exists at all. `false` refuses a swap before
    /// anything is written.
    fn supported(&self) -> bool;

    /// Swaps the two existing names, so that each path holds a login
    /// throughout. On an error nothing has moved.
    fn exchange(&self, a: &Path, b: &Path) -> std::io::Result<()>;
}

/// The host's exchange: `renameat2(RENAME_EXCHANGE)`, which only Linux has.
/// Anywhere else it is not supported and moves nothing. There is no copy
/// fallback.
pub struct HostExchange;

impl LoginExchange for HostExchange {
    fn supported(&self) -> bool {
        cfg!(target_os = "linux")
    }

    #[cfg(target_os = "linux")]
    fn exchange(&self, a: &Path, b: &Path) -> std::io::Result<()> {
        use std::os::unix::ffi::OsStrExt;
        let cstr = |p: &Path| {
            std::ffi::CString::new(p.as_os_str().as_bytes())
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))
        };
        let (a, b) = (cstr(a)?, cstr(b)?);
        // SAFETY: both paths are valid NUL-terminated strings; RENAME_EXCHANGE
        // swaps two existing names atomically and creates or removes nothing.
        let rc = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                a.as_ptr(),
                libc::AT_FDCWD,
                b.as_ptr(),
                libc::RENAME_EXCHANGE,
            )
        };
        if rc == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn exchange(&self, _a: &Path, _b: &Path) -> std::io::Result<()> {
        Err(std::io::Error::from_raw_os_error(libc::ENOSYS))
    }
}

/// A stand-in for tests, on every system: it swaps the two files with three
/// plain renames through a spare name beside `a`. It is not atomic, so it is
/// compiled into test and debug builds only and no shipped binary holds it.
/// A lib test reaches it through `FakeExchange`; a test that starts the
/// binary asks for it with `AMPLIHACK_TEST_LOGIN_EXCHANGE`, and then gets it
/// only inside the temp dir ([`TempDirExchange`]).
#[cfg(any(test, debug_assertions))]
pub struct RenameExchange;

#[cfg(any(test, debug_assertions))]
impl LoginExchange for RenameExchange {
    fn supported(&self) -> bool {
        true
    }

    fn exchange(&self, a: &Path, b: &Path) -> std::io::Result<()> {
        let mut spare = a.as_os_str().to_owned();
        spare.push(".exchange-stand-in");
        let spare = Path::new(&spare);
        // Checked first, so a missing `b` moves nothing.
        fs::symlink_metadata(b)?;
        rename_noreplace(a, spare)?;
        if let Err(e) = fs::rename(b, a) {
            let _ = fs::rename(spare, a);
            return Err(e);
        }
        rename_noreplace(spare, b)
    }
}

/// What a debug binary uses when a test asked for the stand-in:
/// [`RenameExchange`], for two files that both lie inside `temp_dir`, and a
/// refusal that moves nothing for any other pair.
///
/// The two variables that select the stand-in say only who started the
/// process. This says which files it may move, and it is checked on every
/// call. A test's logins are in a folder the test made under the temp dir. A
/// person's logins are under their home folder and in the profile root,
/// neither of which is under the temp dir. So a debug binary that carries
/// both variables by mistake declines to swap real logins, where before it
/// would have swapped them with three renames that are not atomic.
#[cfg(any(test, debug_assertions))]
pub struct TempDirExchange {
    temp_dir: std::path::PathBuf,
}

#[cfg(any(test, debug_assertions))]
impl TempDirExchange {
    /// The stand-in for files inside `temp_dir`.
    pub fn inside(temp_dir: std::path::PathBuf) -> Self {
        Self { temp_dir }
    }
}

#[cfg(any(test, debug_assertions))]
impl LoginExchange for TempDirExchange {
    fn supported(&self) -> bool {
        true
    }

    fn exchange(&self, a: &Path, b: &Path) -> std::io::Result<()> {
        let bases = [self.temp_dir.clone()];
        for login in [a, b] {
            if !crate::user_config::under_any(login, &bases) {
                eprintln!("{}", outside_temp_dir_line(login, &self.temp_dir));
                return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
            }
        }
        RenameExchange.exchange(a, b)
    }
}

/// The one line [`TempDirExchange`] prints when it refuses a pair.
#[cfg(any(test, debug_assertions))]
pub(super) fn outside_temp_dir_line(login: &Path, temp_dir: &Path) -> String {
    format!(
        "amplihack: AMPLIHACK_TEST_LOGIN_EXCHANGE asks for the test stand-in, which moves files \
         inside the temp dir ({}) only; {} is outside it, so nothing was moved. Unset the \
         variable to use this system's own exchange",
        temp_dir.display(),
        login.display()
    )
}

/// Debug builds only: the value `renames`, in a process that also carries
/// the test home variable, makes [`host_exchange`] the stand-in.
#[cfg(debug_assertions)]
pub const TEST_EXCHANGE_ENV: &str = "AMPLIHACK_TEST_LOGIN_EXCHANGE";

/// The exchange a command uses: the host's. A test that starts a debug
/// binary can ask for the stand-in instead, and only together with the test
/// home variable, which every test child carries. What it then gets moves
/// files inside the temp dir and no others ([`TempDirExchange`]), so the two
/// variables alone never reach a real login.
pub fn host_exchange() -> std::sync::Arc<dyn LoginExchange> {
    #[cfg(debug_assertions)]
    {
        let set = |name: &str| std::env::var_os(name);
        exchange_for(
            set(TEST_EXCHANGE_ENV).as_deref(),
            set(crate::user_config::TEST_HOME_GUARD_ENV).as_deref(),
            std::env::temp_dir(),
        )
    }
    #[cfg(not(debug_assertions))]
    std::sync::Arc::new(HostExchange)
}

/// [`host_exchange`] for the two variables' values and the temp dir given:
/// the stand-in confined to `temp_dir` when [`stand_in_requested`], and the
/// host's exchange otherwise.
#[cfg(any(test, debug_assertions))]
pub(super) fn exchange_for(
    exchange: Option<&std::ffi::OsStr>,
    test_home: Option<&std::ffi::OsStr>,
    temp_dir: std::path::PathBuf,
) -> std::sync::Arc<dyn LoginExchange> {
    if stand_in_requested(exchange, test_home) {
        std::sync::Arc::new(TempDirExchange::inside(temp_dir))
    } else {
        std::sync::Arc::new(HostExchange)
    }
}

/// The pure part of [`host_exchange`]: exactly `renames`, and a test home
/// variable that is not empty.
#[cfg(any(test, debug_assertions))]
pub(super) fn stand_in_requested(
    exchange: Option<&std::ffi::OsStr>,
    test_home: Option<&std::ffi::OsStr>,
) -> bool {
    exchange.is_some_and(|v| v == "renames") && test_home.is_some_and(|v| !v.is_empty())
}

/// The fake a lib test holds. It keeps every call in memory, and swaps the
/// two files, which lie in the test's own folder, with [`RenameExchange`].
/// [`FakeExchange::absent`] answers as the host's does where there is no
/// such call: not supported, and nothing moves.
#[cfg(test)]
pub struct FakeExchange {
    supported: bool,
    calls: std::sync::Mutex<Vec<(std::path::PathBuf, std::path::PathBuf)>>,
}

#[cfg(test)]
impl FakeExchange {
    pub fn working() -> std::sync::Arc<Self> {
        Self::new(true)
    }

    pub fn absent() -> std::sync::Arc<Self> {
        Self::new(false)
    }

    fn new(supported: bool) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            supported,
            calls: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// Every pair this fake was asked to exchange, in order.
    pub fn calls(&self) -> Vec<(std::path::PathBuf, std::path::PathBuf)> {
        self.calls.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

#[cfg(test)]
impl LoginExchange for FakeExchange {
    fn supported(&self) -> bool {
        self.supported
    }

    fn exchange(&self, a: &Path, b: &Path) -> std::io::Result<()> {
        self.calls
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push((a.to_path_buf(), b.to_path_buf()));
        if !self.supported {
            return Err(std::io::Error::from_raw_os_error(libc::ENOSYS));
        }
        RenameExchange.exchange(a, b)
    }
}

/// Exchanges two login files through `by`, in one atomic step when `by` is
/// the host's. Both must pass [`fsx::movable_login`]. Where `by` cannot
/// exchange, and on a filesystem that refuses it, the result is
/// `Unsupported` and nothing has moved.
pub fn exchange_logins(by: &dyn LoginExchange, a: &Path, b: &Path) -> Result<(), MoveError> {
    if !fsx::movable_login(a) || !fsx::movable_login(b) {
        return Err(MoveError::Other(
            "a login file is not a private regular file with one link".into(),
        ));
    }
    by.exchange(a, b).map_err(|e| {
        if unsupported(&e) {
            MoveError::Unsupported
        } else {
            MoveError::Other(format!("exchange failed: {}", e.kind()))
        }
    })?;
    sync_parents(a, b).map_err(|e| MoveError::Other(format!("sync after exchange: {e}")))
}

/// Takes `<dir>/.oauth_refresh.lock` with a non-blocking `mkdir`, as Claude
/// does, and returns its inode. An `AlreadyExists` error means Claude (or
/// another swap) holds it: the lock is never broken.
pub fn take_refresh_lock_at(dir: &Path) -> std::io::Result<u64> {
    let path = dir.join(REFRESH_LOCK);
    let mut b = fs::DirBuilder::new();
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(&path)?;
    match fs::symlink_metadata(&path) {
        Ok(m) => Ok(m.ino()),
        Err(e) => {
            // Ours, but its inode is unknown: remove it rather than leave
            // a lock nothing will release.
            let _ = fs::remove_dir(&path);
            Err(e)
        }
    }
}

/// Removes the refresh lock only if it is a directory with inode `ino`, the
/// one amplihack created. Returns whether it was removed. When it is ours
/// but cannot be removed, says so: nothing retries, and while it stays
/// Claude cannot refresh its token in `dir`.
pub fn release_refresh_lock_at(dir: &Path, ino: u64) -> bool {
    let path = dir.join(REFRESH_LOCK);
    if !fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir() && m.ino() == ino) {
        return false;
    }
    match fs::remove_dir(&path) {
        Ok(()) => true,
        Err(e) => {
            eprintln!(
                "warning: could not remove {} ({e}); remove it so Claude can refresh its token",
                path.display()
            );
            false
        }
    }
}
