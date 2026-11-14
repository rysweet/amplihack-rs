//! Issue #1764: no git that a test, or `amplihack` itself, starts may wait on
//! a credential prompt.
//!
//! When a remote asks for a credential and nothing supplies one, git opens
//! `/dev/tty` and asks. Plain `timeout` moves the command into a background
//! process group, so that read stops git on `SIGTTIN`. Nothing fails: the run
//! waits until something kills it, and with uutils `timeout`, which is stopped
//! too, nothing does. CI runners have no terminal, so there git failed with
//! `No such device or address` instead and the trap went unseen.
//!
//! What is pinned here (see `docs/howto/keep-tests-from-prompting-for-git.md`):
//!
//! - (a) `.cargo/config.toml` holds only an `[env]` table that puts
//!   `GIT_TERMINAL_PROMPT=0`, `GIT_ASKPASS=true` and `SSH_ASKPASS=true` into
//!   the test process, and `isolated(..)` sets them again after its
//!   `env_clear()`;
//! - (b) a git started through `amplihack_git::command()`, with no askpass
//!   program and no terminal, exits by itself with `terminal prompts
//!   disabled`;
//! - (c) under `script` and plain `timeout`, in a compound command so that the
//!   trap is armed, git with none of the variables prints the username prompt
//!   and is seen stopped on `SIGTTIN` (the control), while git with
//!   `GIT_TERMINAL_PROMPT=0` alone (c1) or with all three (c2) exits by itself
//!   without prompting;
//! - (d) the three recipe prompts that run long commands tell the agent how to
//!   run them so that a prompt cannot stop them, without a wall-clock
//!   `timeout` (issue #2022), and step 12's evidence schema is unchanged;
//! - the how-to is linked and offers no remedy that weakens TLS, host-key or
//!   credential-storage settings.
//!
//! Every git here talks only to [`AlwaysUnauthorised`], a server on
//! `127.0.0.1` that answers `401`, from a temporary `HOME` with
//! `GIT_CONFIG_GLOBAL=/dev/null` and `GIT_CONFIG_NOSYSTEM=1`, so no stored
//! credential can reach it. Each URL names its server's port and this
//! process's id (`http://127.0.0.1:<port>/issue-1764-<pid>.git`), and that URL
//! is the marker by which the test finds every process a case started.
//!
//! The test waits only on events (#2099). The prompt is seen as bytes on the
//! pipe from `script`; the control's stop on `SIGTTIN` is reported by the
//! kernel to a `ptrace` tracer; a case that does not prompt is judged by the
//! exit of its child. When a case ends, on success, failure or panic, an
//! [`Owned`] guard opens a pidfd for each process of this user whose
//! arguments contain the marker, sends `SIGKILL` through it, waits until the
//! pidfd reports the exit and scans again until none is left.

use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_yaml::Value;

use crate::auto_drive_to_merge::BRICK_LINE_BUDGET;
use crate::isolated_amplihack::{NO_GIT_PROMPT_ENV, isolated};

/// Proxy settings that would send git's request somewhere other than the
/// loopback server.
const PROXY_VARS: [&str; 6] = [
    "http_proxy",
    "HTTP_PROXY",
    "https_proxy",
    "HTTPS_PROXY",
    "all_proxy",
    "ALL_PROXY",
];

/// Settings that must never be offered as a way round a credential prompt
/// (SR10): they weaken TLS, host-key checking or credential storage.
const WEAKENING: [&str; 6] = [
    "GIT_SSL_NO_VERIFY",
    "sslVerify",
    "StrictHostKeyChecking",
    "UserKnownHostsFile",
    "helper store",
    "helper=store",
];

// ── Helpers ──────────────────────────────────────────────────────────────────

/// A git remote on `127.0.0.1` that refuses every request with `401` and
/// `WWW-Authenticate: Basic`, so git asks for a credential. It reads at most
/// 8 KiB of each request, waits at most 5 s for it, and never prints it, so
/// nothing git sends reaches the test output.
struct AlwaysUnauthorised {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

const UNAUTHORISED: &[u8] = b"HTTP/1.1 401 Unauthorized\r\n\
WWW-Authenticate: Basic realm=\"issue-1764\"\r\n\
Content-Length: 0\r\n\
Connection: close\r\n\
\r\n";

impl AlwaysUnauthorised {
    fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind 127.0.0.1:0");
        let port = listener.local_addr().expect("listener address").port();
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(mut stream) = stream else { continue };
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                    // Read the whole request head before answering: closing a
                    // socket with unread data resets the connection, and git
                    // would then fail without ever asking for a credential.
                    let mut head = Vec::new();
                    let mut chunk = [0u8; 1024];
                    while head.len() < 8192 && !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut chunk) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => head.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let _ = stream.write_all(UNAUTHORISED);
                }
            })
        };
        Self {
            port,
            stop,
            thread: Some(thread),
        }
    }

    fn url(&self) -> String {
        format!(
            "http://127.0.0.1:{}/issue-1764-{}.git",
            self.port,
            std::process::id()
        )
    }
}

impl Drop for AlwaysUnauthorised {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the blocked accept so the thread sees the flag.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

// ── Process events ───────────────────────────────────────────────────────────
//
// The unsafe code in this file is in the five wrappers below, the geteuid
// call in `owned_by_me` and the `pre_exec` hook in
// `issue_2099_sigttin_seen_reports_the_stop`. Each wrapper reads `errno` straight after its call.

/// Open a pidfd for `pid`. ESRCH means the process has already gone.
#[cfg(target_os = "linux")]
fn pidfd_open(pid: libc::pid_t) -> std::io::Result<std::os::fd::OwnedFd> {
    use std::os::fd::{FromRawFd, OwnedFd, RawFd};
    // SAFETY: pidfd_open takes a PID and a flags word and touches no memory
    // of this process.
    let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if raw < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `raw` is a descriptor the kernel has just created for this
    // call, so nothing else owns it.
    Ok(unsafe { OwnedFd::from_raw_fd(raw as RawFd) })
}

/// Send `signal` through `pidfd`, which names exactly the process it was
/// opened for, so a reused PID cannot receive it.
#[cfg(target_os = "linux")]
fn pidfd_send_signal(pidfd: &std::os::fd::OwnedFd, signal: libc::c_int) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    // SAFETY: the descriptor is open for the duration of the call, the
    // siginfo pointer is null (the kernel fills it in as for kill) and the
    // flags are 0.
    let rc = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            pidfd.as_raw_fd(),
            signal,
            std::ptr::null::<libc::siginfo_t>(),
            0u32,
        )
    };
    if rc < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Block until `pidfd` reports that its process has exited (POLLIN). A
/// signal that interrupts the wait repeats the same call.
#[cfg(target_os = "linux")]
fn poll_exit(pidfd: &std::os::fd::OwnedFd) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    loop {
        let mut entry = libc::pollfd {
            fd: pidfd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `entry` is a valid, writable pollfd for the duration of the
        // call, and the count is 1.
        let rc = unsafe { libc::poll(&mut entry, 1, -1) };
        if rc >= 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// `PTRACE_SEIZE` (options 0) or `PTRACE_DETACH` on `pid`, with `signal` as
/// the data word of the detach.
#[cfg(target_os = "linux")]
fn ptrace_request(
    request: libc::c_uint,
    pid: libc::pid_t,
    signal: libc::c_int,
) -> std::io::Result<()> {
    // SAFETY: neither request reads or writes memory through the address or
    // data words: the address is null, and the data word is 0 for
    // PTRACE_SEIZE and a signal number for the detach.
    let rc = unsafe {
        libc::ptrace(
            request as _,
            pid,
            std::ptr::null_mut::<libc::c_void>(),
            signal as libc::c_long as *mut libc::c_void,
        )
    };
    if rc < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// `waitpid(pid, __WALL)` for one specific traced process. A signal that
/// interrupts the wait repeats the same call.
#[cfg(target_os = "linux")]
fn waitpid_wall(pid: libc::pid_t) -> std::io::Result<libc::c_int> {
    loop {
        let mut status: libc::c_int = 0;
        // SAFETY: `status` is a valid, writable int for the duration of the
        // call, and `pid` names one process, never a group or any child.
        let rc = unsafe { libc::waitpid(pid, &mut status, libc::__WALL) };
        if rc == pid {
            return Ok(status);
        }
        if rc >= 0 {
            return Err(std::io::Error::other(format!(
                "waitpid({pid}, __WALL) returned {rc}"
            )));
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(std::io::Error::other(format!(
                "waitpid({pid}, __WALL) failed: {error}"
            )));
        }
    }
}

// ── Cleanup by marker ────────────────────────────────────────────────────────

/// The arguments of `pid`, joined by spaces, if it can still be read.
/// Processes that have exited (zombies) have none.
#[cfg(target_os = "linux")]
fn cmdline_of(pid: libc::pid_t) -> Option<String> {
    let raw = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    Some(String::from_utf8_lossy(&raw).replace('\0', " "))
}

/// Whether `/proc/<pid>` belongs to this user.
#[cfg(target_os = "linux")]
fn owned_by_me(pid: libc::pid_t) -> bool {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: geteuid has no arguments and cannot fail.
    let me = unsafe { libc::geteuid() };
    fs::metadata(format!("/proc/{pid}")).is_ok_and(|meta| meta.uid() == me)
}

/// The PIDs of this user's processes whose arguments contain the full
/// `marker` URL, never this test process. Entries that cannot be read are
/// skipped.
#[cfg(target_os = "linux")]
fn marker_pids(marker: &str) -> Vec<libc::pid_t> {
    let me = std::process::id() as libc::pid_t;
    let Ok(entries) = fs::read_dir("/proc") else {
        eprintln!("issue #1764: cannot read /proc; no process for {marker} can be found");
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<libc::pid_t>().ok())
        .filter(|&pid| pid != me && owned_by_me(pid))
        .filter(|&pid| cmdline_of(pid).is_some_and(|line| line.contains(marker)))
        .collect()
}

/// Kill and reap every process of this user that carries `marker`, and
/// `child`, waiting only on exit events.
///
/// Each pass opens a pidfd for each marker process, checks again that the
/// process behind it still carries the marker, sends `SIGKILL` through the
/// pidfd, kills and waits for the direct child, then blocks until each pidfd
/// reports the exit. `SIGKILL` ends a stopped process without continuing it.
/// The next pass finds any process forked before its parent died; the loop
/// ends when a pass kills nothing. Errors are printed, never raised: this
/// runs from `Drop`, possibly while a failed assertion unwinds.
#[cfg(target_os = "linux")]
fn reap_marked(marker: &str, mut child: Option<Child>) {
    loop {
        let mut killed = Vec::new();
        for pid in marker_pids(marker) {
            let pidfd = match pidfd_open(pid) {
                Ok(pidfd) => pidfd,
                Err(error) if error.raw_os_error() == Some(libc::ESRCH) => continue,
                Err(error) => {
                    eprintln!("issue #1764: pidfd_open({pid}) for {marker} failed: {error}");
                    continue;
                }
            };
            // The PID may have been reused between the scan and the open.
            if !cmdline_of(pid).is_some_and(|line| line.contains(marker)) {
                continue;
            }
            match pidfd_send_signal(&pidfd, libc::SIGKILL) {
                Ok(()) => {}
                Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {}
                Err(error) => {
                    eprintln!("issue #1764: SIGKILL to {pid} for {marker} failed: {error}");
                    continue;
                }
            }
            killed.push((pid, pidfd));
        }
        if let Some(mut child) = child.take() {
            match child.kill() {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => {}
                Err(error) => eprintln!(
                    "issue #1764: killing child {} for {marker} failed: {error}",
                    child.id()
                ),
            }
            if let Err(error) = child.wait() {
                eprintln!(
                    "issue #1764: waiting for child {} for {marker} failed: {error}",
                    child.id()
                );
            }
        }
        if killed.is_empty() {
            return;
        }
        for (pid, pidfd) in &killed {
            if let Err(error) = poll_exit(pidfd) {
                eprintln!("issue #1764: waiting for {pid} for {marker} to exit failed: {error}");
                return;
            }
        }
    }
}

/// Owns the direct child of one case and the marker URL that every process
/// the case starts carries. Dropping it, on success, failure or panic, runs
/// [`reap_marked`]. It never panics.
#[cfg(target_os = "linux")]
struct Owned {
    child: Option<Child>,
    marker: String,
}

#[cfg(target_os = "linux")]
impl Owned {
    fn child(&mut self) -> &mut Child {
        self.child
            .as_mut()
            .expect("the guard holds its child until it is dropped")
    }
}

#[cfg(target_os = "linux")]
impl Drop for Owned {
    fn drop(&mut self) {
        reap_marked(&self.marker, self.child.take());
    }
}

// ── Prompt and stop ──────────────────────────────────────────────────────────

/// The bytes git writes when it asks for a username. The prompt has no
/// newline after it.
#[cfg(target_os = "linux")]
const PROMPT: &[u8] = b"Username for '";

/// What precedes the same bytes in git's error when terminal prompts are off
/// (`fatal: could not read Username for '…': terminal prompts disabled`).
#[cfg(target_os = "linux")]
const NOT_A_PROMPT: &[u8] = b"could not read ";

/// What [`read_until_prompt`] saw: the prompt, or the end of the output
/// without one. Each holds the output read so far.
#[cfg(target_os = "linux")]
#[derive(Debug)]
enum Watched {
    Prompt(String),
    Eof(String),
}

/// Whether `output` holds the username prompt: [`PROMPT`] not preceded by
/// [`NOT_A_PROMPT`].
#[cfg(target_os = "linux")]
fn holds_prompt(output: &[u8]) -> bool {
    output
        .windows(PROMPT.len())
        .enumerate()
        .any(|(at, window)| window == PROMPT && !output[..at].ends_with(NOT_A_PROMPT))
}

/// Read `output` with blocking reads until the username prompt appears or the
/// output ends. It scans bytes, not lines, so a prompt split across two reads
/// is still found. It only reads; nothing is ever written back.
#[cfg(target_os = "linux")]
fn read_until_prompt<R: Read>(mut output: R) -> Watched {
    let mut seen = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match output.read(&mut chunk) {
            Ok(0) => return Watched::Eof(String::from_utf8_lossy(&seen).into_owned()),
            Ok(n) => {
                // The whole output is scanned again after each read: it is
                // short, and a match split across reads needs no tail logic.
                seen.extend_from_slice(&chunk[..n]);
                if holds_prompt(&seen) {
                    return Watched::Prompt(String::from_utf8_lossy(&seen).into_owned());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => panic!(
                "reading the output of `script` failed: {error}; read so far: {}",
                String::from_utf8_lossy(&seen)
            ),
        }
    }
}

/// Field 4 (parent PID) of `/proc/<pid>/stat`.
#[cfg(target_os = "linux")]
fn parent_of(pid: libc::pid_t) -> Option<libc::pid_t> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name may hold spaces; the fields follow its closing `)`.
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// Whether `ancestor` is reached by following parent links up from `pid`.
#[cfg(target_os = "linux")]
fn descends_from(pid: libc::pid_t, ancestor: libc::pid_t) -> bool {
    let mut at = pid;
    while at > 1 {
        match parent_of(at) {
            Some(parent) if parent == ancestor => return true,
            Some(parent) => at = parent,
            None => return false,
        }
    }
    false
}

/// The `git ls-remote <url>` process under `script_pid`, owned by this user,
/// found with one `/proc` scan after the prompt has appeared. Panics if there
/// is none: the tracer may only ever attach to a descendant of the test.
#[cfg(target_os = "linux")]
fn git_ls_remote_under(script_pid: libc::pid_t, url: &str) -> libc::pid_t {
    let candidates: Vec<libc::pid_t> = marker_pids(url)
        .into_iter()
        .filter(|&pid| {
            let Ok(raw) = fs::read(format!("/proc/{pid}/cmdline")) else {
                return false;
            };
            let argv: Vec<&[u8]> = raw.split(|&b| b == 0).collect();
            argv.len() >= 3
                && Path::new(OsStr::new(&*String::from_utf8_lossy(argv[0])))
                    .file_name()
                    .is_some_and(|name| name == "git")
                && argv[1] == b"ls-remote"
                && argv[2..].contains(&url.as_bytes())
        })
        .filter(|&pid| descends_from(pid, script_pid))
        .collect();
    match candidates.as_slice() {
        [pid] => *pid,
        _ => panic!(
            "the control printed the prompt, but there is not exactly one \
             `git ls-remote {url}` process under script {script_pid}: {candidates:?}; \
             processes for the URL: {:#?}",
            processes_mentioning(url)
        ),
    }
}

/// How the control's stop was established.
#[cfg(target_os = "linux")]
#[derive(Debug, PartialEq, Eq)]
enum Armed {
    /// The kernel reported the process stopped on `SIGTTIN` to a tracer.
    Sigttin,
    /// `PTRACE_SEIZE` failed with EPERM (a sandbox), so the stop could not be
    /// observed; the prompt alone counts.
    PtraceDenied,
}

/// Attach to `pid` with `PTRACE_SEIZE` and wait for the kernel to report it
/// stopped on `SIGTTIN`, then detach and leave it stopped.
///
/// Run it on its own thread and join that thread: every ptrace call and the
/// wait happen here, on one thread. A process that was already stopped is
/// reported as a group-stop (`PTRACE_EVENT_STOP`) and is detached with no
/// signal. A process that had not yet dequeued the signal is reported in a
/// signal-delivery stop, which already shows the trap is armed; detaching
/// with `SIGTTIN` delivers the signal, so the process stops as it would have
/// done untraced. Only EPERM from the seize maps to [`Armed::PtraceDenied`];
/// any other error, or any other stop, panics, after detaching if the seize
/// succeeded.
#[cfg(target_os = "linux")]
fn sigttin_seen(pid: libc::pid_t) -> Armed {
    if let Err(error) = ptrace_request(libc::PTRACE_SEIZE as libc::c_uint, pid, 0) {
        if error.raw_os_error() == Some(libc::EPERM) {
            eprintln!(
                "issue #1764: PTRACE_SEIZE on {pid} was refused ({error}); the SIGTTIN \
                 stop is not observed and the prompt alone counts"
            );
            return Armed::PtraceDenied;
        }
        panic!("PTRACE_SEIZE on {pid} failed: {error}");
    }
    let detach = |signal: libc::c_int| {
        if let Err(error) = ptrace_request(libc::PTRACE_DETACH as libc::c_uint, pid, signal) {
            panic!("PTRACE_DETACH from {pid} failed: {error}");
        }
    };
    let status = match waitpid_wall(pid) {
        Ok(status) => status,
        Err(error) => {
            // The process is no longer reported, so there is nothing left to
            // detach from.
            panic!("waiting for the traced process {pid} failed: {error}")
        }
    };
    if !libc::WIFSTOPPED(status) {
        panic!(
            "trap was not armed: the traced process {pid} ended instead of stopping on \
             SIGTTIN: status {status:#x}"
        );
    }
    let signal = libc::WSTOPSIG(status);
    let group_stop = status >> 16 == libc::PTRACE_EVENT_STOP;
    match (signal, group_stop) {
        (libc::SIGTTIN, true) => {
            detach(0);
            Armed::Sigttin
        }
        (libc::SIGTTIN, false) => {
            // Signal-delivery stop on SIGTTIN: the trap is armed. Detaching
            // with SIGTTIN delivers it, so the process stops untraced.
            detach(libc::SIGTTIN);
            Armed::Sigttin
        }
        _ => {
            // Pass on any signal it was about to receive, so the process
            // behaves as if it had never been traced.
            detach(if group_stop { 0 } else { signal });
            panic!(
                "trap was not armed: the traced process {pid} stopped with signal \
                 {signal} (group-stop: {group_stop}, status {status:#x}), not on \
                 SIGTTIN. If git was left in the foreground group it prompts and then \
                 blocks on its terminal read instead of stopping, and the inner \
                 `timeout 8` ends it with SIGTERM (signal 15)"
            );
        }
    }
}

/// How a case's child ended.
#[cfg(target_os = "linux")]
#[derive(Debug)]
enum Outcome {
    /// The child exited by itself; `code` is `None` if a signal ended it.
    Exited {
        code: Option<i32>,
        stdout: String,
        stderr: String,
    },
    /// The output held the username prompt.
    Prompted { output: String },
}

/// Point a git (or a `script` that runs one) at `home` and away from every
/// stored credential: no global or system config, so no credential helper; a
/// `HOME` with no `.netrc`; no repository discovery above `home`. Git's
/// messages are kept in English so the assertions can read them.
fn sealed_from_credentials<'a>(command: &'a mut Command, home: &Path) -> &'a mut Command {
    command
        .current_dir(home)
        .env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CEILING_DIRECTORIES",
            home.parent().expect("the temp HOME has a parent"),
        )
        .env("LC_ALL", "C")
}

/// Fail if a process whose command line contains `marker` is still running.
/// Called once a case's [`Owned`] guard has dropped, which waited for every
/// marker process to exit, so one check suffices.
fn assert_nothing_left_behind(marker: &str) {
    let left = processes_mentioning(marker);
    assert!(
        left.is_empty(),
        "processes for {marker} outlived the case that started them: {left:#?}"
    );
}

/// Every process (by `/proc`) whose command line contains `marker`, as
/// `"<pid> <command line>"`. Panics without `/proc`: its callers run only on
/// Linux, and an empty answer would let `assert_nothing_left_behind` pass
/// without checking anything.
fn processes_mentioning(marker: &str) -> Vec<String> {
    fs::read_dir("/proc")
        .expect("read /proc: part (c) runs only on Linux")
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let pid = entry.file_name().into_string().ok()?;
            if !pid.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let cmdline = fs::read(entry.path().join("cmdline")).ok()?;
            let cmdline = String::from_utf8_lossy(&cmdline).replace('\0', " ");
            cmdline
                .contains(marker)
                .then(|| format!("{pid} {}", cmdline.trim_end()))
        })
        .collect()
}

fn on_path(tool: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(tool).is_file()))
}

/// The value `command` sets for `key`, if it sets one.
fn env_value<'a>(command: &'a Command, key: &str) -> Option<&'a OsStr> {
    command
        .get_envs()
        .find(|(name, _)| *name == OsStr::new(key))
        .and_then(|(_, value)| value)
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join("amplifier-bundle/recipes").is_dir())
        .map(Path::to_path_buf)
        .expect("workspace must contain amplifier-bundle/recipes")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn recipe_text(name: &str) -> String {
    read(&workspace_root().join(format!("amplifier-bundle/recipes/{name}.yaml")))
}

/// The `prompt:` of step `id` in recipe `name`.
fn step_prompt(name: &str, id: &str) -> String {
    let recipe: Value = serde_yaml::from_str(&recipe_text(name))
        .unwrap_or_else(|e| panic!("parse {name}.yaml: {e}"));
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .unwrap_or_else(|| panic!("{name}.yaml declares no steps"))
        .iter()
        .find(|step| step.get("id").and_then(Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("{name}.yaml has no step `{id}`"))
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{name}.yaml step `{id}` has no prompt"))
        .to_string()
}

// ── (a) Environment ──────────────────────────────────────────────────────────

/// A test process gets the three variables from cargo's `[env]` table, and an
/// `isolated(..)` child gets them back after `env_clear()` has dropped them.
#[test]
fn part_a_test_processes_and_isolated_children_get_the_suppressors() {
    for (name, value) in NO_GIT_PROMPT_ENV {
        let got = std::env::var(name).ok();
        assert_eq!(
            got.as_deref(),
            Some(value),
            "{name} is {got:?} in the test process, not {value:?}. The [env] table in \
             .cargo/config.toml sets it with force = true, so find out why it did not \
             reach this test. Do not add it to .github/workflows/ci.yml: this check \
             exists to show that [env] reaches the tests."
        );
    }

    let child = isolated("x");
    for (name, value) in NO_GIT_PROMPT_ENV {
        assert_eq!(
            env_value(&child, name),
            Some(OsStr::new(value)),
            "isolated(..) must set {name}={value} after its env_clear(), which drops \
             the value from [env]"
        );
    }
}

/// A cargo config in the repository applies to everyone who builds it (SR1),
/// so it holds the three `[env]` entries and nothing else.
#[test]
fn part_a_cargo_config_holds_only_the_env_table() {
    let config = read(&workspace_root().join(".cargo/config.toml"));
    let lines: Vec<&str> = config
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    let expected: Vec<String> = std::iter::once("[env]".to_string())
        .chain(
            NO_GIT_PROMPT_ENV
                .iter()
                .map(|(name, value)| format!("{name} = {{ value = \"{value}\", force = true }}")),
        )
        .collect();
    assert_eq!(
        lines, expected,
        ".cargo/config.toml must hold exactly the [env] table for the three git prompt \
         variables; review any other setting line by line before changing this test"
    );
}

// ── (b) Git builder ──────────────────────────────────────────────────────────

/// A git started through the builder, with no askpass program and a null
/// stdin, exits by itself with `terminal prompts disabled` instead of opening
/// the terminal. On CI, where there is no terminal, the message is what tells
/// this apart from the `No such device or address` failure that hid the trap.
/// The test waits for git's own exit; nothing is timed.
#[cfg(target_os = "linux")]
#[test]
fn part_b_builder_git_fails_at_once_without_askpass() {
    let server = AlwaysUnauthorised::start();
    let url = server.url();
    let home = tempfile::tempdir().expect("temp HOME");
    let out = tempfile::tempdir().expect("tempdir for git's stdout");
    let stdout_path = out.path().join("stdout");

    let mut git = amplihack_git::command();
    // This process already has the variable from `[env]`, so git's behaviour
    // below cannot show whether the builder set it.
    assert_eq!(
        env_value(&git, "GIT_TERMINAL_PROMPT"),
        Some(OsStr::new("0")),
        "amplihack_git::command() must set GIT_TERMINAL_PROMPT=0 itself"
    );
    git.args(["ls-remote", &url]);
    // After the builder: it scrubs GIT_CEILING_DIRECTORIES.
    sealed_from_credentials(&mut git, home.path());
    for var in [
        "GIT_ASKPASS",
        "SSH_ASKPASS",
        "GIT_CONFIG_PARAMETERS",
        "GIT_CONFIG_COUNT",
    ]
    .into_iter()
    .chain(PROXY_VARS)
    {
        git.env_remove(var);
    }

    let mut child = git
        .stdin(Stdio::null())
        .stdout(File::create(&stdout_path).expect("stdout file"))
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn git");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let mut guard = Owned {
        child: Some(child),
        marker: url.clone(),
    };
    let drain = std::thread::spawn(move || {
        let mut text = Vec::new();
        stderr
            .read_to_end(&mut text)
            .expect("read git's stderr to its end");
        String::from_utf8_lossy(&text).into_owned()
    });
    let stderr = drain
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
    let status = guard.child().wait().expect("wait for git");
    let outcome = Outcome::Exited {
        code: status.code(),
        stdout: String::from_utf8_lossy(&fs::read(&stdout_path).expect("read git's stdout"))
            .into_owned(),
        stderr,
    };
    drop(guard);
    assert_nothing_left_behind(&url);

    match &outcome {
        Outcome::Exited {
            code: Some(code),
            stderr,
            ..
        } if *code != 0 => assert!(
            stderr.contains("terminal prompts disabled"),
            "git failed, but not because terminal prompts are off: {outcome:#?}"
        ),
        _ => panic!(
            "git should have exited non-zero by itself, not by a signal, with \
             `terminal prompts disabled`: {outcome:#?}"
        ),
    }
}

// ── (c) Pseudo-terminal ──────────────────────────────────────────────────────

/// One case of part (c): its guard, what its output showed, and the
/// directory that holds its `HOME` and `script`'s stderr. Drop the guard
/// before the directory.
#[cfg(target_os = "linux")]
struct Scripted {
    guard: Owned,
    watched: Watched,
    dir: tempfile::TempDir,
}

/// Run `timeout 8 git ls-remote <url>` under `script`, in a pseudo-terminal,
/// with `remove` taken out of the environment that `isolated(..)` builds, and
/// read `script`'s output until git's username prompt appears or the output
/// ends.
///
/// The shell command must be compound. Given one simple command, the shell
/// execs it, `timeout` becomes the session leader and stays in the foreground
/// group, so git can read the terminal and the trap is never armed. With a
/// compound command the shell stays in the foreground, `timeout` puts git in
/// a background group, and a read of the terminal stops it on `SIGTTIN`.
/// Only the port and the process id are put into the shell string.
///
/// The `8` affects only GNU `timeout`, which ignores `SIGTTIN`, keeps
/// running and reports 124; uutils `timeout` is stopped with git. No check
/// reads the value.
#[cfg(target_os = "linux")]
fn git_under_script(server: &AlwaysUnauthorised, remove: &[&str]) -> Scripted {
    let dir = tempfile::tempdir().expect("tempdir for the case");
    let home = dir.path().join("home");
    fs::create_dir(&home).expect("create the temp HOME");
    let inner = format!(
        "timeout 8 git ls-remote http://127.0.0.1:{port}/issue-1764-{pid}.git \
         </dev/null; rc=$?; echo \"rc=$rc\"; exit $rc",
        port = server.port,
        pid = std::process::id(),
    );
    let mut script = isolated("script");
    script
        .args(["-qec", &inner, "/dev/null"])
        .env("SHELL", "/bin/sh");
    sealed_from_credentials(&mut script, &home);
    for var in remove {
        script.env_remove(var);
    }
    // Never the test's own stdin: `script` would put a developer's terminal
    // into raw mode.
    let child = script
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(File::create(dir.path().join("stderr")).expect("stderr file"))
        .spawn()
        .expect("spawn script");
    let mut guard = Owned {
        child: Some(child),
        marker: server.url(),
    };
    // The pipe stays open in the child until the guard drops, so `script`
    // never writes into a closed pipe while git is stopped.
    let stdout = guard
        .child()
        .stdout
        .as_mut()
        .expect("script's stdout is piped");
    let watched = read_until_prompt(stdout);
    Scripted {
        guard,
        watched,
        dir,
    }
}

/// How `case` ended: the prompt, or the exit of `script` once its output has
/// ended.
#[cfg(target_os = "linux")]
fn outcome(case: &mut Scripted) -> Outcome {
    match &case.watched {
        Watched::Prompt(output) => Outcome::Prompted {
            output: output.clone(),
        },
        Watched::Eof(output) => {
            let status = case.guard.child().wait().expect("wait for script");
            Outcome::Exited {
                code: status.code(),
                stdout: output.clone(),
                stderr: String::from_utf8_lossy(
                    &fs::read(case.dir.path().join("stderr")).expect("read script's stderr"),
                )
                .into_owned(),
            }
        }
    }
}

/// End `case`: reap every process it started, then remove its directory, then
/// check that nothing carrying `url` is left.
#[cfg(target_os = "linux")]
fn finish(case: Scripted, url: &str) {
    let Scripted { guard, dir, .. } = case;
    drop(guard);
    drop(dir);
    assert_nothing_left_behind(url);
}

/// The status in the `rc=<n>` line the inner shell prints. Git's prompt ends
/// without a newline, so in the control the status follows it on the same
/// line: `Username for '…': rc=124`.
fn rc_line(stdout: &str) -> Option<i32> {
    stdout
        .lines()
        .find_map(|line| line.trim().rsplit_once("rc=")?.1.parse().ok())
}

/// `outcome` must be an exit by itself, without the prompt, with an `rc=`
/// line other than 124. Returns its output.
#[cfg(target_os = "linux")]
fn assert_exits_by_itself<'a>(label: &str, outcome: &'a Outcome) -> &'a str {
    match outcome {
        Outcome::Exited { stdout, .. } if rc_line(stdout).is_some_and(|rc| rc != 124) => {
            stdout.as_str()
        }
        Outcome::Prompted { output } => panic!(
            "{label}: git printed the username prompt, so it would have waited on the \
             terminal: {output}"
        ),
        Outcome::Exited { .. } => panic!(
            "{label}: git should have exited by itself without prompting, printing an \
             rc= line other than 124: {outcome:#?}"
        ),
    }
}

/// The control: with none of the three variables, git must print the
/// username prompt, and `git ls-remote` must be seen stopped on `SIGTTIN`.
///
/// The prompt on the pipe is the event that starts the check. The test then
/// finds `git ls-remote` under `script` and, on a thread it joins, attaches
/// with `ptrace` and waits until the kernel reports the stop. If `ptrace` is
/// refused with EPERM, the prompt alone counts, and a tracer failure panics.
/// Without a prompt the control fails with `trap was not armed`, whatever the
/// status: an `rc=124` from the inner `timeout` is printed in the message but
/// is not accepted, since a git stalled on the server also produces it.
#[cfg(target_os = "linux")]
fn assert_control_stops_on_sigttin(case: &mut Scripted, url: &str) {
    if matches!(case.watched, Watched::Prompt(_)) {
        let script_pid = case.guard.child().id() as libc::pid_t;
        let git = git_ls_remote_under(script_pid, url);
        let traced = std::thread::spawn(move || sigttin_seen(git)).join();
        match traced {
            Ok(Armed::Sigttin) => return,
            Ok(Armed::PtraceDenied) => {
                eprintln!(
                    "issue #1764 part (c): the control prompted; ptrace was refused, so \
                     its SIGTTIN stop was not observed"
                );
                return;
            }
            // Re-raise at once: reading on would block for ever if git is
            // still stopped, turning a tracer failure into a hang.
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
    // No prompt: the trap was not shown. An `rc=124` here only means the
    // inner `timeout` fired, which a git stalled on the server also causes,
    // so it is reported but never accepted.
    let ended = outcome(case);
    let rc = match &ended {
        Outcome::Exited { stdout, .. } => rc_line(stdout),
        Outcome::Prompted { .. } => None,
    };
    panic!(
        "trap was not armed: with none of the three variables, git under plain \
         `timeout` in a pseudo-terminal should have printed the username prompt and \
         stopped on SIGTTIN, but no prompt appeared (rc line: {rc:?}). Without the \
         trap, (c1) and (c2) prove nothing: {ended:#?}"
    );
}

/// The trap, and the variables that keep git out of it, in a real
/// pseudo-terminal. The control has none of the three variables: it must
/// print the username prompt and `git ls-remote` must be seen stopped on
/// `SIGTTIN` (through `ptrace`). (c1) and
/// (c2) must exit by themselves without prompting. Each case is reaped
/// through pidfds, by its marker URL, before the next one starts, and the
/// test then checks that no process using the URL is left.
#[cfg(target_os = "linux")]
#[test]
fn part_c_plain_timeout_trap_in_a_pseudo_terminal() {
    if let Some(missing) = ["script", "timeout"]
        .into_iter()
        .find(|tool| !on_path(tool))
    {
        eprintln!("issue #1764 part (c) skipped: `{missing}` is not on PATH");
        return;
    }
    let server = AlwaysUnauthorised::start();
    let url = server.url();
    let all: Vec<&str> = NO_GIT_PROMPT_ENV.iter().map(|(name, _)| *name).collect();

    let mut control = git_under_script(&server, &all);
    assert_control_stops_on_sigttin(&mut control, &url);
    finish(control, &url);

    let mut c1 = git_under_script(&server, &["GIT_ASKPASS", "SSH_ASKPASS"]);
    let c1_outcome = outcome(&mut c1);
    finish(c1, &url);
    let c1_out = assert_exits_by_itself("(c1) GIT_TERMINAL_PROMPT=0 alone", &c1_outcome);
    assert!(
        c1_out.contains("terminal prompts disabled"),
        "(c1) must fail because terminal prompts are off: {c1_outcome:#?}"
    );

    let mut c2 = git_under_script(&server, &[]);
    let c2_outcome = outcome(&mut c2);
    finish(c2, &url);
    assert_exits_by_itself("(c2) all three variables", &c2_outcome);
}

// ── (d) Recipe text ──────────────────────────────────────────────────────────

/// The agent steps that run long commands.
const LONG_COMMAND_STEPS: [(&str, &str); 3] = [
    ("workflow-precommit-test", "step-12-run-precommit"),
    ("workflow-precommit-test", "step-13-local-testing"),
    ("autodrive-merge-round", "step-04-address-blockers"),
];

/// Step 12's evidence schema, which the verification step's `jq` gate reads.
/// The guidance for long commands must not change it.
const STEP_12_SCHEMA: [&str; 2] = [
    "workspace_command (\"cargo test --workspace --locked\"),",
    "{\"status\":\"PASS\",\"exit_code\":0,\"precommit_exit_code\":0,\"workspace_exit_code\":0,\
     \"workspace_command\":\"cargo test --workspace --locked\",\"validation_scope\":\"FULL_WORKSPACE\",\
     \"test_threads\":\"DEFAULT\",\"evidence\":[{\"log\":\"<actual retained raw log>\",\
     \"exit_receipt\":\"<actual retained numeric exit receipt>\"}]}",
];

/// Wall-clock wrappers a long-command step must not offer (issue #2022): a
/// build or a test has no time limit, and `timeout` counts stopped time.
const WALL_CLOCK_WRAPPERS: [&str; 4] = [
    "setsid -w timeout",
    "timeout --kill-after",
    "timeout --foreground",
    "<secs>",
];

/// Each step tells the agent to export `GIT_TERMINAL_PROMPT=0` and to run a
/// long command as `setsid -w <cmd> </dev/null`, and why not under `timeout`:
/// the work has no time limit (issue #2022), and plain `timeout` leaves a
/// prompting git stopped on `SIGTTIN`. No step offers a wall-clock wrapper.
#[test]
fn part_d_recipe_prompts_say_how_to_run_long_commands() {
    for (recipe, id) in LONG_COMMAND_STEPS {
        let prompt = step_prompt(recipe, id);
        let required = if recipe == "autodrive-merge-round" {
            vec![
                "GIT_TERMINAL_PROMPT=0",
                "noninteractive execution",
                "natural completion",
            ]
        } else {
            vec![
                "GIT_TERMINAL_PROMPT=0",
                "`setsid -w <cmd> </dev/null`",
                "`<cmd> </dev/null`",
                "Do not wrap",
                "issue #2022",
                "SIGSTOP",
                "SIGTTIN",
            ]
        };
        for needle in required {
            assert!(
                prompt.contains(needle),
                "{recipe}.yaml step `{id}` must tell the agent about `{needle}`"
            );
        }
        for wrapper in WALL_CLOCK_WRAPPERS {
            assert!(
                !prompt.contains(wrapper),
                "{recipe}.yaml step `{id}` must not tell the agent to run work under `{wrapper}` (issue #2022)"
            );
        }
        for weakening in WEAKENING {
            assert!(
                !prompt.contains(weakening),
                "{recipe}.yaml step `{id}` must not offer `{weakening}`"
            );
        }
    }

    let step_12 = step_prompt("workflow-precommit-test", "step-12-run-precommit");
    for pinned in STEP_12_SCHEMA {
        assert!(
            step_12.contains(pinned),
            "step 12's evidence schema must stay byte-for-byte: missing `{pinned}`"
        );
    }
    assert!(
        recipe_text("workflow-precommit-test")
            .contains(".workspace_command == \"cargo test --workspace --locked\""),
        "the verification step's jq gate on workspace_command must stay"
    );

    let lines = recipe_text("autodrive-merge-round").lines().count();
    assert!(
        lines <= BRICK_LINE_BUDGET,
        "autodrive-merge-round.yaml is {lines} lines; the brick budget is \
         {BRICK_LINE_BUDGET}, and the guidance must fit inside it"
    );
}

// ── Shell tests ──────────────────────────────────────────────────────────────

/// The shell tests that run the system git against a remote that is not
/// local. Run by hand with `bash`, outside cargo, they get no `[env]` table.
const SHELL_TESTS: [&str; 12] = [
    "tests/git_identity_test.sh",
    "tests/issue_1268_terminal_state_adjudication.sh",
    "tests/issue_1290_identity_preflight.sh",
    "tests/issue_1361_duplicate_pr_claim_check.sh",
    "tests/issue_1448_review_is_not_a_duplicate.sh",
    "tests/issue_1596_final_status_branch_pr_lookup.sh",
    "tests/issue_1676_build_resolves_agent_pr.sh",
    "tests/issue_1680_fresh_run_state_key.sh",
    "tests/outside-in/scripts/scenario8-checks.sh",
    "amplifier-bundle/recipes/tests/test-issue-983-azdo-work-item-assignment.sh",
    "amplifier-bundle/recipes/tests/test-default-workflow-reliability.sh",
    "amplifier-bundle/recipes/tests/test-issue-1060-existing-pr-is-idempotent.sh",
];

/// Straight after its `set` line, each script exports the three variables and
/// then takes its stdin from `/dev/null`, before any git can run.
#[test]
fn shell_tests_turn_git_prompts_off_at_the_top() {
    let root = workspace_root();
    let export = format!(
        "export {}",
        NO_GIT_PROMPT_ENV
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    for script in SHELL_TESTS {
        let text = read(&root.join(script));
        let code: Vec<&str> = text
            .lines()
            .map(str::trim)
            .skip_while(|line| !line.starts_with("set "))
            .skip(1)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .take(2)
            .collect();
        assert_eq!(
            code,
            [export.as_str(), "exec </dev/null"],
            "{script}: the first commands after its `set` line must be `{export}` and \
             `exec </dev/null`"
        );
    }
}

// ── How-to ───────────────────────────────────────────────────────────────────

#[test]
fn howto_is_linked_and_offers_no_weakening() {
    let root = workspace_root();
    let howto = read(&root.join("docs/howto/keep-tests-from-prompting-for-git.md"));
    for (file, link) in [
        ("mkdocs.yml", "howto/keep-tests-from-prompting-for-git.md"),
        (
            "docs/index.md",
            "howto/keep-tests-from-prompting-for-git.md",
        ),
        (
            "docs/howto/develop-amplihack.md",
            "(./keep-tests-from-prompting-for-git.md)",
        ),
        (
            "docs/reference/default-workflow-agentic-finalization.md",
            "(../howto/keep-tests-from-prompting-for-git.md)",
        ),
    ] {
        assert!(
            read(&root.join(file)).contains(link),
            "{file} must link the #1764 how-to"
        );
    }
    for weakening in WEAKENING {
        assert!(
            !howto.contains(weakening),
            "the #1764 how-to must not offer `{weakening}`"
        );
    }
}

/// Reads a fixed list of chunks, so a test can split the prompt across reads.
#[cfg(target_os = "linux")]
struct Chunks(std::collections::VecDeque<Vec<u8>>);

#[cfg(target_os = "linux")]
impl Read for Chunks {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let Some(mut chunk) = self.0.pop_front() else {
            return Ok(0);
        };
        let n = chunk.len().min(buf.len());
        buf[..n].copy_from_slice(&chunk[..n]);
        if n < chunk.len() {
            self.0.push_front(chunk.split_off(n));
        }
        Ok(n)
    }
}

/// D1: the reader scans bytes, so it finds a prompt that has no newline and
/// is split across two reads at any point.
#[cfg(target_os = "linux")]
#[test]
fn issue_2099_reader_finds_a_prompt_split_across_reads() {
    let output = b"Cloning\r\nUsername for 'http://127.0.0.1:1/x.git': ";
    for split in 1..output.len() {
        let chunks = Chunks([output[..split].to_vec(), output[split..].to_vec()].into());
        match read_until_prompt(chunks) {
            Watched::Prompt(seen) => assert!(
                seen.contains("Username for '"),
                "split at {split}: the prompt text is kept: {seen:?}"
            ),
            Watched::Eof(seen) => panic!("split at {split}: prompt missed: {seen:?}"),
        }
    }
}

/// D2: output with no prompt reaches EOF and keeps every byte, including the
/// `rc=` line that (c1) and (c2) are judged by.
#[cfg(target_os = "linux")]
#[test]
fn issue_2099_reader_reports_eof_without_a_prompt() {
    let output =
        b"fatal: could not read Username for 'http://x': terminal prompts disabled\r\nrc=128\r\n";
    // (c1) prints git's error `could not read Username for '…'`, which holds
    // the prompt's bytes. The reader must not take it for the prompt, or (c1)
    // would fail as prompted: the prompt is `Username for '` not preceded by
    // `could not read `.
    match read_until_prompt(Chunks([b"rc=128\r\n".to_vec()].into())) {
        Watched::Eof(seen) => assert_eq!(rc_line(&seen), Some(128)),
        Watched::Prompt(seen) => panic!("no prompt was written: {seen:?}"),
    }
    match read_until_prompt(Chunks([output.to_vec()].into())) {
        Watched::Eof(seen) => {
            assert!(seen.contains("terminal prompts disabled"));
            assert_eq!(rc_line(&seen), Some(128));
        }
        Watched::Prompt(seen) => panic!(
            "git's `could not read Username for '…'` error is not a prompt; (c1) \
             prints it and must not fail as prompted: {seen:?}"
        ),
    }
    match read_until_prompt(Chunks(std::collections::VecDeque::new())) {
        Watched::Eof(seen) => assert!(seen.is_empty()),
        Watched::Prompt(seen) => panic!("empty output has no prompt: {seen:?}"),
    }
}

/// The status line follows the prompt on the same line in the control.
#[test]
fn issue_2099_rc_line_reads_the_status_after_a_prompt() {
    assert_eq!(rc_line("Username for 'http://x': rc=124\r\n"), Some(124));
    assert_eq!(rc_line("noise\nrc=128\n"), Some(128));
    assert_eq!(rc_line("no status here\n"), None);
}

/// A marker unique to one unit test, in the same shape as the server URL.
#[cfg(target_os = "linux")]
fn unit_marker(name: &str) -> String {
    format!(
        "http://127.0.0.1:0/issue-1764-{}-{name}.git",
        std::process::id()
    )
}

/// D4, D5, AC8: dropping `Owned` kills and reaps the direct child and a
/// stopped grandchild found by the marker, and leaves nothing behind. The
/// readiness event is the `ready` line; no wait is timed.
#[cfg(target_os = "linux")]
#[test]
fn issue_2099_owned_reaps_child_and_stopped_grandchild_by_marker() {
    use std::io::{BufRead, BufReader};
    let marker = unit_marker("reap");
    // The grandchild stops itself with SIGSTOP; the child waits on it, which
    // returns only when it ends.
    let mut child = Command::new("sh")
        .args([
            "-c",
            "sh -c 'kill -STOP $$' \"$0\" & echo ready; wait",
            &marker,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn sh");
    let mut ready = String::new();
    BufReader::new(child.stdout.take().expect("piped stdout"))
        .read_line(&mut ready)
        .expect("read the ready line");
    assert_eq!(ready.trim(), "ready");

    let found = marker_pids(&marker);
    assert!(
        found.contains(&(child.id() as libc::pid_t)),
        "marker_pids finds the direct child: {found:?}"
    );
    assert!(
        found.len() >= 2,
        "marker_pids finds the grandchild too: {found:?}"
    );
    assert!(
        !found.contains(&(std::process::id() as libc::pid_t)),
        "marker_pids never returns the test process"
    );

    drop(Owned {
        child: Some(child),
        marker: marker.clone(),
    });
    assert!(
        processes_mentioning(&marker).is_empty(),
        "Owned::drop left processes behind: {:#?}",
        processes_mentioning(&marker)
    );
    assert!(marker_pids(&marker).is_empty());
}

/// S12: dropping `Owned` after its child has already exited and been reaped
/// by nobody, or with no child at all, does not panic.
#[cfg(target_os = "linux")]
#[test]
fn issue_2099_owned_drop_tolerates_exited_and_missing_children() {
    let marker = unit_marker("exited");
    let child = Command::new("sh")
        .args(["-c", "exit 0", &marker])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn sh");
    drop(Owned {
        child: Some(child),
        marker: marker.clone(),
    });
    drop(Owned {
        child: None,
        marker: marker.clone(),
    });
    assert!(processes_mentioning(&marker).is_empty());
}

/// D1: ptrace reports the SIGTTIN stop of a process the test started, whether
/// the stop comes before or after the seize, and leaves it stopped, never
/// continued, for `Owned` to reap. The child has its own process group, so
/// the group is not orphaned and the kernel does not discard SIGTTIN. The
/// child also gets the default action for SIGTTIN back before `exec`: an
/// ignored signal stays ignored across `exec`, and a launcher that ignores
/// SIGTTIN (the recipe runner does) would otherwise let the shell run on.
#[cfg(target_os = "linux")]
#[test]
fn issue_2099_sigttin_seen_reports_the_stop() {
    use std::io::{BufRead, BufReader};
    use std::os::unix::process::CommandExt;
    let marker = unit_marker("sigttin");
    let mut command = Command::new("sh");
    command
        .args(["-c", "echo ready; kill -TTIN $$; echo resumed", &marker])
        .process_group(0);
    // SAFETY: the closure runs in the forked child before `exec` and calls
    // only `signal`, which is async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            if libc::signal(libc::SIGTTIN, libc::SIG_DFL) == libc::SIG_ERR {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn sh");
    let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
    let pid = child.id() as libc::pid_t;
    let guard = Owned {
        child: Some(child),
        marker: marker.clone(),
    };
    let mut ready = String::new();
    stdout.read_line(&mut ready).expect("read the ready line");
    assert_eq!(ready.trim(), "ready");

    let armed = std::thread::spawn(move || sigttin_seen(pid))
        .join()
        .expect("the tracer thread does not panic");
    if armed == Armed::PtraceDenied {
        eprintln!("issue #2099: ptrace denied; stop not observed");
    }
    drop(guard);
    assert!(processes_mentioning(&marker).is_empty());
    // A process detached from a group-stop is woken for a moment before it
    // stops again, so `/proc/<pid>/stat` may read `R` straight after the
    // join. The event that shows it stayed stopped until `Owned` killed it is
    // the end of its output without the line it prints when continued.
    let mut rest = String::new();
    stdout
        .read_to_string(&mut rest)
        .expect("read the rest of the output");
    assert!(
        !rest.contains("resumed"),
        "detach must leave the process stopped, not continue it: {rest:?}"
    );
}

/// S5: any ptrace error other than EPERM panics. A PID at `pid_max`, which no
/// process can hold, gives ESRCH, so the tracer must not report it as armed
/// or as denied.
#[cfg(target_os = "linux")]
#[test]
fn issue_2099_sigttin_seen_panics_on_a_process_that_has_gone() {
    let pid: libc::pid_t = fs::read_to_string("/proc/sys/kernel/pid_max")
        .expect("read /proc/sys/kernel/pid_max")
        .trim()
        .parse()
        .expect("pid_max is a number");
    let traced = std::thread::spawn(move || sigttin_seen(pid)).join();
    assert!(
        traced.is_err(),
        "sigttin_seen on a PID no process can hold must panic, not report a result"
    );
}
