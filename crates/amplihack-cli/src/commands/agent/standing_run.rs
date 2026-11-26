//! The standing status of one session, the room gate of its relaunch and
//! the decision the wrapper acts on (issue #2912).
//!
//! The rule the decision applies is [`after_exit`] in the parent module:
//! whose stop an exit was. What is here is the reading of the world it
//! needs, which is the session's record and the rotator's swap record, and
//! the evidence the relaunch leaves on the record: whether the model
//! replied during the run, read from the transcript of its conversation.

use super::lines::{
    exit_reason, gone_alert, launch_failed_alert, loudly, refused_line, relaunch_line,
    relaunch_note, waiting_line,
};
use super::progress::replied_since;
use super::relaunch::{self, Lost, Room};
use super::{AfterExit, Exit, Quiet, Relaunch, after_exit};
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

    /// Whether the model wrote a reply at or after `started` in the
    /// transcript of the conversation this session's record names, in
    /// whichever configuration directory holds it.
    ///
    /// The wrapper reads this once, after the exit, with `started` taken
    /// just before the launch, and puts it on the record of the relaunch
    /// (see [`Quiet`]). Nothing is decided on it: the session is relaunched
    /// either way. The size of the transcript is not read, because a start
    /// that does nothing and a request that fails both grow it (see
    /// `standing_progress.rs`).
    ///
    /// `false` as well when the session is not standing, its record names
    /// no conversation, or the configuration directories cannot be listed.
    /// The record then shows no conversation, and `amplihack agent status`
    /// prints `-` for it, so a reader of the record can tell "no reply" from
    /// "nothing to read".
    ///
    /// It is read from the conversation the session's own record names, not
    /// from the launch directory as a whole, so a second session running in
    /// the same directory cannot pass this one off as working.
    pub(in crate::commands::agent) fn replied(
        &self,
        dir: &AgentsDir,
        home: &Path,
        launch_dir: &Path,
        name: &str,
        started: SystemTime,
    ) -> bool {
        if !self.on {
            return false;
        }
        let Some(id) = entry::conversation(dir, name) else {
            return false;
        };
        let Ok(dirs) = sessions::config_dirs_from(home, |k| std::env::var_os(k)) else {
            return false;
        };
        dirs.iter()
            .filter_map(|d| sessions::transcript_path(d, launch_dir, &id))
            .filter_map(|p| File::open(p).ok())
            .any(|f| replied_since(BufReader::new(f), started))
    }

    /// The next launch of a standing session, or `None` to stop.
    ///
    /// A [`Relaunch`] means start Claude Code again under its launch id,
    /// with the arguments rebuilt from its entry. Every process exit nobody
    /// chose is relaunched once a login has room, and the relaunch is
    /// written to the pane, the supervisor log and the session's record.
    /// An owner-ended exit, a signalled wrapper and a session that is not
    /// standing are not logged, since each of those is a stop someone
    /// chose.
    ///
    /// The refusals are a launch that failed without an exit status from
    /// Claude Code (`launch-failed`), a wait for room that ended without a
    /// login with room (`gone`), and a record that has moved on
    /// (`superseded`). The first two leave their reason on the session's
    /// record as well, because each stops a session its owner still wants
    /// running. `stopped` is the wrapper's flag for SIGHUP and SIGTERM, read
    /// again during and after a wait for room.
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
        match after_exit(self.on, exit.signalled, marker, exit.result) {
            AfterExit::OwnerEnded | AfterExit::GoingAway => return None,
            AfterExit::LaunchFailed => {
                loudly(dir, &refused_line(registry::now(), name, "launch-failed"));
                entry::note_refusal(dir, name, exit.launch_id, &launch_failed_alert());
                return None;
            }
            AfterExit::Relaunch => {}
        }
        let reason = exit_reason(exit.result);
        // An exit with no room is the usage limit this feature exists for:
        // the wait is what makes the next launch different from the one
        // that failed.
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
        // wants the session running, so the cause goes on its record and
        // the record is left active for the supervisor's recovery. That
        // starts the session in a new wrapper, which waits for room again
        // should the run exit with none.
        if let Room::Gone(why) = waited {
            gone(dir, name, exit, &reason, Some(why));
            return None;
        }
        let room = waited == Room::Waited;
        let now = registry::now();
        let quiet = Quiet::after(exit.quiet, exit.replied, now);
        let note = relaunch_note(now, &reason, room, quiet);
        // One pane per session: the relaunch happens only while the entry
        // is still this launch's. If the supervisor rebuilt the session in
        // another window while this wrapper waited for room, or ended,
        // stopped or unregistered it, the launch id has moved on and the
        // relaunch is refused rather than making a second Claude Code run
        // against one conversation.
        let Some((launch_id, entry)) = entry::reactivate(dir, name, exit.launch_id, &note) else {
            loudly(dir, &refused_line(now, name, "superseded"));
            return None;
        };
        loudly(dir, &relaunch_line(now, name, &reason, room));
        Some(Relaunch {
            launch_id,
            entry,
            quiet,
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
