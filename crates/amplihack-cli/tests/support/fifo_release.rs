//! Issue #2582: hold a process tree live on a FIFO while a test walks it,
//! with no sleep, deadline, poll or timer anywhere.
//!
//! A test writes a stub (a runner, an agent CLI) that opens this FIFO for
//! reading and blocks. [`start`] spawns the command and opens the FIFO for
//! writing, which returns only once the stub has opened the read end: that
//! open is the rendezvous, so the tree is live and complete when `start`
//! returns.
//!
//! A watcher thread waits on the child. If the child exits before the stub
//! ever blocks, the watcher opens the read end itself (non-blocking), so the
//! test's open returns instead of hanging and the test fails with the child's
//! own output. [`Live`]'s `Drop` writes the release byte and joins the
//! watcher — which has already reaped the child — on every way out, panics
//! included.
//!
//! Nothing here sends a signal: every process is stopped by letting it finish.
//! A test that hangs on one of these opens has a defect in the stub, and
//! wrapping it in a `timeout` is how a shared process group comes to be
//! signalled (#2674).

#![allow(dead_code)]

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use super::proc_tree;

/// Creates a 0600 FIFO at `path`. The parent folder must exist.
pub fn mkfifo(path: &Path) {
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: `c` is a valid NUL-terminated path for the lifetime of the call.
    let rc = unsafe { libc::mkfifo(c.as_ptr(), 0o600) };
    assert_eq!(
        rc,
        0,
        "mkfifo {}: {}",
        path.display(),
        std::io::Error::last_os_error()
    );
}

/// A spawned command whose stub is blocked on `fifo`. Dropping it releases
/// the stub and reaps the child.
pub struct Live {
    pid: u32,
    fifo: PathBuf,
    writer: Option<File>,
    watcher: Option<JoinHandle<(Output, Option<File>)>>,
    output: Option<Output>,
}

/// Spawns `command` and returns once its stub has opened `fifo` for reading.
///
/// `what` names the command in the panic message. Panics with the child's
/// status, stdout and stderr if it exits before the stub blocks.
pub fn start(mut command: Command, fifo: &Path, what: &str) -> Live {
    let child = command
        .spawn()
        .unwrap_or_else(|e| panic!("start {what}: {e}"));
    let pid = child.id();
    let exited = Arc::new(AtomicBool::new(false));
    let watcher = {
        let exited = Arc::clone(&exited);
        let fifo = fifo.to_path_buf();
        std::thread::spawn(move || {
            let output = child
                .wait_with_output()
                .expect("wait for the spawned command");
            exited.store(true, Ordering::SeqCst);
            // Release a test blocked in its open for writing. Non-blocking,
            // so this never waits for a writer; held until joined.
            let reader = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&fifo)
                .ok();
            (output, reader)
        })
    };
    let mut live = Live {
        pid,
        fifo: fifo.to_path_buf(),
        writer: None,
        watcher: Some(watcher),
        output: None,
    };
    let writer = OpenOptions::new()
        .write(true)
        .open(fifo)
        .unwrap_or_else(|e| panic!("open the release FIFO {}: {e}", fifo.display()));
    live.writer = Some(writer);
    if exited.load(Ordering::SeqCst) {
        let output = live.finish();
        panic!(
            "{what} exited before its stub blocked:\n{}",
            describe(&output)
        );
    }
    live
}

impl Live {
    /// The spawned child's PID: the root of the walk.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// The live process tree under the child, root first.
    pub fn tree(&self) -> Vec<proc_tree::Process> {
        proc_tree::descendants(self.pid)
    }

    /// Release the stub, wait for the child to exit, and return its output.
    pub fn finish(&mut self) -> Output {
        if let Some(mut writer) = self.writer.take() {
            let _ = writer.write_all(b"\n");
        }
        if let Some(watcher) = self.watcher.take() {
            let (output, _reader) = watcher.join().expect("the watcher thread");
            self.output = Some(output);
        }
        self.output.clone().expect("the child's output")
    }

    /// The FIFO this run is released through.
    pub fn fifo(&self) -> &Path {
        &self.fifo
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        if self.watcher.is_some() {
            let _ = self.finish();
        }
    }
}

/// A child's status and streams, for a panic message.
pub fn describe(out: &Output) -> String {
    format!(
        "status: {}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Every process in `tree` as `pid: [argv...]`, one per line.
pub fn show(tree: &[proc_tree::Process]) -> String {
    tree.iter()
        .map(|(pid, argv)| {
            let argv: Vec<String> = argv
                .iter()
                .map(|a| String::from_utf8_lossy(a).into_owned())
                .collect();
            format!("{pid}: {argv:?}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}
