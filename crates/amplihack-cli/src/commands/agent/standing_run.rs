//! The standing status of one session, the room gate of its relaunch and
//! the decision the wrapper acts on (issue #2912).
//!
//! The rule the decision applies is in the parent module: [`after_exit`]
//! for whose stop an exit was, and [`worth_relaunching`] for whether a
//! relaunch answers anything new. What is here is the reading of the world
//! that rule needs, which is the session's record, the transcript of its
//! conversation and the rotator's swap record.

use super::lines::{
    exit_reason, loudly, no_progress_alert, refused_line, relaunch_line, waiting_line,
};
use super::relaunch::{self, Room};
use super::{AfterExit, Exit, Progress, Relaunch, after_exit, worth_relaunching};
use crate::commands::agent::registry;
use crate::commands::agent::store::AgentsDir;
use crate::commands::agent::{entry, sessions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

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

    /// How many bytes of its own conversation this session has on disk: the
    /// transcript of the conversation its record names, in whichever
    /// configuration directory holds it. `Some(0)` when the record names a
    /// conversation that has no transcript yet, which is what a
    /// conversation with no turn written to it looks like.
    ///
    /// `None` is the absence of a measurement rather than a measurement of
    /// nothing: the session is not standing, its record names no
    /// conversation, or the configuration directories cannot be listed.
    /// [`Progress::of`] keeps that apart from `Some(0)` and
    /// [`no_progress_alert`] says which of the two a refusal saw, because
    /// "the conversation did not grow" and "there is no conversation of
    /// this session's to look at" are different facts about a run. Neither
    /// is evidence that the run did something, so neither relaunches a run
    /// that is already on trial.
    ///
    /// The wrapper takes this before every launch and again after the exit.
    /// Growth across a run is the [`Progress::Advanced`] of
    /// [`worth_relaunching`]: the evidence that the run did something a
    /// relaunch could continue. It is a measurement of work and not a count
    /// of attempts, so it never caps a session that is running: a session
    /// that is working grows it every turn.
    ///
    /// It is read from the conversation the session's own record names, not
    /// from the launch directory as a whole, so a second session running in
    /// the same directory can neither pass this session off as working nor
    /// keep its relaunch loop alive.
    pub(in crate::commands::agent) fn conversation_bytes(
        &self,
        dir: &AgentsDir,
        home: &Path,
        launch_dir: &Path,
        name: &str,
    ) -> Option<u64> {
        if !self.on {
            return None;
        }
        let id = entry::conversation(dir, name)?;
        let dirs = sessions::config_dirs_from(home, |k| std::env::var_os(k)).ok()?;
        Some(
            dirs.iter()
                .filter_map(|d| sessions::transcript_len(d, launch_dir, &id))
                .max()
                .unwrap_or(0),
        )
    }

    /// The next launch of a standing session, or `None` to stop.
    ///
    /// A [`Relaunch`] means start Claude Code again under its launch id,
    /// with the arguments rebuilt from its entry. Every relaunch, wait and
    /// refusal is logged; an owner-ended exit, a signalled wrapper and a
    /// session that is not standing are not, since each of those is a stop
    /// someone chose. A `no-progress` refusal is left on the session's
    /// record as well, because it stops a session its owner still wants
    /// running. `stopped` is the wrapper's flag for SIGHUP and SIGTERM,
    /// read again during and after a wait for room.
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
        let room = waited == Room::Waited;
        if !worth_relaunching(exit.on_trial, exit.progress, room) {
            loudly(dir, &refused_line(registry::now(), name, "no-progress"));
            entry::note_refusal(
                dir,
                name,
                exit.launch_id,
                &no_progress_alert(&reason, exit.progress),
            );
            return None;
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
