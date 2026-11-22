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
/// suite runs. `AMPLIHACK_TEST_BINARY` overrides it and is used as given.
pub fn amplihack_bin() -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        if let Some(p) = std::env::var_os("AMPLIHACK_TEST_BINARY") {
            return PathBuf::from(p);
        }
        let run_id = std::env::var(RUN_ID_VAR)
            .ok()
            .filter(|id| run_id_is_plain(id));
        match run_id {
            Some(id) => run_copy(&copy_base(), &id, SystemTime::now(), build).unwrap_or_else(|e| {
                panic!("could not prepare the amplihack binary for test run {id}: {e}")
            }),
            None => {
                let built_dir = build();
                private_copy(&built_dir).unwrap_or_else(|e| {
                    panic!(
                        "could not copy the amplihack binary from {}: {e}",
                        built_dir.display()
                    )
                })
            }
        }
    })
    .clone()
}

/// Runs `cargo build -p amplihack` and returns the directory the binary was
/// linked into.
fn build() -> PathBuf {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        .args(["build", "--quiet", "-p", "amplihack", "--bin", "amplihack"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .status()
        .expect("failed to run cargo build for the amplihack binary");
    assert!(status.success(), "cargo build -p amplihack failed");
    let built_dir = std::env::current_exe()
        .unwrap()
        .parent() // deps
        .and_then(Path::parent) // debug
        .unwrap()
        .to_path_buf();
    let built = built_dir.join("amplihack");
    assert!(
        built.is_file(),
        "no amplihack binary at {}",
        built.display()
    );
    built_dir
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
) -> io::Result<PathBuf> {
    let dir = base.join(format!("{RUN_DIR_PREFIX}{id}"));
    let bin = dir.join("amplihack");
    if is_regular_file(&bin) {
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
    fs2::FileExt::lock_exclusive(&lock)?;
    if is_regular_file(&bin) {
        return Ok(bin);
    }
    remove_stale_run_dirs(base, id, now);
    let built_dir = build();
    make_private_dir(&dir)?;
    let hooks = built_dir.join("amplihack-hooks");
    if is_regular_file(&hooks) {
        copy_into(&hooks, &dir, "amplihack-hooks")?;
    }
    copy_into(&built_dir.join("amplihack"), &dir, "amplihack")
}

const RUN_DIR_PREFIX: &str = "amplihack-bin-run-";

fn is_regular_file(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_file())
}

/// Creates `dir` if it is absent and makes it private to the owner.
fn make_private_dir(dir: &Path) -> io::Result<()> {
    match fs::create_dir(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Removes the directories and lock files of other runs last changed more
/// than [`STALE_AFTER`] before `now`. A run still going keeps its copy: its
/// directory is younger than that. A failure to remove one is not this
/// run's failure, so it is ignored; `cargo clean` removes them all.
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
    let bin = copy_into(&built_dir.join("amplihack"), dir.path(), "amplihack")?;
    let hooks = built_dir.join("amplihack-hooks");
    if fs::symlink_metadata(&hooks).is_ok_and(|m| m.file_type().is_file()) {
        copy_into(&hooks, dir.path(), "amplihack-hooks")?;
    }
    *DIR.lock().unwrap_or_else(|p| p.into_inner()) = Some(dir);
    remove_at_exit();
    Ok(bin)
}

/// Copies through a child so sibling forks cannot inherit a writable
/// executable descriptor. Publish by rename only after the copier exits.
/// No fsync is needed for a private copy that need not survive a crash.
fn copy_into(src: &Path, dir: &Path, name: &str) -> io::Result<PathBuf> {
    let tmp = dir.join(format!(".{name}.tmp"));
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
