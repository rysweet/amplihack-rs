//! The `amplihack` binary, built once and copied to a private directory
//! (issue #2019, race A; issue #2932, once per run).
//!
//! It lives in `bins/amplihack`, not in this crate, so
//! `CARGO_BIN_EXE_amplihack` is not defined here, and `cargo test -p
//! amplihack-cli` does not build it. A leftover binary may be stale, so the
//! first caller runs `cargo build -p amplihack` (a no-op when current).
//!
//! Cargo can re-link `target/debug/amplihack` while this suite runs (another
//! test binary's build, or a `cargo build` beside it), which leaves a test
//! that runs it with `NotFound` or a half-written file. So the first caller
//! copies the built binary, and `amplihack-hooks` beside it when the build
//! left a regular file, into a fresh private directory, and every caller
//! gets that copy. Nothing re-links it during the run.
//!
//! "First caller" is per test run where the runner says which run this is.
//! cargo-nextest starts every test in its own process and gives them all the
//! same `NEXTEST_RUN_ID`. Building and copying in each of those processes
//! cost a `cargo build` and a copy of the binary per test: about 14 seconds
//! for the median `cli_integration` test, most of it waiting for cargo's
//! lock on the build directory (issue #2932). With a run id, the first
//! process of the run builds and copies under a file lock into a directory
//! named for the run, and every other process of the run uses that copy.
//! The directory is left for the rest of the run; a later run removes run
//! directories more than a day old.
//!
//! Without a run id (`cargo test`, where one process runs the whole test
//! binary) the copy belongs to the process and is removed when it exits.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

/// The private copy's directory, dropped (and so removed) at process exit.
static DIR: Mutex<Option<tempfile::TempDir>> = Mutex::new(None);

/// The variable cargo-nextest sets, to the same value, in every test process
/// of one run.
const RUN_ID_VAR: &str = "NEXTEST_RUN_ID";

/// Run directories older than this are removed by a later run.
const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// Path of a current `amplihack` binary that nothing re-links while the
/// suite runs. A set `AMPLIHACK_TEST_BINARY` stops the test with a message,
/// because that binary is not the test-support build.
pub fn amplihack_bin() -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        if let Some(p) = std::env::var_os("AMPLIHACK_TEST_BINARY") {
            panic!(
                "AMPLIHACK_TEST_BINARY={} is unverified; use the protected test-support build",
                PathBuf::from(p).display()
            );
        }
        let run_id = std::env::var(RUN_ID_VAR)
            .ok()
            .filter(|id| run_id_is_plain(id));
        let shared = run_id.and_then(|id| {
            match run_copy(&copy_base(), &id, SystemTime::now(), build) {
                Ok(bin) => Some(bin),
                // No lock, no shared copy: the lock is what makes one. This
                // process makes its own copy, as every process did before.
                Err(RunCopyError::NoLock(e)) => {
                    eprintln!(
                        "note: no file lock for test run {id} ({e}); this test process builds and copies the amplihack binary itself"
                    );
                    None
                }
                Err(RunCopyError::Other(e)) => {
                    panic!("could not prepare the amplihack binary for test run {id}: {e}")
                }
            }
        });
        shared.unwrap_or_else(|| {
            let built_dir = build();
            private_copy(&built_dir).unwrap_or_else(|e| {
                panic!(
                    "could not copy the amplihack binary from {}: {e}",
                    built_dir.display()
                )
            })
        })
    })
    .clone()
}

/// Runs `cargo build -p amplihack` with the `amplihack-cli/test-support`
/// feature into a Cargo output folder of its own, and returns the directory
/// the binary was linked into (issue #2182).
fn build() -> PathBuf {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        .args([
            "build",
            "--quiet",
            "-p",
            "amplihack",
            "--bin",
            "amplihack",
            "--features",
            "amplihack-cli/test-support",
            "--target-dir",
        ])
        .arg(protected_target())
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .status()
        .expect("failed to run cargo build for the amplihack binary");
    assert!(status.success(), "cargo build -p amplihack failed");
    let built_dir = protected_target().join("debug");
    let built = built_dir.join("amplihack");
    assert!(
        built.is_file(),
        "no amplihack binary at {}",
        built.display()
    );
    built_dir
}

/// The Cargo output folder of the test-support build: a private temporary
/// directory, so the feature never reaches the build the developer's own
/// `target` holds, and removed when this process exits. The binaries a run
/// shares are copied out of it first.
fn protected_target() -> &'static Path {
    static TARGET: OnceLock<tempfile::TempDir> = OnceLock::new();
    TARGET
        .get_or_init(|| {
            let dir = tempfile::Builder::new()
                .prefix("amplihack-protected-build-")
                .tempdir()
                .expect("create isolated Cargo output");
            extern "C" fn cleanup() {
                if let Some(dir) = TARGET.get()
                    && let Err(error) = std::fs::remove_dir_all(dir.path())
                {
                    eprintln!(
                        "remove protected test build {}: {error}",
                        dir.path().display()
                    );
                }
            }
            // Static TempDirs are never dropped. Keep artifacts through child
            // completion, then release the dedicated build output at exit.
            assert_eq!(
                unsafe { libc::atexit(cleanup) },
                0,
                "register protected build cleanup"
            );
            dir
        })
        .path()
}

/// Where private copies are made: cargo's per-target temporary directory, or
/// the system one when the test was not built by cargo.
fn copy_base() -> PathBuf {
    option_env!("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

/// A run id is used in a directory name, so only a plain one is accepted:
/// letters, digits and hyphens, at most 64 bytes. Anything else gets the
/// per-process copy.
pub(crate) fn run_id_is_plain(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The copy shared by every test process of run `id`: `build` runs, and the
/// binaries are copied, in the first process to get here; the rest find the
/// copy and return it. `build` returns the directory holding the built
/// binaries. `now` is the time used to judge which older run directories
/// are stale.
///
/// The copy is complete when `amplihack` exists in the run directory: it is
/// renamed into place last, after `amplihack-hooks`, so a process that sees
/// it needs no lock.
pub(crate) fn run_copy(
    base: &Path,
    id: &str,
    now: SystemTime,
    build: impl FnOnce() -> PathBuf,
) -> Result<PathBuf, RunCopyError> {
    let dir = base.join(format!("{RUN_DIR_PREFIX}{id}"));
    let bin = dir.join("amplihack");
    if is_regular_file(&bin) {
        check_private_dir(&dir)?;
        return Ok(bin);
    }
    fs::create_dir_all(base)?;
    // The lock file sits beside the run directory, so the directory holds
    // the binaries and nothing else.
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(base.join(format!("{RUN_DIR_PREFIX}{id}.lock")))?;
    fs2::FileExt::lock_exclusive(&lock).map_err(RunCopyError::NoLock)?;
    if is_regular_file(&bin) {
        check_private_dir(&dir)?;
        return Ok(bin);
    }
    remove_stale_run_dirs(base, id, now);
    let built_dir = build();
    make_private_dir(&dir)?;
    // `amplihack-hooks` first and `amplihack` last: the fast path above
    // takes the presence of `amplihack` to mean the copy is complete.
    let hooks = built_dir.join("amplihack-hooks");
    if is_regular_file(&hooks) {
        copy_into(&hooks, &dir, "amplihack-hooks")?;
    }
    let bin = copy_into(&built_dir.join("amplihack"), &dir, "amplihack")?;
    remove_leftover_temporaries(&dir);
    Ok(bin)
}

/// Why [`run_copy`] gave no shared copy. Without a lock the caller can
/// still make a copy of its own; any other failure is a fault to report.
#[derive(Debug)]
pub(crate) enum RunCopyError {
    /// The file system would not give an exclusive lock.
    NoLock(io::Error),
    Other(io::Error),
}

impl From<io::Error> for RunCopyError {
    fn from(e: io::Error) -> Self {
        Self::Other(e)
    }
}

impl std::fmt::Display for RunCopyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoLock(e) => write!(f, "no file lock: {e}"),
            Self::Other(e) => write!(f, "{e}"),
        }
    }
}

/// Removes temporary files a stopped process left in the run directory.
/// Called with the lock held, after this process's own copies are in place.
fn remove_leftover_temporaries(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') && name.ends_with(".tmp") {
            let _ = fs::remove_file(entry.path());
        }
    }
}

const RUN_DIR_PREFIX: &str = "amplihack-bin-run-";

fn is_regular_file(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_file())
}

/// Creates `dir` if it is absent and makes it private to the owner. A
/// directory left half-made by a process that was stopped is reused; one
/// that is not this user's own directory is refused.
fn make_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        check_own_dir(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// A run directory found already made is used only if it is a real
/// directory (not a link) that this user owns and nobody else can enter.
/// The per-process copy got this from `tempfile`; a directory with a known
/// name has to be checked.
fn check_private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        check_own_dir(dir)?;
        let mode = fs::symlink_metadata(dir)?.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(io::Error::other(format!(
                "{} is open to other users (mode {mode:o})",
                dir.display()
            )));
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

#[cfg(unix)]
fn check_own_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::symlink_metadata(dir)?;
    if !meta.file_type().is_dir() {
        return Err(io::Error::other(format!(
            "{} is not a directory",
            dir.display()
        )));
    }
    // SAFETY: `geteuid` takes no arguments and cannot fail.
    let euid = unsafe { libc::geteuid() };
    if meta.uid() != euid {
        return Err(io::Error::other(format!(
            "{} belongs to another user",
            dir.display()
        )));
    }
    Ok(())
}

/// Removes the directories and lock files of other runs last changed more
/// than [`STALE_AFTER`] before `now`. A directory's time is the moment its
/// run copied the binaries in, at the start of the run. So a run keeps its
/// copy for a day; a test run that goes on for longer than that, or a clock
/// set forward by a day, lets another run remove the copy while it is still
/// in use. A failure to remove one is not this run's failure, so it is
/// ignored; `cargo clean` removes them all.
fn remove_stale_run_dirs(base: &Path, id: &str, now: SystemTime) {
    let Ok(entries) = fs::read_dir(base) else {
        return;
    };
    let own_dir = format!("{RUN_DIR_PREFIX}{id}");
    let own_lock = format!("{own_dir}.lock");
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(RUN_DIR_PREFIX) || name == own_dir || name == own_lock {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        let stale = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age > STALE_AFTER);
        if !stale {
            continue;
        }
        if meta.file_type().is_dir() {
            let _ = fs::remove_dir_all(entry.path());
        } else {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Copies `amplihack`, and `amplihack-hooks` when it is a regular file, from
/// `built_dir` into a new private directory; the path of the `amplihack`
/// copy.
fn private_copy(built_dir: &Path) -> io::Result<PathBuf> {
    let base = copy_base();
    fs::create_dir_all(&base)?;
    let dir = tempfile::Builder::new()
        .prefix("amplihack-bin-")
        .tempdir_in(&base)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700))?;
    }
    let hooks = built_dir.join("amplihack-hooks");
    if is_regular_file(&hooks) {
        copy_into(&hooks, dir.path(), "amplihack-hooks")?;
    }
    let bin = copy_into(&built_dir.join("amplihack"), dir.path(), "amplihack")?;
    *DIR.lock().unwrap_or_else(|p| p.into_inner()) = Some(dir);
    remove_at_exit();
    Ok(bin)
}

/// Copies through a child so sibling forks cannot inherit a writable
/// executable descriptor. Publish by rename only after the copier exits.
/// No fsync is needed for a private copy that need not survive a crash.
fn copy_into(src: &Path, dir: &Path, name: &str) -> io::Result<PathBuf> {
    // Named for this process, so two copiers can never write one file.
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let dest = dir.join(name);
    #[cfg(unix)]
    {
        let status = Command::new("/bin/cp")
            .arg("--")
            .arg(src)
            .arg(&tmp)
            .status()?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "copying {} failed: {status}",
                src.display()
            )));
        }
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755))?;
    }
    // Windows has no sibling fork that could inherit a write descriptor.
    #[cfg(not(unix))]
    fs::copy(src, &tmp)?;
    fs::rename(&tmp, &dest)?;
    Ok(dest)
}

/// Removes the private directory when the test process exits: statics are
/// never dropped, so without this every run would leave a binary behind.
fn remove_at_exit() {
    #[cfg(unix)]
    {
        extern "C" fn remove() {
            // `try_lock`: never wait at exit on a thread that holds `DIR`.
            if let Ok(mut dir) = DIR.try_lock() {
                drop(dir.take());
            }
        }
        // SAFETY: `remove` is a plain `extern "C"` function with no
        // arguments, as `atexit` requires, and it is registered once (the
        // caller runs inside a `OnceLock` initialiser).
        let rc = unsafe { libc::atexit(remove) };
        if rc != 0 {
            eprintln!(
                "warning: could not register removal of the private amplihack copy; it stays under the target tmp dir"
            );
        }
    }
}
