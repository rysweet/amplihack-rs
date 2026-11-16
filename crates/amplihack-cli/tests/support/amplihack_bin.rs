//! The `amplihack` binary, built once for the whole `cli_integration` binary
//! and copied to a private directory (issue #2019, race A).
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
//! left a regular file, into a fresh private temporary directory, and every
//! caller gets that copy. Nothing re-links it during the run. The directory
//! is removed when the process exits.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

/// The private copy's directory, dropped (and so removed) at process exit.
static DIR: Mutex<Option<tempfile::TempDir>> = Mutex::new(None);

/// Path of a current `amplihack` binary that nothing re-links while the
/// suite runs. `AMPLIHACK_TEST_BINARY` overrides it and is used as given.
pub fn amplihack_bin() -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        if let Some(p) = std::env::var_os("AMPLIHACK_TEST_BINARY") {
            return PathBuf::from(p);
        }
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
        private_copy(&built_dir).unwrap_or_else(|e| {
            panic!(
                "could not copy the amplihack binary from {}: {e}",
                built_dir.display()
            )
        })
    })
    .clone()
}

/// Copies `amplihack`, and `amplihack-hooks` when it is a regular file, from
/// `built_dir` into a new private directory; the path of the `amplihack`
/// copy.
fn private_copy(built_dir: &Path) -> io::Result<PathBuf> {
    let base = option_env!("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
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
