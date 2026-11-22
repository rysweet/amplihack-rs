//! The standing status of one session, the room gate of its relaunch and
//! the decision the wrapper acts on (issue #2912).
//!
//! The rule the decision applies is in the parent module: [`after_exit`]
//! for whose stop an exit was, and [`worth_relaunching`] for whether a
//! relaunch answers anything new. What is here is the reading of the world
//! that rule needs, which is the session's record, the model's replies in
//! the transcript of its conversation and the rotator's swap record.

use super::lines::{
    exit_reason, gone_alert, loudly, no_progress_alert, refused_line, relaunch_line, waiting_line,
};
use super::progress::replied_since;
use super::recheck::{Recheck, recheck};
use super::relaunch::{self, Lost, Room};
use super::{AfterExit, Evidence, Exit, Progress, Relaunch, after_exit, worth_relaunching};
use crate::commands::agent::registry;
use crate::commands::agent::store::AgentsDir;
use crate::commands::agent::{entry, sessions};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

/// Whether this session is standing, and the rotator root its room gate
/// reads. Settled once, before the first launch.
pub(in crate::commands::agent) struct Standing {
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
    pub(in crate::commands::agent) fn resolve(flag: Option<bool>, name: &str, home: &Path) -> Self {
        use crate::commands::agent::sessions::root_error;
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

    /// What the run that began at `started` did in this session's
    /// conversation: [`Progress::Advanced`] when the transcript of the
    /// conversation its record names, in whichever configuration directory
    /// holds it, has a reply the model wrote at or after `started`.
    ///
    /// The wrapper reads this once, after the exit, with `started` taken
    /// just before the launch. A reply is the evidence that the run did
    /// something a relaunch could continue: the [`Progress::Advanced`] of
    /// [`worth_relaunching`]. It is a reading of work and not a count of
    /// attempts, so it never caps a session that is running: a session
    /// that is working writes a reply every turn. The size of the
    /// transcript is not read, because a start that does nothing and a
    /// request that fails both grow it (see `standing_progress.rs`).
    ///
    /// [`Progress::Unmeasured`] when the session is not standing, its record
    /// names no conversation, or the configuration directories cannot be
    /// listed. [`no_progress_alert`] says which a refusal saw, because "the
    /// model wrote nothing" and "there is no conversation of this session's
    /// to look at" are different facts about a run. Neither is evidence
    /// that the run did something, so neither relaunches a run that is
    /// already on trial.
    ///
    /// It is read from the conversation the session's own record names, not
    /// from the launch directory as a whole, so a second session running in
    /// the same directory can neither pass this session off as working nor
    /// keep its relaunch loop alive.
    pub(in crate::commands::agent) fn progress(
        &self,
        dir: &AgentsDir,
        home: &Path,
        launch_dir: &Path,
        name: &str,
        started: SystemTime,
    ) -> Progress {
        Progress::of(self.replied(dir, home, launch_dir, name, started))
    }

    /// Whether the session's conversation has a reply dated at or after
    /// `started`; `None` when there is no conversation of its own to read.
    fn replied(
        &self,
        dir: &AgentsDir,
        home: &Path,
        launch_dir: &Path,
        name: &str,
        started: SystemTime,
    ) -> Option<bool> {
        if !self.on {
            return None;
        }
        let id = entry::conversation(dir, name)?;
        let dirs = sessions::config_dirs_from(home, |k| std::env::var_os(k)).ok()?;
        Some(
            dirs.iter()
                .filter_map(|d| sessions::transcript_path(d, launch_dir, &id))
                .filter_map(|p| File::open(p).ok())
                .any(|f| replied_since(BufReader::new(f), started)),
        )
    }

    /// The next launch of a standing session, or `None` to stop.
    ///
    /// A [`Relaunch`] means start Claude Code again under its launch id,
    /// with the arguments rebuilt from its entry. Every relaunch, wait and
    /// refusal is logged; an owner-ended exit, a signalled wrapper and a
    /// session that is not standing are not, since each of those is a stop
    /// someone chose. A `no-progress` refusal, and a `gone` refusal that no
    /// signal caused, are left on the session's record as well, because
    /// each stops a session its owner still wants running. `stopped` is the
    /// wrapper's flag for SIGHUP and SIGTERM, read again during and after a
    /// wait for room.
    ///
    /// Before a `no-progress` refusal, a running rotator is asked whether
    /// the room the refusal rests on was there after the exit; see
    /// `standing_recheck.rs`.
    pub(in crate::commands::agent) fn next(
        &self,
        dir: &AgentsDir,
        name: &str,
        exit: &Exit<'_>,
        stopped: &AtomicBool,
    ) -> Option<Relaunch> {
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
        let exited = registry::now();
        let reason = exit_reason(exit.result);
        // Before the brake, because an exit with no room is the usage limit
        // this feature exists for: the wait is what makes the next launch
        // different from the one that failed.
        let waited = match self.root.as_deref() {
            Some(root) => relaunch::wait_for_room_until(root, stopped, || {
                loudly(dir, &waiting_line(registry::now(), name, "no-room"));
            }),
            None => Room::Now,
        };
        // A signal during or just before the wait is the pane or the host
        // going away: a stop someone chose, so it is logged and no more.
        if waited == Room::Stopped || stopped.load(Ordering::SeqCst) {
            gone(dir, name, exit, &reason, None);
            return None;
        }
        // The wait ended with no room and no one asked it to: the deadline
        // passed, or the swap record could not be watched. The owner still
        // wants the session running, so the cause goes on its record as a
        // `no-progress` refusal's does, and the record is left active for
        // the supervisor's recovery. That starts the session in a new
        // wrapper, which waits for room again should the run exit with none.
        if let Room::Gone(why) = waited {
            gone(dir, name, exit, &reason, Some(why));
            return None;
        }
        let mut room = waited == Room::Waited;
        if !worth_relaunching(exit.on_trial, exit.progress, room) {
            // The refusal would say a login had room, and the swap record
            // that says so is written only when the rotator swaps: it may
            // be older than the run and blind to a limit the run has just
            // met. A running rotator is asked first.
            let checked = match self.root.as_deref() {
                Some(root) => recheck(root, exit.since, exited, stopped, |why| {
                    loudly(dir, &waiting_line(registry::now(), name, why));
                }),
                None => Recheck::Refuse(Evidence::Unchecked),
            };
            match checked {
                Recheck::Appeared => room = true,
                Recheck::Refuse(evidence) => {
                    loudly(dir, &refused_line(registry::now(), name, "no-progress"));
                    let alert = no_progress_alert(&reason, exit.progress, evidence);
                    entry::note_refusal(dir, name, exit.launch_id, &alert);
                    return None;
                }
                Recheck::Stopped => {
                    gone(dir, name, exit, &reason, None);
                    return None;
                }
                Recheck::Gone(why) => {
                    gone(dir, name, exit, &reason, Some(why));
                    return None;
                }
            }
        }
        // One pane per session: the relaunch happens only while the entry
        // is still this launch's. If the supervisor rebuilt the session in
        // another window while this wrapper waited for room, the launch id
        // has moved on and the relaunch is refused rather than making a
        // second Claude Code run against one conversation.
        let Some((launch_id, entry)) = entry::reactivate(dir, name, exit.launch_id) else {
            loudly(dir, &refused_line(registry::now(), name, "superseded"));
            return None;
        };
        loudly(dir, &relaunch_line(registry::now(), name, &reason, room));
        Some(Relaunch {
            launch_id,
            entry,
            // Nothing is behind this relaunch but the rule that every exit
            // is given one, so it is the trial: should the run it starts
            // come back with nothing to show as well, the brake stops the
            // chain rather than giving it another start.
            on_trial: exit.progress != Progress::Advanced && !room,
        })
    }
}

/// A `gone` refusal: one line in the pane and the log and, when `why` says
/// a wait for room ended with no room and no signal, the cause on the
/// session's record, which is left `active` for the supervisor's recovery.
fn gone(dir: &AgentsDir, name: &str, exit: &Exit<'_>, reason: &str, why: Option<Lost>) {
    loudly(dir, &refused_line(registry::now(), name, "gone"));
    if let Some(why) = why {
        entry::note_refusal(dir, name, exit.launch_id, &gone_alert(reason, why));
    }
}
