//! Issue #2042: a private fake process table for one test, read by the
//! `amplihack` binary through `AMPLIHACK_TEST_PROC_ROOT` (`PROC_ROOT_ENV`).
//!
//! The tree is a `tempfile` directory under `std::env::temp_dir()`, created
//! with mode 0700 whatever the umask and owned by this user, so it meets
//! every condition of the binary's seam (`procfs::resolve`, S27). It
//! always holds `self/environ` = `AMPLIHACK_FAKE=1\0` (`seed_proc_tree`, shared
//! with the empty tree in `isolated_amplihack.rs`); without it the
//! liveness scan reports "unsupported" and every profile counts as live.
//!
//! Static pids lie in `FAKE_PID_BASE..=i32::MAX`. `FAKE_PID_BASE` is one
//! above the largest Linux `pid_max`, so a signal sent to a fake pid fails
//! with `ESRCH` and never reaches a host process; the upper bound stops the
//! `as pid_t` wrap to a negative value, which `kill` reads as a process group.
//!
//! [`FakeProc::link_own`] links one real pid into the tree. Pass only a pid
//! the test started (`child.id()`, its own pid file, or a `started (pid N)`
//! line), and keep a spawned `Child` unreaped until the assertions are done.
//! The rotator's owner check sees the owner of the link, not the process;
//! that is acceptable only because the process is the test's own.
//!
//! Every I/O failure panics. Nothing here reads a real process's `environ`.

#![cfg(target_os = "linux")]
#![allow(dead_code)]

use std::fs::{DirBuilder, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// One above the largest Linux `pid_max` (2^22).
pub const FAKE_PID_BASE: u32 = 4_194_305;

pub use crate::isolated_amplihack::SELF_ENVIRON;

fn private_dir(path: &Path) {
    DirBuilder::new()
        .mode(0o700)
        .create(path)
        .unwrap_or_else(|e| panic!("create {}: {e}", path.display()));
}

fn private_file(path: &Path, body: &[u8]) {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut f| f.write_all(body))
        .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

/// A fake `/proc` owned by one test, removed when dropped.
pub struct FakeProc {
    dir: tempfile::TempDir,
}

impl Default for FakeProc {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeProc {
    pub fn new() -> Self {
        Self::new_in(std::env::temp_dir())
    }

    /// A tree inside `parent`, for a test that gives the child a `TMPDIR`
    /// of its own: the seam honours only a tree inside the child's temp dir,
    /// so a tree under this process's temp dir would be refused and the
    /// child would read the host `/proc`.
    pub fn new_in(parent: impl AsRef<Path>) -> Self {
        let dir = tempfile::Builder::new()
            .prefix("amplihack-fake-proc-")
            // Made 0700 at creation: the default mode follows the umask, and
            // a group-writable tree could be changed after the seam checks it.
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(parent)
            .expect("create the fake proc root");
        crate::isolated_amplihack::seed_proc_tree(dir.path());
        Self { dir }
    }

    /// The tree's root, for `PROC_ROOT_ENV`.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// A static entry `<root>/<pid>/`. Panics on a pid outside
    /// `FAKE_PID_BASE..=i32::MAX`.
    pub fn process(&self, pid: u32) -> FakeEntry {
        assert!(
            (FAKE_PID_BASE..=i32::MAX as u32).contains(&pid),
            "fake pid {pid} must lie in {FAKE_PID_BASE}..={}",
            i32::MAX
        );
        let dir = self.path().join(pid.to_string());
        private_dir(&dir);
        FakeEntry { dir }
    }

    /// `<root>/<pid> -> /proc/<pid>`, for a process the test started.
    pub fn link_own(&self, pid: u32) {
        assert!(
            pid > 1 && pid != std::process::id(),
            "link_own: pid {pid} is not one this test started"
        );
        let link = self.path().join(pid.to_string());
        std::os::unix::fs::symlink(format!("/proc/{pid}"), &link)
            .unwrap_or_else(|e| panic!("link {}: {e}", link.display()));
    }

    /// Deletes an entry or link. A missing entry counts as removed.
    pub fn remove(&self, pid: u32) {
        let path = self.path().join(pid.to_string());
        let result = match std::fs::symlink_metadata(&path) {
            Ok(m) if m.is_dir() => std::fs::remove_dir_all(&path),
            Ok(_) => std::fs::remove_file(&path),
            Err(e) => Err(e),
        };
        match result {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                panic!("remove {}: {e}", path.display())
            }
            _ => {}
        }
    }

    /// `self/cgroup`, for rotation-owner tests.
    pub fn self_cgroup(&self, text: &str) {
        private_file(&self.path().join("self/cgroup"), text.as_bytes());
    }
}

/// One static process directory in a [`FakeProc`].
pub struct FakeEntry {
    dir: PathBuf,
}

impl FakeEntry {
    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// `cmdline`: each argument followed by NUL.
    pub fn cmdline(&self, args: &[&str]) -> &Self {
        let mut raw = Vec::new();
        for a in args {
            assert!(!a.contains('\0'), "an argument holds a NUL");
            raw.extend_from_slice(a.as_bytes());
            raw.push(0);
        }
        private_file(&self.dir.join("cmdline"), &raw);
        self
    }

    /// `status` with `State:` set to `state` and `Uid:` to the euid.
    pub fn status(&self, state: char) -> &Self {
        assert!(state.is_ascii_uppercase(), "state {state:?} is one letter");
        let uid = euid();
        let body =
            format!("Name:\tfake\nState:\t{state} (fake)\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n");
        private_file(&self.dir.join("status"), body.as_bytes());
        self
    }

    /// `environ`: each `key=value` followed by NUL.
    pub fn environ(&self, vars: &[(&str, &str)]) -> &Self {
        let mut raw = Vec::new();
        for (k, v) in vars {
            assert!(
                !k.is_empty() && !k.contains('=') && !k.contains('\0'),
                "bad environ key"
            );
            assert!(!v.contains('\0'), "an environ value holds a NUL");
            raw.extend_from_slice(format!("{k}={v}").as_bytes());
            raw.push(0);
        }
        private_file(&self.dir.join("environ"), &raw);
        self
    }

    /// `exe` as a symlink to `target`.
    pub fn exe_link(&self, target: impl AsRef<Path>) -> &Self {
        let link = self.dir.join("exe");
        std::os::unix::fs::symlink(target.as_ref(), &link)
            .unwrap_or_else(|e| panic!("link {}: {e}", link.display()));
        self
    }
}
