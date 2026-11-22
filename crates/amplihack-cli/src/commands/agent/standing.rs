//! Issue #2912: a standing session keeps running.
//!
//! A session marked standing is not left stopped. When its Claude Code
//! process exits for any reason other than its owner ending it, the
//! wrapper starts it again in the same pane with `claude --continue`,
//! after waiting for a login with room if none has room at the exit.
//!
//! There is no count-based limit on relaunches. A count is a retry limit,
//! and the defect this fixes was a supervisor session held stopped for
//! fourteen hours by one. The brake is [`worth_relaunching`] instead, and
//! what it judges is the relaunch chain rather than the run:
//!
//! - Every exit is given a relaunch. A session that sat idle for hours and
//!   was then killed by the host, by the out-of-memory killer or by a pane
//!   that went away has nothing to show for its run, and is started again
//!   all the same, because "this run did no work" and "this start is
//!   failing" are different facts.
//! - A relaunch given on nothing but that rule, when the model wrote no
//!   reply during the run and no room had to be waited for, is a trial. If
//!   the run it starts comes back the same way, the chain is a start-up
//!   crash loop and the wrapper stops: one more start would repeat the one
//!   that just failed.
//!
//! So a process that cannot start is started again once and then left to
//! the supervisor with its failure on its own record as evidence, while a
//! session that does work, or that waits for capacity, is relaunched as
//! often as it exits.
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

/// Whether a run did any work: a reply the model wrote to the session's
/// conversation during the run, read from its transcript after the exit.
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

/// What one run did in its session's conversation, read by
/// [`Standing::progress`] after the exit (issue #2912).
///
/// It is read from replies the model wrote, not from the size of the
/// transcript. Claude Code appends bookkeeping lines on a start that does
/// no work, and a request that fails is written as an assistant line of its
/// own, so a start-up crash loop grows the transcript on every start; see
/// `standing_progress.rs` for what was observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// The conversation holds a reply the model wrote during the run: the
    /// run did a turn of work, and a relaunch has more to carry on from
    /// than the last one had.
    Advanced,
    /// The conversation holds no such reply, or has no transcript: the run
    /// left nothing a `--continue` could carry on.
    Idle,
    /// There is no conversation of this session's to read, so the run is
    /// neither known to have worked nor known to have been idle. A record
    /// names a conversation only once the wrapper's id thread has found the
    /// Claude process and its sessions file, and it names none while
    /// another active agent holds that conversation (C3).
    Unmeasured,
}

impl Progress {
    /// From whether the session's conversation holds a reply the model
    /// wrote during the run. `None` is the absence of a conversation to
    /// read, not a conversation with nothing in it.
    pub fn of(replied: Option<bool>) -> Self {
        match replied {
            Some(true) => Self::Advanced,
            Some(false) => Self::Idle,
            None => Self::Unmeasured,
        }
    }
}

/// Whether a relaunch answers something new rather than repeating the
/// launch that has just failed. This is the brake on the relaunch loop, and
/// every input is read from the exit itself: no count of attempts, no timer
/// and no poll.
///
/// A relaunch is made unless all three of these are against it:
///
/// - `progress`: [`Progress::Advanced`] means the model wrote a reply to
///   the session's conversation during the run, so the run did something
///   and there is more for `--continue` to carry on. Whatever failure comes
///   next will be a new one. Lines Claude Code writes without a turn, and
///   the assistant line it writes for a failed request, are not replies.
/// - `room_appeared`: no login had room at the exit and the wrapper waited
///   until one did, so the capacity that was spent has been replaced. This
///   is the usage-limit case the feature exists for, and it is why a
///   process that dies in under a second on a limit wall is still started
///   again.
/// - `on_trial`: the run that has just exited was itself started by a
///   relaunch that had neither of the above behind it. That relaunch was
///   the one trial an exit with nothing to show is given; the run it
///   started has now come back with nothing to show either, so the chain,
///   and not just this run, is getting nowhere.
///
/// An exit that is not on trial is therefore always relaunched, whatever it
/// has to show. A session killed while idle, by the out-of-memory killer,
/// by a host event or by a pane that went away, is exactly that case, and
/// it is the one the issue was reported from.
///
/// With all three against it, the process ran on a host that could run it,
/// was started again for that reason alone, and left nothing behind twice:
/// a configuration it cannot start under, a `--continue` that resolves to
/// nothing, a crash during start-up. Starting it again would repeat it, so
/// the wrapper stops, says why in the pane and the log, and leaves the
/// failure on the session's record for the supervisor and for `amplihack
/// agent status`.
pub fn worth_relaunching(on_trial: bool, progress: Progress, room_appeared: bool) -> bool {
    !on_trial || progress == Progress::Advanced || room_appeared
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
    /// What the run did in the session's conversation: [`Standing::progress`]
    /// read after the exit, for replies dated from just before the launch.
    pub progress: Progress,
    /// This run was started by a relaunch that had nothing behind it but
    /// the rule that every exit is given one: the trial of
    /// [`worth_relaunching`]. `false` for the first run of a wrapper, and
    /// for any run started because the model replied during the run before
    /// it or because room appeared.
    pub on_trial: bool,
}

/// The next launch of a standing session: what [`Standing::next`] hands
/// back to the wrapper's loop.
pub(super) struct Relaunch {
    /// The launch id this run is recorded under, taken from the entry under
    /// the registry's lock.
    pub launch_id: String,
    /// The entry the run's Claude Code arguments are rebuilt from.
    pub entry: Entry,
    /// This relaunch is a trial: nothing was behind it but the rule that
    /// every exit is given one. The wrapper carries it into the next
    /// [`Exit::on_trial`], which is what lets the brake judge the chain.
    pub on_trial: bool,
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
