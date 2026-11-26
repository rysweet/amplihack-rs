//! Issue #2912: a standing session keeps running.
//!
//! A session marked standing is not left stopped. When its Claude Code
//! process exits for any reason other than its owner ending it, the
//! wrapper starts it again in the same pane with `claude --continue`,
//! after waiting for a login with room if none has room at the exit.
//!
//! The wrapper does not judge whether a chain of relaunches is getting
//! anywhere. It keeps no count that can stop a session and has no brake:
//! a session whose process exits is relaunched every time, however often
//! that is. The defect this fixes was a supervisor session held stopped
//! for fourteen hours by a count, and a brake in the wrapper, whatever it
//! reads, stops a session in the same way.
//!
//! What the wrapper does instead is leave the evidence where the supervisor
//! agent reads it. Every relaunch is written to the pane, to the supervisor
//! log and to the session's own record, and the record says how many
//! relaunches there have been since the model last replied in the
//! session's conversation, and since when ([`Quiet`]). A session that keeps
//! exiting without a reply is either a start-up crash loop or a host that
//! keeps killing it. Which of the two it is, and whether to stop it, is the
//! supervisor agent's decision, made from that record, the supervisor log
//! and the transcript; docs/howto/keep-sessions-running.md says what to read
//! and how to stop a session.
//!
//! Two exits are not relaunched, and each rests on a fact about the exit
//! rather than on a number of attempts:
//!
//! - the launch returned an error that is not an exit status of Claude
//!   Code, so the launch path itself failed and there is no process exit
//!   to answer. Starting it again would fail the same way within moments,
//!   and with no Claude Code start to pace it the loop would spin;
//! - the wait for a login with room ended without one.
//!
//! Each writes a `refused` line, leaves its reason on the session's record
//! and leaves the record `active`, so the supervisor's own recovery is the
//! next thing to try.
//!
//! [`Standing`] is the whole of the decision: the wrapper calls
//! [`Standing::next`] once its child has returned and either starts the
//! session again with what it gets back, or stops.
//!
//! This is not part of the `limit-resume` hook, which answers the
//! usage-limit menu of a session that is still running. The two share the
//! same definition of a login with room and nothing else: this one covers
//! the case where the process has gone.

use super::registry::Entry;

/// The room gate of a relaunch: whether a login has room, and the wait for
/// one. A child of this module because nothing else needs it.
#[path = "relaunch.rs"]
mod relaunch;

/// The wrapper-facing half: the session's standing status, the room gate
/// it reads and the relaunch it decides on. A separate file because every
/// file in `agent/` is held to 300 lines (D23, enforced by
/// `tests/issue_1760_agent_source_scan.rs`).
#[path = "standing_run.rs"]
mod run;

/// Every line this feature writes to the pane, to the supervisor log and
/// to the session's record, and the exit text they name.
#[path = "standing_lines.rs"]
mod lines;

/// Whether the model replied during a run, read from the session's
/// transcript after the exit, for the record of the relaunch.
#[path = "standing_progress.rs"]
mod progress;

pub(super) use run::Standing;

/// What the wrapper does once its Claude Code process has returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterExit {
    /// The owner ended the session: a clean return, or its ended marker.
    OwnerEnded,
    /// The pane or the host is going away, or the session is not standing.
    GoingAway,
    /// The launch returned an error that is not an exit status of Claude
    /// Code: no process exited, so there is nothing to relaunch.
    LaunchFailed,
    /// Claude Code exited and nobody ended it: the session is relaunched,
    /// once a login has room.
    Relaunch,
}

/// Whose stop an exit was, from what the wrapper saw of it and nothing
/// else. Pure, and without a count: the resume cap this issue removes was
/// exactly such a count.
///
/// In order: a session that is not standing, and one whose wrapper was
/// signalled, are left alone; then a clean return is the owner ending the
/// session, as is an ended marker; then an error that carries no exit
/// status is a launch that failed. Anything left is a process exit nobody
/// chose, and it is relaunched.
///
/// `signalled` is whether the wrapper has had SIGHUP or SIGTERM,
/// `ended_marker` whether the session's SessionEnd marker is on disk, and
/// `result` what the launch returned. Claude Code's own exit status comes
/// back as an error that carries it (`command_error::exit_code`); a signal
/// that killed it comes back as the status `128 + N`.
pub fn after_exit(
    standing: bool,
    signalled: bool,
    ended_marker: bool,
    result: &anyhow::Result<()>,
) -> AfterExit {
    if !standing || signalled {
        AfterExit::GoingAway
    } else if result.is_ok() || ended_marker {
        AfterExit::OwnerEnded
    } else if result
        .as_ref()
        .is_err_and(|e| crate::command_error::exit_code(e).is_none())
    {
        AfterExit::LaunchFailed
    } else {
        AfterExit::Relaunch
    }
}

/// The relaunches of a session since the model last replied in its
/// conversation, and when the first of them was made.
///
/// This is evidence for the supervisor agent and nothing else: the wrapper
/// writes it on the session's record with each relaunch, and no decision
/// in the wrapper reads it. A long chain made in minutes is the shape of a
/// start-up crash loop; a short one spread over hours is the shape of a
/// host that keeps killing an idle session. Telling them apart, and acting,
/// is left to the agent, which can read the transcript as well.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quiet {
    /// Unix seconds of the first relaunch since the model last replied.
    pub since: i64,
    /// The relaunches since the model last replied, this one included.
    pub relaunches: u32,
}

impl Quiet {
    /// The chain after a run that ended before the relaunch made at `now`:
    /// `None` when the model replied during the run, otherwise `before`
    /// with this relaunch added, or a chain that starts with it.
    pub fn after(before: Option<Self>, replied: bool, now: i64) -> Option<Self> {
        if replied {
            return None;
        }
        Some(match before {
            Some(q) => Self {
                since: q.since,
                relaunches: q.relaunches.saturating_add(1),
            },
            None => Self {
                since: now,
                relaunches: 1,
            },
        })
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
    /// The model wrote a reply in the session's conversation during the
    /// run: [`Standing::replied`], read after the exit. It goes on the
    /// record of the relaunch and decides nothing.
    pub replied: bool,
    /// The chain this run belonged to, from the [`Relaunch`] that started
    /// it; `None` for the first run of a wrapper and after a reply.
    pub quiet: Option<Quiet>,
}

/// The next launch of a standing session: what [`Standing::next`] hands
/// back to the wrapper's loop.
pub(super) struct Relaunch {
    /// The launch id this run is recorded under, taken from the entry under
    /// the registry's lock.
    pub launch_id: String,
    /// The entry the run's Claude Code arguments are rebuilt from.
    pub entry: Entry,
    /// The chain this relaunch extends, which the wrapper carries into the
    /// next [`Exit::quiet`] so the record can say how long it has run.
    pub quiet: Option<Quiet>,
}

#[cfg(test)]
#[path = "tests_standing.rs"]
mod tests_standing;
#[cfg(test)]
#[path = "tests_standing_lines.rs"]
mod tests_standing_lines;
#[cfg(test)]
#[path = "tests_standing_progress.rs"]
mod tests_standing_progress;
