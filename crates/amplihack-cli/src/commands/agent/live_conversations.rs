//! The agent registry as a source for `sessions archive` (issue #1746):
//! which conversations a registered agent may still be resumed into.

use super::{registry, store};
use std::io;
use std::path::{Path, PathBuf};

pub(crate) use store::Refusal;

/// Why the registry could not be read for `sessions archive`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Unreadable {
    /// An I/O error on this path.
    Io(PathBuf, io::ErrorKind),
    /// The supervisor's checks refused a file or directory; the refusal
    /// names it and says what is wrong.
    Refused(Refusal),
}

/// Conversation ids of registered agents that are not `ended`: `active`
/// ones the supervisor resumes when their session is lost, and `stopped`
/// ones a user can bring back with `agent register` or `claude --supervise`.
/// `sessions archive` treats these as live, so it never removes a
/// transcript the supervisor still needs (issue #1746, after #1760).
///
/// Reads without the lock and creates nothing. The first question is
/// whether `~/.amplihack/agents/registry.json` exists at all; when it does
/// not, the list is empty and none of the supervisor's directory rules are
/// applied, so a user who has never registered an agent can keep
/// `~/.amplihack` wherever they like, a symlink to another disk included.
/// Only when the file is present do the supervisor's checks run (the same
/// ones `amplihack agent status` applies), and any failure is an error
/// that names the path and the reason, so the caller fails closed.
///
/// The error is this module's own [`Unreadable`]; `sessions::live` turns it
/// into its own type, so neither module depends on the other's errors.
pub(crate) fn unended_conversations(home: &Path) -> Result<Vec<String>, Unreadable> {
    let root = store::AgentsDir::path(home);
    let file = root.join("registry.json");
    // Intermediate components are followed, the file itself is not, so a
    // symlinked `~/.amplihack` is looked through, while a dangling
    // `registry.json` symlink still counts as present and is refused below.
    match std::fs::symlink_metadata(&file) {
        Ok(_) => {}
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(Vec::new());
        }
        Err(e) => return Err(Unreadable::Io(file, e.kind())),
    }
    let dir = match store::AgentsDir::existing(home) {
        Ok(Some(dir)) => dir,
        // Removed since the check above.
        Ok(None) => return Ok(Vec::new()),
        Err(r) => return Err(Unreadable::Refused(r)),
    };
    let reg = store::load(&dir).map_err(|e| match e {
        store::LoadError::Io(io) => Unreadable::Io(file.clone(), io.kind()),
        store::LoadError::Unsafe(r) | store::LoadError::Corrupt(r) => Unreadable::Refused(r),
        too_new @ store::LoadError::TooNew(_) => Unreadable::Refused(store::Refusal {
            path: file.clone(),
            problem: too_new.to_string(),
        }),
    })?;
    Ok(reg
        .agents
        .into_values()
        .filter(|e| e.state != registry::State::Ended)
        .filter_map(|e| e.conversation_id)
        .collect())
}
