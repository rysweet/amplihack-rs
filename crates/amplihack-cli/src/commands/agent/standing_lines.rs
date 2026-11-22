//! Every line a standing session writes, in the form documented in
//! docs/howto/keep-sessions-running.md (issue #2912).
//!
//! All of it reaches a live terminal as well as the supervisor log, so the
//! session name and the exit reason are cut to [`MAX_TEXT`] characters and
//! every line is sanitised: no control character of either may survive to
//! forge a line or an escape sequence.

use super::relaunch::Lost;
use super::{Evidence, Progress};
use crate::commands::agent::registry::fmt_ts;
use crate::commands::agent::report::{self, MAX_LINE, MAX_TEXT, sanitize};
use crate::commands::agent::store::AgentsDir;
use amplihack_utils::rotator_status::MAX_WAIT;

/// Writes one standing-session line to the pane and to the supervisor log.
/// Loud by design: a session that was started again, or that was not, is
/// something an operator must be able to find afterwards. A log that cannot
/// be written is not allowed to stop the relaunch, so it is a debug line.
pub(super) fn loudly(dir: &AgentsDir, line: &str) {
    eprintln!("{line}");
    if let Err(e) = report::append(&dir.log(), line) {
        tracing::debug!("agent: could not log a standing-session line: {e}");
    }
}

/// The reason text of a relaunch line: `exit-status-<n>`, or the name of
/// the signal that killed Claude Code, which the launch path reports as
/// `128 + N`. An error that is not an exit status is a launch that never
/// reached a child.
pub(super) fn exit_reason(result: &anyhow::Result<()>) -> String {
    let Err(e) = result else {
        return "exit-status-0".to_string();
    };
    let Some(code) = crate::command_error::exit_code(e) else {
        return "launch-failed".to_string();
    };
    match code {
        129..=255 => amplihack_utils::launch_target::signal_name(code - 128).to_string(),
        _ => format!("exit-status-{code}"),
    }
}

/// `<ts> standing session relaunch session=<name> reason=<reason>
/// waited=<room|none>`, written to the pane and to the supervisor log
/// before a standing session is started again. `reason` is the exit:
/// `exit-status-<n>` or a signal name. Documented in
/// docs/howto/keep-sessions-running.md.
pub fn relaunch_line(now: i64, name: &str, reason: &str, waited: bool) -> String {
    let waited = if waited { "room" } else { "none" };
    line(now, "relaunch", name, reason, Some(waited))
}

/// `<ts> standing session refused session=<name> reason=<reason>`: the
/// wrapper of a standing session did not start it again. `reason` is
/// `no-progress`, `gone` or `superseded`.
pub fn refused_line(now: i64, name: &str, reason: &str) -> String {
    line(now, "refused", name, reason, None)
}

/// The text left in the session's registry record by a `no-progress`
/// refusal, which leaves a session stopped while its owner still wants it
/// running.
///
/// It names the exit, that the exit was of a relaunch given on nothing but
/// the rule that every exit is given one, what was read of the
/// conversation (that the model wrote no reply to it during the run, or
/// that there was none of this session's to read), and what the room the
/// refusal rests on was read from. So an operator reading `amplihack agent
/// status` has the cause without the log, and the supervisor's pass has it
/// on the entry it recovers, including whether a usage limit was ruled out
/// by the rotator or only by a swap record older than the exit.
pub fn no_progress_alert(reason: &str, progress: Progress, evidence: Evidence) -> String {
    let reason = sanitize(reason, MAX_TEXT);
    let measured = match progress {
        Progress::Unmeasured => "with no conversation of its own on record",
        Progress::Advanced | Progress::Idle => "without a reply from the model in its conversation",
    };
    let room = match evidence {
        Evidence::Unchecked => "while a login had room",
        Evidence::Record => {
            "while the last swap record showed a login with room and no rotator \
             reading after the exit could be had"
        }
        Evidence::Reading => {
            "while the rotator's first reading after the exit found a login with room"
        }
    };
    let whole = format!(
        "not relaunched: the relaunch of an exit that showed nothing itself \
         exited {reason} {measured} {room}, so another relaunch would repeat it"
    );
    sanitize(&whole, MAX_LINE)
}

/// The text left in the session's registry record by a `gone` refusal that
/// no signal caused: no login had room when the session's process exited,
/// and the wait for one ended without it. This too leaves a session
/// stopped while its owner wants it running, so it names the exit and why
/// the wait ended, for `amplihack agent status` and the supervisor.
pub fn gone_alert(reason: &str, why: Lost) -> String {
    let reason = sanitize(reason, MAX_TEXT);
    let why = match why {
        Lost::Deadline => format!(
            "the wait for one reached its limit of {} days",
            MAX_WAIT.as_secs() / 86_400
        ),
        Lost::Watch => "the watch on the rotator's swap record was lost".to_string(),
        Lost::Unarmed => "the rotator's swap record could not be watched".to_string(),
        Lost::Unwatchable => {
            "the rotator's directory is missing or not private to this user".to_string()
        }
        Lost::Rotator => "the rotator stopped before one appeared".to_string(),
    };
    let whole = format!(
        "not relaunched: it exited {reason} while no login had room, and {why}, \
         so the session was left for the supervisor"
    );
    sanitize(&whole, MAX_LINE)
}

/// `<ts> standing session waiting session=<name> reason=<reason>`: the
/// relaunch is held, so an operator looking at a quiet pane can tell a
/// session waiting from one that has stopped. `reason` is `no-room` when no
/// login has room and the wrapper waits until one does, and
/// `room-unconfirmed` when the brake would refuse on room that only a swap
/// record older than the exit showed, and the wrapper waits for the
/// rotator's next reading. Written once per wait and reason, after the
/// watch on the rotate directory is armed.
pub fn waiting_line(now: i64, name: &str, reason: &str) -> String {
    line(now, "waiting", name, reason, None)
}

/// Every standing-session line. The name and the reason are cut to
/// [`MAX_TEXT`] characters and the whole line is sanitised: it reaches a
/// live terminal as well as the log, so no control character of either may
/// survive to forge a line or an escape sequence.
fn line(now: i64, verb: &str, name: &str, reason: &str, waited: Option<&str>) -> String {
    let name = sanitize(name, MAX_TEXT);
    let reason = sanitize(reason, MAX_TEXT);
    let tail = waited.map(|w| format!(" waited={w}")).unwrap_or_default();
    let whole = format!(
        "{} standing session {verb} session={name} reason={reason}{tail}",
        fmt_ts(now)
    );
    sanitize(&whole, MAX_LINE)
}
