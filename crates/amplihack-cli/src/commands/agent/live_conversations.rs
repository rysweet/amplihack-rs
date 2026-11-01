//! The agent registry as a source for `sessions archive` (issue #1746):
//! which conversations a registered agent may still be resumed into.

use super::{registry, store};
use crate::commands::sessions::live::{LiveError, Why};
use std::io;
use std::path::{Path, PathBuf};

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
pub(crate) fn unended_conversations(home: &Path) -> Result<Vec<String>, LiveError> {
    let top = home.join(".amplihack");
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
        Err(e) => return Err((file, e.kind().into())),
    }
    let candidates = [file.as_path(), root.as_path(), top.as_path()];
    let dir = match store::AgentsDir::existing(home) {
        Ok(Some(dir)) => dir,
        // Removed since the check above.
        Ok(None) => return Ok(Vec::new()),
        Err(e) => return Err(reason(&e.to_string(), &candidates, &root)),
    };
    let reg = store::load(&dir).map_err(|e| match e {
        store::LoadError::Io(io) => (file.clone(), io.kind().into()),
        other => reason(&other.to_string(), &candidates, &file),
    })?;
    Ok(reg
        .agents
        .into_values()
        .filter(|e| e.state != registry::State::Ended)
        .filter_map(|e| e.conversation_id)
        .collect())
}

/// The registry's message as a path and a reason. The store's messages
/// start with `<path>: `; that path is used and the prefix is dropped so it
/// is not printed twice. A message without one is shown whole against
/// `fallback`.
fn reason(msg: &str, candidates: &[&Path], fallback: &Path) -> LiveError {
    for p in candidates {
        let prefix = format!("{}: ", p.display());
        if let Some(rest) = msg.strip_prefix(&prefix) {
            return (p.to_path_buf(), Why::Agents(rest.to_string()));
        }
    }
    (PathBuf::from(fallback), Why::Agents(msg.to_string()))
}
