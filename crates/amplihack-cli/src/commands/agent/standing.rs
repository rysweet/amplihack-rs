//! Issue #2912: a standing session keeps running.
//!
//! A session marked standing is not left stopped. When its Claude Code
//! process exits for any reason other than its owner ending it, the
//! wrapper starts it again in the same pane with `claude --continue`,
//! after waiting for a login with room if none has room at the exit.
//!
//! There is no count-based limit on relaunches. A count is a retry limit,
//! and the defect this fixes was a supervisor session held stopped for
//! fourteen hours by one. The brakes are the owner's own exit, a session
//! whose Claude Code never started, and the wait for room.
//!
//! [`Standing`] is the whole of the decision: the wrapper calls
//! [`Standing::next`] once its child has returned and either starts the
//! session again with what it gets back, or stops. Every relaunch and
//! every refusal is written to the pane and to the supervisor log.
//!
//! This is not part of the `limit-resume` hook, which answers the
//! usage-limit menu of a session that is still running. The two share the
//! same definition of a login with room and nothing else: this one covers
//! the case where the process has gone.

use super::entry;
use super::registry::{self, Entry, fmt_ts};
use super::report::{self, MAX_LINE, MAX_TEXT, sanitize};
use super::store::AgentsDir;
use relaunch::Room;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// The room gate of a relaunch: whether a login has room, and the wait for
/// one. A child of this module because nothing else needs it.
#[path = "relaunch.rs"]
mod relaunch;

/// What the wrapper does once its Claude Code process has returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterExit {
    /// The owner ended the session: a clean return, or its ended marker.
    OwnerEnded,
    /// The pane or the host is going away, or the session is not standing.
    GoingAway,
    /// Start the session again, continuing its conversation.
    Relaunch,
    /// Claude Code never started, so there is no conversation to continue
    /// and starting it again would only repeat the failure.
    NeverStarted,
}

/// What to do after a Claude Code process returned, from four observations
/// and nothing else. Pure, and deliberately without a count: the resume cap
/// this issue removes was exactly such a count.
///
/// In order: a session that is not standing, and one whose wrapper was
/// signalled, are left alone; then a clean return is the owner ending the
/// session, as is an ended marker; then a session whose Claude never
/// started is refused. Anything left is a relaunch.
///
/// `ok` is whether the launch returned `Ok`, `signalled` whether the
/// wrapper has had SIGHUP or SIGTERM, `ended_marker` whether the session's
/// SessionEnd marker is on disk, and `saw_claude` whether the id thread
/// found a Claude Code process below this one during the run.
pub fn after_exit(
    standing: bool,
    ok: bool,
    signalled: bool,
    ended_marker: bool,
    saw_claude: bool,
) -> AfterExit {
    if !standing || signalled {
        AfterExit::GoingAway
    } else if ok || ended_marker {
        AfterExit::OwnerEnded
    } else if !saw_claude {
        AfterExit::NeverStarted
    } else {
        AfterExit::Relaunch
    }
}

/// What the wrapper saw of one run of Claude Code, for [`Standing::next`].
pub(super) struct Exit<'a> {
    /// The launch id the run was started under.
    pub launch_id: &'a str,
    /// Unix seconds, taken just before the run's registry write; an ended
    /// marker older than this belongs to an earlier run.
    pub since: i64,
    /// The conversation `--resume` named at the first launch, if any.
    pub first_conversation: Option<&'a str>,
    /// What the launch returned.
    pub result: &'a anyhow::Result<()>,
    /// The wrapper has had SIGHUP or SIGTERM.
    pub signalled: bool,
    /// The id thread found a Claude Code process below the wrapper.
    pub saw_claude: bool,
}

/// Whether this session is standing, and the rotator root its room gate
/// reads. Settled once, before the first launch.
pub(super) struct Standing {
    on: bool,
    /// `None` when there is no rotator to wait for, so a relaunch never
    /// waits.
    root: Option<PathBuf>,
}

impl Standing {
    /// `--standing` or `--no-standing` if either was given, else the
    /// `agent.standing` user key. A key that cannot be read is a warning
    /// and `false`, so a file nobody can make sense of never starts agents
    /// unattended.
    pub(super) fn resolve(flag: Option<bool>, name: &str, home: &Path) -> Self {
        use crate::user_config::{AGENT_STANDING, agent_standing};
        let on = flag.unwrap_or_else(|| match agent_standing(home) {
            Ok(on) => on,
            Err(why) => {
                eprintln!(
                    "warning: {AGENT_STANDING} could not be read ({why}); \
                     agent {name} is not standing"
                );
                false
            }
        });
        let root =
            amplihack_utils::rotator_status::locate_root(|k| std::env::var_os(k), Some(home))
                .ok()
                .map(|(root, _)| root);
        Self { on, root }
    }

    /// The next launch of a standing session, or `None` to stop.
    ///
    /// `Some((launch_id, entry))` means start Claude Code again under that
    /// launch id, with the arguments rebuilt from that entry. Every
    /// relaunch, wait and refusal is logged; an owner-ended exit, a
    /// signalled wrapper and a session that is not standing are not, since
    /// each of those is a stop someone chose. `stopped` is the wrapper's flag
    /// for SIGHUP and SIGTERM, read again during and after a wait for room.
    pub(super) fn next(
        &self,
        dir: &AgentsDir,
        name: &str,
        exit: &Exit<'_>,
        stopped: &AtomicBool,
    ) -> Option<(String, Entry)> {
        let marker = entry::owner_ended(
            dir,
            name,
            exit.launch_id,
            exit.first_conversation,
            exit.since,
        );
        let decision = after_exit(
            self.on,
            exit.result.is_ok(),
            exit.signalled,
            marker,
            exit.saw_claude,
        );
        match decision {
            AfterExit::OwnerEnded | AfterExit::GoingAway => return None,
            AfterExit::NeverStarted => {
                loudly(dir, &refused_line(registry::now(), name, "never-started"));
                return None;
            }
            AfterExit::Relaunch => {}
        }
        let reason = exit_reason(exit.result);
        let waited = match self.root.as_deref() {
            Some(root) => relaunch::wait_for_room_until(root, stopped, || {
                loudly(dir, &waiting_line(registry::now(), name));
            }),
            None => Room::Now,
        };
        if waited == Room::Gone || stopped.load(Ordering::SeqCst) {
            loudly(dir, &refused_line(registry::now(), name, "gone"));
            return None;
        }
        // One pane per session: the relaunch happens only while the entry
        // is still this launch's. If the supervisor rebuilt the session in
        // another window while this wrapper waited for room, the launch id
        // has moved on and the relaunch is refused rather than making a
        // second Claude Code run against one conversation.
        let Some((next_id, taken)) = entry::reactivate(dir, name, exit.launch_id) else {
            loudly(dir, &refused_line(registry::now(), name, "superseded"));
            return None;
        };
        loudly(
            dir,
            &relaunch_line(registry::now(), name, &reason, waited == Room::Waited),
        );
        Some((next_id, taken))
    }
}

/// Writes one standing-session line to the pane and to the supervisor log.
/// Loud by design: a session that was started again, or that was not, is
/// something an operator must be able to find afterwards. A log that cannot
/// be written is not allowed to stop the relaunch, so it is a debug line.
fn loudly(dir: &AgentsDir, line: &str) {
    eprintln!("{line}");
    if let Err(e) = report::append(&dir.log(), line) {
        tracing::debug!("agent: could not log a standing-session line: {e}");
    }
}

/// The reason text of a relaunch line: `exit-status-<n>`, or the name of
/// the signal that killed Claude Code, which the launch path reports as
/// `128 + N`. An error that is not an exit status is a launch that never
/// reached a child.
fn exit_reason(result: &anyhow::Result<()>) -> String {
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
/// `never-started`, `gone` or `superseded`.
pub fn refused_line(now: i64, name: &str, reason: &str) -> String {
    line(now, "refused", name, reason, None)
}

/// `<ts> standing session waiting session=<name> reason=no-room`: no login
/// had room when the session's process exited, so the relaunch is held
/// until one appears. Written once per wait, after the watch on the swap
/// record is armed, so an operator looking at a quiet pane can tell a
/// session waiting for capacity from one that has stopped.
pub fn waiting_line(now: i64, name: &str) -> String {
    line(now, "waiting", name, "no-room", None)
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

#[cfg(test)]
#[path = "tests_standing.rs"]
mod tests_standing;
#[cfg(test)]
#[path = "tests_standing_lines.rs"]
mod tests_standing_lines;
