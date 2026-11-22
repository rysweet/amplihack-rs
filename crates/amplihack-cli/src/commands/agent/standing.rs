//! Issue #2912: a standing session keeps running.
//!
//! A session marked standing is not left stopped. When its Claude Code
//! process exits for any reason other than its owner ending it, the
//! wrapper starts it again in the same pane with `claude --continue`,
//! after waiting for a login with room if none has room at the exit.
//!
//! There is no count-based limit on relaunches. A count is a retry limit,
//! and the defect this fixes was a supervisor session held stopped for
//! fourteen hours by one. The brake is [`worth_relaunching`] instead: a
//! relaunch is made only when something has changed since the launch that
//! failed, which is either conversation the run added or capacity the wait
//! for room found. A process that starts and dies with nothing to show for
//! itself is therefore started again no more than once, and its failure is
//! handed to the supervisor as evidence.
//!
//! [`Standing`] is the whole of the decision: the wrapper calls
//! [`Standing::next`] once its child has returned and either starts the
//! session again with what it gets back, or stops. Every relaunch and
//! every refusal is written to the pane and to the supervisor log, and a
//! refusal that leaves a failing session stopped is written to the
//! session's own record as well.
//!
//! This is not part of the `limit-resume` hook, which answers the
//! usage-limit menu of a session that is still running. The two share the
//! same definition of a login with room and nothing else: this one covers
//! the case where the process has gone.

use super::entry;
use super::registry::{self, Entry, fmt_ts};
use super::report::{self, MAX_LINE, MAX_TEXT, sanitize};
use super::sessions;
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
    /// A relaunch is in question. Whether it is made is
    /// [`worth_relaunching`], which needs the room gate's answer and so
    /// cannot be settled until the wrapper has waited.
    Relaunch,
}

/// Whose stop an exit was, from three observations and nothing else. Pure,
/// and deliberately without a count: the resume cap this issue removes was
/// exactly such a count.
///
/// In order: a session that is not standing, and one whose wrapper was
/// signalled, are left alone; then a clean return is the owner ending the
/// session, as is an ended marker. Anything left is nobody's stop, and a
/// relaunch is in question.
///
/// `ok` is whether the launch returned `Ok`, `signalled` whether the
/// wrapper has had SIGHUP or SIGTERM, and `ended_marker` whether the
/// session's SessionEnd marker is on disk.
pub fn after_exit(standing: bool, ok: bool, signalled: bool, ended_marker: bool) -> AfterExit {
    if !standing || signalled {
        AfterExit::GoingAway
    } else if ok || ended_marker {
        AfterExit::OwnerEnded
    } else {
        AfterExit::Relaunch
    }
}

/// Whether a relaunch answers something new rather than repeating the run
/// that has just failed. This is the brake on the relaunch loop, and it is
/// read from the exit itself: no count of attempts, no timer and no poll.
///
/// Two observations make the next launch different from the last, and the
/// wrapper has both by the time it asks:
///
/// - `advanced`: the session's conversation grew during the run, so the run
///   did something and there is more for `--continue` to carry on. Whatever
///   failure comes next will be a new one.
/// - `room_appeared`: no login had room at the exit and the wrapper waited
///   until one did, so the capacity that was spent has been replaced. This
///   is the usage-limit case the feature exists for, and it is why a
///   process that dies in under a second on a limit wall is still started
///   again.
///
/// With neither, the process ran on a host that could run it and left
/// nothing behind: a configuration it cannot start under, a `--continue`
/// that resolves to nothing, a crash during start-up. Starting it again
/// would repeat it, so the wrapper stops, says why in the pane and the log,
/// and leaves the failure on the session's record for the supervisor and
/// for `amplihack agent status`.
pub fn worth_relaunching(advanced: bool, room_appeared: bool) -> bool {
    advanced || room_appeared
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
    /// The session's conversation holds more than it did before the launch:
    /// [`Standing::conversation_bytes`] taken after the exit is larger than
    /// the same measurement taken before the launch.
    pub advanced: bool,
}

/// Whether this session is standing, and the rotator root its room gate
/// reads. Settled once, before the first launch.
pub(super) struct Standing {
    on: bool,
    /// `None` when there is no rotator to wait for, or its root could not
    /// be found (a warning), so a relaunch never waits.
    root: Option<PathBuf>,
}

impl Standing {
    /// `--standing` or `--no-standing` if either was given, else the
    /// `agent.standing` user key. A key that cannot be read is a warning
    /// and `false`, so a file nobody can make sense of never starts agents
    /// unattended.
    pub(super) fn resolve(flag: Option<bool>, name: &str, home: &Path) -> Self {
        use super::sessions::root_error;
        use crate::user_config::{AGENT_STANDING, agent_standing};
        use amplihack_utils::rotator_status::locate_root;
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
        // A root that cannot be found means no wait for room. That changes
        // what a standing session does, so it is said, with the cause.
        let root = match locate_root(|k| std::env::var_os(k), Some(home)) {
            Ok((root, _)) => Some(root),
            Err(why) => {
                if on {
                    eprintln!(
                        "warning: the profile root could not be found ({}); \
                         standing agent {name} will be started again without \
                         waiting for a login with room",
                        root_error(&why)
                    );
                }
                None
            }
        };
        Self { on, root }
    }

    /// How many bytes of its own conversation this session has on disk: the
    /// transcript of the conversation its record names, in whichever
    /// configuration directory holds it. `0` when the session is not
    /// standing, when the record names no conversation, when no transcript
    /// is there, or when the configuration directories cannot be listed.
    ///
    /// The wrapper takes this before every launch and again after the exit.
    /// Growth across a run is the `advanced` of [`worth_relaunching`]: the
    /// evidence that the run did something a relaunch could continue. It is
    /// a measurement of work and not a count of attempts, so it never caps
    /// a session that is running: a session that is working grows it every
    /// turn, and one that starts and dies never does.
    ///
    /// It is read from the conversation the session's own record names, not
    /// from the launch directory as a whole, so a second session running in
    /// the same directory can neither pass this session off as working nor
    /// keep its relaunch loop alive.
    pub(super) fn conversation_bytes(
        &self,
        dir: &AgentsDir,
        home: &Path,
        launch_dir: &Path,
        name: &str,
    ) -> u64 {
        if !self.on {
            return 0;
        }
        let Some(id) = entry::conversation(dir, name) else {
            return 0;
        };
        let Ok(dirs) = sessions::config_dirs_from(home, |k| std::env::var_os(k)) else {
            return 0;
        };
        dirs.iter()
            .filter_map(|d| sessions::transcript_len(d, launch_dir, &id))
            .max()
            .unwrap_or(0)
    }

    /// The next launch of a standing session, or `None` to stop.
    ///
    /// `Some((launch_id, entry))` means start Claude Code again under that
    /// launch id, with the arguments rebuilt from that entry. Every
    /// relaunch, wait and refusal is logged; an owner-ended exit, a
    /// signalled wrapper and a session that is not standing are not, since
    /// each of those is a stop someone chose. A `no-progress` refusal is
    /// left on the session's record as well, because it stops a session its
    /// owner still wants running. `stopped` is the wrapper's flag for
    /// SIGHUP and SIGTERM, read again during and after a wait for room.
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
        match after_exit(self.on, exit.result.is_ok(), exit.signalled, marker) {
            AfterExit::OwnerEnded | AfterExit::GoingAway => return None,
            AfterExit::Relaunch => {}
        }
        let reason = exit_reason(exit.result);
        // Before the brake, because an exit with no room is the usage limit
        // this feature exists for: the wait is what makes the next launch
        // different from the one that failed.
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
        if !worth_relaunching(exit.advanced, waited == Room::Waited) {
            loudly(dir, &refused_line(registry::now(), name, "no-progress"));
            entry::note_refusal(dir, name, exit.launch_id, &no_progress_alert(&reason));
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
/// `no-progress`, `gone` or `superseded`.
pub fn refused_line(now: i64, name: &str, reason: &str) -> String {
    line(now, "refused", name, reason, None)
}

/// The text left in the session's registry record by a `no-progress`
/// refusal: the one refusal that leaves a session stopped while its owner
/// still wants it running. It names the exit and the evidence, so an
/// operator reading `amplihack agent status` has the cause without the log,
/// and the supervisor's pass has it on the entry it recovers.
pub fn no_progress_alert(reason: &str) -> String {
    let reason = sanitize(reason, MAX_TEXT);
    let whole = format!(
        "not relaunched: exited {reason} without advancing its conversation \
         while a login had room, so a relaunch would repeat it"
    );
    sanitize(&whole, MAX_LINE)
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
