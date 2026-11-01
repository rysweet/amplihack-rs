//! The agent registry as a source for `sessions archive` (issue #1746):
//! which conversations a registered agent may still be resumed into.

use super::{registry, store};
use std::path::PathBuf;

/// Conversation ids of registered agents that are not `ended`: `active`
/// ones the supervisor resumes when their session is lost, and `stopped`
/// ones a user can bring back with `agent register` or `claude --supervise`.
/// `sessions archive` treats these as live, so it never removes a
/// transcript the supervisor still needs (issue #1746, after #1760).
///
/// Reads without the lock and creates nothing. A missing directory or file
/// is an empty list; a registry that exists but cannot be read is an error
/// with the path, so the caller fails closed.
pub(crate) fn unended_conversations(
    home: &std::path::Path,
) -> Result<Vec<String>, (PathBuf, std::io::ErrorKind)> {
    // `existing` reports an unsafe or unreadable directory as a message
    // only; any such failure counts as unreadable.
    let dir = match store::AgentsDir::existing(home) {
        Ok(Some(dir)) => dir,
        Ok(None) => return Ok(Vec::new()),
        Err(_) => {
            return Err((
                store::AgentsDir::path(home),
                std::io::ErrorKind::InvalidData,
            ));
        }
    };
    let reg = store::load(&dir).map_err(|e| {
        let kind = match &e {
            store::LoadError::Io(io) => io.kind(),
            _ => std::io::ErrorKind::InvalidData,
        };
        (dir.registry(), kind)
    })?;
    Ok(reg
        .agents
        .into_values()
        .filter(|e| e.state != registry::State::Ended)
        .filter_map(|e| e.conversation_id)
        .collect())
}
