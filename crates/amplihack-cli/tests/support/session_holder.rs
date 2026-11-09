//! Issue #2040 (D9): a process that stands in for a running Claude session.
//!
//! The liveness scan reads a process's environment only when its `exe`
//! names Claude or amplihack (or is `node`, `bun` or `deno` running it), so a
//! plain `/bin/sleep` with `CLAUDE_CONFIG_DIR` set no longer counts. The
//! holder is a copy of `sleep` in a directory named `claude-session`, made
//! once per test process. A symlink would not do: the kernel's
//! `/proc/<pid>/exe` names the resolved file.

#![cfg(unix)]

use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The `sleep` the tests ran before, found on the usual paths.
fn system_sleep() -> PathBuf {
    ["/bin/sleep", "/usr/bin/sleep"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
        .expect("a sleep binary in /bin or /usr/bin")
}

/// `<temp>/claude-session/sleep`: a copy of `sleep`, mode `0700`, removed
/// with its temp dir when the process exits.
pub fn claude_sleep() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let base = tempfile::Builder::new()
            .prefix("amplihack-test-holder-")
            .tempdir()
            .expect("create the holder temp dir")
            .keep();
        let dir = base.join("claude-session");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .expect("create claude-session/");
        let bin = dir.join("sleep");
        std::fs::copy(system_sleep(), &bin).expect("copy sleep");
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700))
            .expect("make the holder executable");
        extern "C" fn remove_at_exit() {
            if let Some(bin) = BIN.get()
                && let Some(base) = bin.parent().and_then(Path::parent)
            {
                let _ = std::fs::remove_dir_all(base);
            }
        }
        // SAFETY: registering a plain `extern "C"` function has no
        // preconditions; it runs once, at normal process exit.
        unsafe { libc::atexit(remove_at_exit) };
        bin
    })
}
