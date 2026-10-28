//! Whether the background rotator (`amplihack profile rotate watch`) runs:
//! its pid file, `<root>/.auto-switch.d/rotate/rotate.pid`, checked against
//! `/proc`. The CLI's `rotate status` and `rotate stop` and the
//! limit-resume hook use the same check. Read-only.

use super::ledger::{Ledger, read_ledger};
use crate::secure_files::read_bounded_nofollow;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

const MAX_PID_FILE: u64 = 32;

/// What the pid file says about the rotator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonState {
    Running(u32),
    /// No pid file.
    Stopped,
    /// A pid file that is unreadable, malformed or names no rotator of this
    /// user.
    Stale,
    /// There is no `/proc` to check the pid against.
    Unverifiable,
}

/// `<root>/.auto-switch.d/rotate/rotate.pid`.
pub fn pid_path(root: &Path) -> PathBuf {
    root.join(".auto-switch.d")
        .join("rotate")
        .join("rotate.pid")
}

/// A valid pid file: at most 32 bytes of ASCII digits (one trailing newline
/// allowed) for a pid greater than 1 that is not `own`.
pub fn parse_pid(bytes: &[u8], own: u32) -> Option<u32> {
    if bytes.len() as u64 > MAX_PID_FILE {
        return None;
    }
    let digits = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let pid: u32 = std::str::from_utf8(digits).ok()?.parse().ok()?;
    (pid > 1 && pid != own).then_some(pid)
}

/// NUL-separated argv holding `profile`, `rotate` and `watch`, whose
/// `argv[0]` has the file name `exe_name` when one is given.
pub fn is_rotator(cmdline: &[u8], exe_name: Option<&OsStr>) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let mut args = cmdline.split(|&b| b == 0).filter(|a| !a.is_empty());
    let Some(argv0) = args.next() else {
        return false;
    };
    if exe_name.is_some_and(|name| Path::new(OsStr::from_bytes(argv0)).file_name() != Some(name)) {
        return false;
    }
    let rest: Vec<&[u8]> = args.collect();
    [&b"profile"[..], b"rotate", b"watch"]
        .iter()
        .all(|w| rest.contains(w))
}

/// Whether `pid` is a rotator owned by this user, per `proc_root`.
fn verified(proc_root: &Path, pid: u32, exe_name: Option<&OsStr>) -> bool {
    use crate::secure_files::owned_by_euid;
    let dir = proc_root.join(pid.to_string());
    std::fs::symlink_metadata(&dir).is_ok_and(|m| owned_by_euid(&m))
        && matches!(read_bounded_nofollow(&dir.join("cmdline"), 64 * 1024),
            Ok(Some((raw, _))) if is_rotator(&raw, exe_name))
}

/// The rotator's state from `pid_file` and `proc_root`. `exe_name`, when
/// given, must be the file name of the rotator's `argv[0]`.
pub fn daemon_state(pid_file: &Path, proc_root: &Path, exe_name: Option<&OsStr>) -> DaemonState {
    let raw = match read_bounded_nofollow(pid_file, MAX_PID_FILE) {
        Ok(None) => return DaemonState::Stopped,
        Ok(Some((raw, _))) => raw,
        Err(_) => return DaemonState::Stale,
    };
    if !proc_root.is_dir() {
        return DaemonState::Unverifiable;
    }
    match parse_pid(&raw, std::process::id()) {
        Some(pid) if verified(proc_root, pid, exe_name) => DaemonState::Running(pid),
        _ => DaemonState::Stale,
    }
}

/// The rotator for `root` cannot record a cycle the hook can use: no
/// rotator of this user runs (under any binary name), or its ledger exists
/// but cannot be read. Without a `/proc` only the ledger is checked.
pub fn rotator_down(root: &Path, now: i64) -> bool {
    rotator_down_at(root, Path::new("/proc"), now)
}

/// [`rotator_down`] against another `/proc`.
pub fn rotator_down_at(root: &Path, proc_root: &Path, now: i64) -> bool {
    match daemon_state(&pid_path(root), proc_root, None) {
        DaemonState::Running(_) | DaemonState::Unverifiable => {
            read_ledger(root, now) == Ledger::Unreadable
        }
        DaemonState::Stopped | DaemonState::Stale => true,
    }
}

/// The pid of a running rotator (per `pid_file` and `proc_root`) whose
/// executable is not `current_exe`: another amplihack binary, or this one
/// before it was replaced by an upgrade. Files are compared by device and
/// inode, so a binary replaced at the same path counts as another. A
/// rotator whose executable cannot be checked, or a missing `current_exe`,
/// counts as another binary.
pub fn foreign_rotator(
    pid_file: &Path,
    proc_root: &Path,
    current_exe: Option<&Path>,
) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    let DaemonState::Running(pid) = daemon_state(pid_file, proc_root, None) else {
        return None;
    };
    let id = |p: &Path| std::fs::metadata(p).ok().map(|m| (m.dev(), m.ino()));
    let theirs = id(&proc_root.join(pid.to_string()).join("exe"));
    let ours = current_exe.and_then(id);
    match (theirs, ours) {
        (Some(a), Some(b)) if a == b => None,
        _ => Some(pid),
    }
}
