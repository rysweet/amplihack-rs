//! The whole-run rotate lock (issue #2040, D10):
//! `<root>/.auto-switch.d/rotate/rotate-run.lock`. One rotate run, real or
//! dry, at a time, so two runs never scan `/proc` and probe logins side by
//! side. One non-blocking attempt; nothing waits.
//!
//! The holder writes its pid into the file. That pid is for display only:
//! it is never signalled, and its raw bytes never reach a message.

use super::cycle::{Ctx, Mode};
use super::ledger::RotateState;
use super::log::msg;
use super::prelude::log_sink;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::now;
use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

/// The most of the file read for the holder's pid.
const MAX_PID_BYTES: u64 = 32;

/// The run lock, held until dropped. Dropping it unlocks the file before
/// closing it, so a child forked by another thread that has not yet run
/// `exec` does not keep the lock held through its copy of the descriptor.
pub struct RunLock {
    file: File,
}

impl Drop for RunLock {
    fn drop(&mut self) {
        // Closing the file releases the lock anyway once every copy is closed.
        let _ = FileExt::unlock(&self.file);
    }
}

pub enum Taken {
    Held(RunLock),
    /// Another run holds it; its pid when the file held a usable one.
    Busy(Option<u32>),
    /// The lock file failed a check and was left as it is; the bare
    /// reason, such as `"is not owned by you"`. See [`refusal`].
    Refused(&'static str),
}

/// One attempt at the run lock. A lock file that fails a check (a symlink,
/// not a regular file, not yours, open to group or other users, more than
/// one link) is `Taken::Refused` and is left as it is.
pub fn take(rs: &RotateState) -> Result<Taken, ProfileError> {
    rs.ensure()?;
    let opened = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(&rs.run_lock);
    let mut file = match opened {
        Ok(file) => file,
        Err(e) => match e.raw_os_error() {
            Some(libc::ELOOP) => return Ok(Taken::Refused("is a symlink")),
            // O_RDWR on a directory.
            Some(libc::EISDIR) => return Ok(Taken::Refused("is not a regular file")),
            _ => return Err(ProfileError::Io(e)),
        },
    };
    if let Err(reason) = check(&file.metadata().map_err(ProfileError::Io)?) {
        return Ok(Taken::Refused(reason));
    }
    match FileExt::try_lock_exclusive(&file) {
        Ok(()) => {
            write_pid(&mut file).map_err(ProfileError::Io)?;
            Ok(Taken::Held(RunLock { file }))
        }
        Err(e) if e.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {
            Ok(Taken::Busy(read_pid(&mut file)))
        }
        Err(e) => Err(ProfileError::Io(e)),
    }
}

/// The run lock for one cycle in `mode`, taken before anything is read.
/// `None` when a `watch` cycle must skip: another run holds the lock, or
/// the lock file fails a check. The reason is logged and the daemon carries
/// on. A one-shot or dry run fails instead.
pub fn for_cycle(ctx: &Ctx, mode: Mode) -> Result<Option<RunLock>, ProfileError> {
    let skip = |line: &str| {
        log_sink(ctx, false).emit(now(), line);
        Ok(None)
    };
    match take(&ctx.rs)? {
        Taken::Held(lock) => Ok(Some(lock)),
        Taken::Busy(_) if mode == Mode::Watch => skip(msg::RUN_BUSY),
        Taken::Busy(pid) => Err(ProfileError::RotateRunning(pid)),
        Taken::Refused(reason) if mode == Mode::Watch => {
            skip(&format!("skip: rotate run lock {reason}"))
        }
        Taken::Refused(reason) => Err(refusal(reason)),
    }
}

/// The error a one-shot or dry run fails with for a refused lock file.
pub fn refusal(reason: &str) -> ProfileError {
    ProfileError::RotateRefused(format!("rotate run lock {reason}; nothing was read"))
}

/// The checks, with the documented reasons, word for word.
fn check(m: &std::fs::Metadata) -> Result<(), &'static str> {
    if !m.is_file() {
        return Err("is not a regular file");
    }
    if m.uid() != amplihack_utils::secure_files::euid() {
        return Err("is not owned by you");
    }
    if m.mode() & 0o077 != 0 {
        return Err("is open to group or other users");
    }
    if m.nlink() != 1 {
        return Err("has more than one link");
    }
    Ok(())
}

/// Replaces the file's contents with this process's pid.
fn write_pid(file: &mut File) -> io::Result<()> {
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    writeln!(file, "{}", std::process::id())
}

/// The holder's pid: at most 32 bytes, trimmed, a `u32` other than 0.
/// Anything else is `None`.
fn read_pid(file: &mut File) -> Option<u32> {
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut raw = Vec::new();
    file.take(MAX_PID_BYTES + 1).read_to_end(&mut raw).ok()?;
    if raw.len() as u64 > MAX_PID_BYTES {
        return None;
    }
    let text = std::str::from_utf8(&raw).ok()?.trim();
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse::<u32>().ok().filter(|&p| p != 0)
}

#[cfg(all(test, target_os = "linux"))]
#[path = "tests_run_lock.rs"]
mod tests_run_lock;
