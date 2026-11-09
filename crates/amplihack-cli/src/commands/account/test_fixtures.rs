//! Shared fixtures for the issue #2006 unit tests of `commands::account`.
//!
//! Wired from `account/mod.rs` as `#[cfg(test)] mod test_fixtures;`.
//!
//! - [`FakeProc`]: a fake `/proc` tree under a temporary directory, in the
//!   layout `auto_switch/tests_liveness.rs` uses (`status`, `stat`,
//!   `environ`, `cmdline`), plus `self/environ`.
//! - [`FifoTrap`]: a named pipe that records whether anything opened it for
//!   reading. The check is an event, not a timer: the trap's writer thread
//!   sets its flag after writing and before closing, and a reader cannot see
//!   end-of-file until the writer closes, so any read that has finished has
//!   already set the flag.
//! - [`Homes`]: three canonical fixture account homes, `a`, `b` and `c`.

#![allow(dead_code)]

use std::fs;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub const SELF_PID: u32 = 4242;
pub const UID: u32 = 1000;
pub const OTHER_UID: u32 = 2000;
/// The fixed clock every test uses: 2026-11-09T00:00:00Z.
pub const NOW: i64 = 1_794_182_400;
/// A value that must never appear in any output, warning or lease.
pub const SECRET: &str = "SENTINEL-SECRET-2006";

pub fn private_tempdir() -> tempfile::TempDir {
    let dir = tempfile::Builder::new()
        .prefix("account-pick-")
        .tempdir()
        .unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

/// Three canonical account homes `<tmp>/homes/{a,b,c}`.
pub struct Homes {
    pub _tmp: tempfile::TempDir,
    pub root: PathBuf,
}

impl Homes {
    pub fn new() -> Self {
        let tmp = private_tempdir();
        let root = tmp.path().join("homes");
        for n in ["a", "b", "c"] {
            fs::create_dir_all(root.join(n)).unwrap();
        }
        let root = root.canonicalize().unwrap();
        Self { _tmp: tmp, root }
    }

    pub fn home(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    pub fn all(&self) -> Vec<PathBuf> {
        ["a", "b", "c"].iter().map(|n| self.home(n)).collect()
    }
}

pub struct FakeProc {
    pub _tmp: tempfile::TempDir,
    pub root: PathBuf,
}

impl FakeProc {
    pub fn new() -> Self {
        let tmp = private_tempdir();
        let root = tmp.path().join("proc");
        fs::create_dir_all(root.join("self")).unwrap();
        fs::write(root.join("self/environ"), b"HOME=/h\0").unwrap();
        Self { _tmp: tmp, root }
    }

    /// A process `pid` with parent `ppid`, real uid `uid`, state `state`
    /// (`S` or `Z`) and environment `env`. `stat` names the command
    /// `(co) dex)` so a parser that splits on the first `)` gets the
    /// parent wrong.
    pub fn process(&self, pid: u32, ppid: u32, uid: u32, state: char, env: &[&[u8]]) -> PathBuf {
        let dir = self.root.join(pid.to_string());
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("status"),
            format!(
                "Name:\tcodex\nState:\t{state} (x)\nPPid:\t{ppid}\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n"
            ),
        )
        .unwrap();
        fs::write(
            dir.join("stat"),
            format!("{pid} (co) dex) {state} {ppid} {pid} {pid} 0 -1 4194304 0 0 0 0\n"),
        )
        .unwrap();
        let mut environ = Vec::new();
        for e in env {
            environ.extend_from_slice(e);
            environ.push(0);
        }
        fs::write(dir.join("environ"), environ).unwrap();
        fs::write(dir.join("cmdline"), b"codex\0exec\0").unwrap();
        dir
    }

    /// A process of `uid` whose `environ` is a [`FifoTrap`].
    pub fn trapped_process(&self, pid: u32, ppid: u32, uid: u32) -> FifoTrap {
        let dir = self.process(pid, ppid, uid, 'S', &[]);
        fs::remove_file(dir.join("environ")).unwrap();
        FifoTrap::new(&dir.join("environ"), b"CODEX_HOME=/x\0")
    }

    /// A process whose `environ` cannot be read (mode 000).
    pub fn unreadable_process(&self, pid: u32, ppid: u32, uid: u32, env: &[&[u8]]) {
        let dir = self.process(pid, ppid, uid, 'S', env);
        fs::set_permissions(dir.join("environ"), fs::Permissions::from_mode(0o000)).unwrap();
    }

    pub fn remove(&self, pid: u32) {
        fs::remove_dir_all(self.root.join(pid.to_string())).unwrap();
    }
}

/// `KEY=<path>` as bytes.
pub fn kv(key: &str, path: &Path) -> Vec<u8> {
    let mut v = format!("{key}=").into_bytes();
    v.extend_from_slice(path.as_os_str().as_bytes());
    v
}

pub struct FifoTrap {
    path: PathBuf,
    opened: Arc<AtomicBool>,
    writer: Option<std::thread::JoinHandle<()>>,
}

impl FifoTrap {
    pub fn new(path: &Path, payload: &'static [u8]) -> Self {
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: `c` is a valid NUL-terminated path.
        let rc = unsafe { libc::mkfifo(c.as_ptr(), 0o600) };
        assert_eq!(rc, 0, "mkfifo {}", path.display());
        let opened = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&opened);
        let p = path.to_path_buf();
        let writer = std::thread::spawn(move || {
            // Blocks until some reader opens the pipe.
            if let Ok(mut w) = fs::OpenOptions::new().write(true).open(&p) {
                let _ = w.write_all(payload);
                flag.store(true, Ordering::SeqCst);
                drop(w);
            }
        });
        Self {
            path: path.to_path_buf(),
            opened,
            writer: Some(writer),
        }
    }

    /// Whether anything has opened the pipe for reading so far.
    pub fn was_opened(&self) -> bool {
        self.opened.load(Ordering::SeqCst)
    }

    pub fn assert_never_opened(&self, what: &str) {
        assert!(
            !self.was_opened(),
            "{what}: {} was opened for reading",
            self.path.display()
        );
    }
}

impl Drop for FifoTrap {
    fn drop(&mut self) {
        // Release a writer still waiting for a reader, then join it. If the
        // pipe is already gone nothing can release the writer, so it is left
        // detached rather than joined (a join would never return).
        let released = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&self.path)
            .is_ok();
        if let Some(h) = self.writer.take()
            && released
        {
            let _ = h.join();
        }
    }
}
