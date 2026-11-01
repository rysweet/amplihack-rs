//! `setting-changes.log` (issue #1803): which process changed the autostart
//! or auto-switch setting. A trace, not an audit record; see "Setting change
//! log" in docs/reference/profile-command.md.
//!
//! `exe=` stays the last field, so a path with spaces cannot pose as another
//! field. No environment value, argument or account is written.

use super::ledger::RotateState;
use super::log::RotateLog;
use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::usage_cmd::sanitize_display;
use std::io;
use std::path::Path;

const FILE: &str = "setting-changes.log";
const ROTATED: &str = "setting-changes.log.1";

/// The `pid= ppid= exe=` tail of a line, on one line; `exe=-` without a path.
pub fn fields(pid: u32, ppid: u32, exe: Option<&Path>) -> String {
    let exe = exe.map_or_else(
        || "-".to_string(),
        |p| sanitize_display(&p.to_string_lossy(), false),
    );
    format!("pid={pid} ppid={ppid} exe={exe}")
}

/// Appends `<time> <event> pid=.. ppid=.. exe=..`. The root must be private;
/// the state directories are created if needed.
pub fn record(root: &Path, event: &str) -> io::Result<()> {
    let rs = RotateState::new(root);
    rs.ensure().map_err(|e| io::Error::other(e.to_string()))?;
    let mut log = RotateLog::open(&rs.dir.join(FILE), &rs.dir.join(ROTATED))?;
    // SAFETY: getppid has no preconditions.
    let ppid = unsafe { libc::getppid() } as u32;
    let exe = std::env::current_exe().ok();
    let tail = fields(std::process::id(), ppid, exe.as_deref());
    log.write(now(), &format!("{event} {tail}"))
}

/// [`record`] for a command that has already changed the setting: a failed
/// log write warns on stderr and the command still succeeds.
pub fn record_or_warn(root: &Path, event: &str) {
    if let Err(e) = record(root, event) {
        eprintln!(
            "warning: setting change not logged: {}",
            sanitize_display(&e.to_string(), false)
        );
    }
}
