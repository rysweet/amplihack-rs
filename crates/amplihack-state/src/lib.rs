//! File operations, locking, and environment configuration.
//!
//! Provides the infrastructure layer for hooks: atomic file operations,
//! timeout-based locking, and typed environment variable parsing.

/// Atomic JSON file operations with crash-safe writes.
pub mod atomic_json;
/// Atomic counter for metrics and sequencing.
pub mod counter;
/// Typed environment variable parsing and configuration.
pub mod env_config;
/// Timeout-based file locking for session coordination.
pub mod file_lock;
/// Atomic flag (semaphore) for single-writer coordination.
pub mod semaphore;

/// Atomic JSON file operations with crash-safe writes.
pub use atomic_json::AtomicJsonFile;
/// Atomic counter for metrics and sequencing.
pub use counter::AtomicCounter;
/// Typed environment variable parsing.
pub use env_config::EnvVar;
/// Timeout-based file lock.
pub use file_lock::FileLock;
/// Atomic boolean flag for coordination.
pub use semaphore::AtomicFlag;

#[cfg(all(test, unix))]
mod permission_tests {
    use std::os::unix::{fs::MetadataExt, process::CommandExt};
    use std::path::Path;
    use std::process::Command;

    pub(crate) fn mode(path: &Path) -> u32 {
        std::fs::symlink_metadata(path).unwrap().mode() & 0o777
    }

    /// Re-exec just this test with umask 0002. The parent test runner's umask
    /// is never modified, even while sibling tests create files concurrently.
    pub(crate) fn under_umask_0002(name: &str, body: impl FnOnce()) {
        under_umask(name, 0o002, body);
    }

    pub(crate) fn under_umask(name: &str, mask: libc::mode_t, body: impl FnOnce()) {
        const CHILD: &str = "AMPLIHACK_STATE_PERMISSION_TEST_CHILD";
        let parent = std::os::unix::process::parent_id().to_string();
        if std::env::var(CHILD).as_deref() == Ok(parent.as_str()) {
            body();
            return;
        }
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", name, "--test-threads=1", "--nocapture"])
            .env(CHILD, std::process::id().to_string());
        // SAFETY: only umask(2) runs between fork and exec, in the child.
        unsafe {
            command.pre_exec(move || {
                libc::umask(mask);
                Ok(())
            });
        }
        let output = command.output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "{name}: {}\n{stdout}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[cfg(all(test, unix))]
mod tests_private_umask;
