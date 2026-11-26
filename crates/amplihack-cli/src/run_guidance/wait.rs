//! Waiting for guidance, or for a command, whichever comes first
//! (issue #3129, D4): the mechanism behind `recipe guidance wait`.
//!
//! [`arm`] puts one [`Waiter`] beside every folder whose parent exists and
//! passes the trust rule. [`wait_armed`] then reads the entries again (so an
//! entry published before a FIFO existed is not missed), starts the command
//! if there is one, and blocks in `poll` on the waiters and a [`ChildWatch`]
//! until a wake byte leads to a new entry or the command exits. A byte that
//! leads to no new entry is spurious and the wait goes on.
//!
//! The main thread is the command's only reaper, so a `SIGTERM` sent when
//! guidance arrives can only reach the command (or its zombie), never a
//! process that reused its PID. The signal goes to the command alone, never
//! to a process group.
//!
//! Nothing here sleeps, sets a timer or retries; the only `unsafe` code is
//! in [`super::watch`] and [`super::wake`].

use super::folders::{Folders, PrFolder};
use super::fs_safety::{check_dir, ensure_private_dir};
use super::wake::Waiter;
use super::watch::{ChildWatch, terminate, wait_readable};
use super::{AUTO_DRIVE_DIR, GUIDANCE_DIR, GuideError};
use std::ffi::{OsStr, OsString};
use std::io::ErrorKind;
use std::os::fd::RawFd;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};

/// How a wait ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WaitOutcome {
    /// An entry arrived after the mark: the newest entry's name. A command
    /// still running was stopped and reaped first.
    Guidance(String),
    /// The command finished first, with this status.
    Command(ExitStatus),
}

/// Makes `<state root>/auto-drive/<key>/` when it is missing, under a state
/// root that already exists and passes the trust rule. The state root itself
/// is never created, which is why this is not `ensure_private_chain` (that
/// creates its base when it is missing).
fn make_pr_parent(pr: &PrFolder) -> Result<(), String> {
    match check_dir(&pr.state_root) {
        Ok(true) => {}
        Ok(false) => {
            return Err(format!(
                "the state folder {} does not exist",
                pr.state_root.display()
            ));
        }
        Err(e) => return Err(e.to_string()),
    }
    let auto_drive = pr.state_root.join(AUTO_DRIVE_DIR);
    ensure_private_dir(&auto_drive)
        .and_then(|()| ensure_private_dir(&auto_drive.join(&pr.key)))
        .map_err(|e| e.to_string())
}

/// Arms one waiter beside each listed guidance folder, once per parent, and
/// says why for each one it could not arm. For the pull request's folder, a
/// missing parent is created first ([`make_pr_parent`]).
pub(crate) fn arm(folders: &Folders) -> (Vec<Waiter>, Vec<String>) {
    let mut parents: Vec<&Path> = Vec::new();
    let mut waiters: Vec<Waiter> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    for folder in &folders.list {
        // A listed path that is not a guidance folder has nothing beside it.
        if !folder.path.is_absolute() || folder.path.file_name() != Some(OsStr::new(GUIDANCE_DIR)) {
            continue;
        }
        let Some(parent) = folder.path.parent() else {
            continue;
        };
        if parents.contains(&parent) {
            continue;
        }
        parents.push(parent);
        if let Some(pr) = folders.pr.as_ref().filter(|pr| pr.path == folder.path)
            && std::fs::symlink_metadata(parent).is_err_and(|e| e.kind() == ErrorKind::NotFound)
            && let Err(problem) = make_pr_parent(pr)
        {
            problems.push(problem);
            continue;
        }
        match Waiter::arm(parent) {
            Ok(waiter) => waiters.push(waiter),
            Err(e) => problems.push(e.to_string()),
        }
    }
    (waiters, problems)
}

/// Starts `program args` with stdin from `/dev/null`, its stdout on this
/// process's stderr (so stdout holds only the result line) and stderr
/// inherited, in this process group, so `recipe stop` reaches it. It holds
/// no wake descriptor: every one is opened with `O_CLOEXEC`.
fn spawn(program: &OsStr, args: &[OsString]) -> Result<Child, GuideError> {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(std::io::stderr()))
        .spawn()
        .map_err(|e| {
            GuideError::Io(format!(
                "cannot start {}: {e}",
                Path::new(program).display()
            ))
        })
}

/// Sends the command `SIGTERM`, never to its group, and reaps it. Blocks
/// until it has exited: there is no timeout.
///
/// A command that has already exited is only reaped, never signalled: once
/// reaped, its PID is no longer its own. Until then only this thread reaps
/// it, so the PID cannot be reused between the check and the signal.
pub(crate) fn stop(child: &mut Child) -> Result<(), GuideError> {
    let failed = |e: std::io::Error| GuideError::Io(format!("cannot stop the command: {e}"));
    if child.try_wait().map_err(failed)?.is_some() {
        return Ok(());
    }
    terminate(child.id()).map_err(failed)?;
    child.wait().map(drop).map_err(failed)
}

/// Waits with `waiters` armed (at least one) for an entry after `since`, or
/// for `command` (when not empty) to finish. An entry already there ends the
/// wait before the command starts.
///
/// # Errors
///
/// [`GuideError::Io`] when the command cannot be started or watched, or the
/// wait fails. A command still running is stopped and reaped first.
pub(crate) fn wait_armed(
    folders: &mut Folders,
    waiters: &mut [Waiter],
    since: &str,
    command: &[OsString],
) -> Result<WaitOutcome, GuideError> {
    // Read after arming: an entry published before the FIFO existed is found
    // here, and one published after it writes a wake byte.
    if let Some(name) = folders.newest_new(since) {
        return Ok(WaitOutcome::Guidance(name));
    }
    let mut child = match command.split_first() {
        Some((program, args)) => Some(spawn(program, args)?),
        None => None,
    };
    let result = supervise(folders, waiters, child.as_mut(), since);
    if result.is_err()
        && let Some(child) = child.as_mut()
    {
        // Never leave the command running behind an error.
        let _ = stop(child);
    }
    result
}

/// Blocks until a wake byte leads to a new entry, or the command exits.
fn supervise(
    folders: &mut Folders,
    waiters: &mut [Waiter],
    mut child: Option<&mut Child>,
    since: &str,
) -> Result<WaitOutcome, GuideError> {
    let watch = match &child {
        Some(child) => Some(
            ChildWatch::start(child.id())
                .map_err(|e| GuideError::Io(format!("cannot watch the command: {e}")))?,
        ),
        None => None,
    };
    let mut fds: Vec<RawFd> = waiters.iter().map(Waiter::as_raw_fd).collect();
    fds.extend(watch.as_ref().map(ChildWatch::as_raw_fd));
    loop {
        let ready = wait_readable(&fds)
            .map_err(|e| GuideError::Io(format!("cannot wait for guidance: {e}")))?;
        let mut woken = false;
        for (waiter, _) in waiters.iter_mut().zip(&ready).filter(|(_, r)| **r) {
            waiter.take();
            woken = true;
        }
        // Guidance first: a byte that leads to no new entry is spurious.
        if woken && let Some(name) = folders.newest_new(since) {
            if let Some(child) = child.as_deref_mut() {
                stop(child)?;
            }
            return Ok(WaitOutcome::Guidance(name));
        }
        if watch.is_some()
            && ready.last() == Some(&true)
            && let Some(child) = child.as_deref_mut()
        {
            // The watch saw the exit without reaping; this reaps it.
            let status = child
                .try_wait()
                .map_err(|e| GuideError::Io(format!("cannot collect the command's status: {e}")))?;
            return status.map(WaitOutcome::Command).ok_or_else(|| {
                GuideError::Io(
                    "lost track of the command: its watch ended before it exited".to_string(),
                )
            });
        }
    }
}
