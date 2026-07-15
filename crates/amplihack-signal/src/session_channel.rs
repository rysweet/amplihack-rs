//! Per-session channel: [`SignalSession`] owns one group + a file-backed inbox.
//!
//! The [`Inbox`] is the cross-process seam: the detached `signal-subscriber`
//! process **pushes** accepted instructions into it, while the hook process
//! **drains** it on `PostToolUse` / `UserPromptSubmit`. It is backed by an
//! [`amplihack_state::atomic_json::AtomicJsonFile`] (crash-safe, lock-guarded)
//! at a path derived through [`amplihack_types::paths::sanitize_session_id`].
//!
//! The inbox is **bounded**: it holds at most [`Inbox::DEFAULT_CAPACITY`]
//! pending instructions. When full, the **oldest** is evicted to admit the
//! newest (backpressure by bounded queue).

use amplihack_state::atomic_json::AtomicJsonFile;
use amplihack_types::paths::sanitize_session_id;
use std::path::{Path, PathBuf};

/// Errors from inbox operations.
#[derive(Debug, thiserror::Error)]
pub enum InboxError {
    /// An underlying atomic-json / filesystem error.
    #[error("inbox storage error: {0}")]
    Storage(String),
}

/// Outcome of an [`Inbox::push`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    /// The instruction was queued with room to spare.
    Queued,
    /// The queue was full; the oldest instruction was evicted to make room.
    EvictedOldest,
}

/// A bounded, file-backed queue of pending operator instructions.
pub struct Inbox {
    #[allow(dead_code)]
    path: PathBuf,
    #[allow(dead_code)]
    capacity: usize,
}

impl Inbox {
    /// Default bounded capacity (flood resistance).
    pub const DEFAULT_CAPACITY: usize = 32;

    /// Create an inbox at an explicit file `path` with an explicit `capacity`.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>, capacity: usize) -> Self {
        Self {
            path: path.into(),
            capacity,
        }
    }

    /// Derive the per-session inbox path under `root` from `session_id`,
    /// sanitizing the id to prevent path traversal, with the default capacity.
    #[must_use]
    pub fn at_session(session_id: &str, root: &Path) -> Self {
        let sanitized = sanitize_session_id(session_id);
        let path = root.join(sanitized).join("inbox.json");
        Self::new(path, Self::DEFAULT_CAPACITY)
    }

    /// The resolved inbox file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Push one instruction. Bounded: evicts the oldest when at capacity.
    pub fn push(&self, instruction: &str) -> Result<PushOutcome, InboxError> {
        let file = AtomicJsonFile::new(&self.path);
        let capacity = self.capacity;
        let instruction = instruction.to_string();
        let mut evicted = false;
        file.update(|queue: &mut Vec<String>| {
            queue.push(instruction);
            while queue.len() > capacity {
                queue.remove(0);
                evicted = true;
            }
        })
        .map_err(|e| InboxError::Storage(e.to_string()))?;
        Ok(if evicted {
            PushOutcome::EvictedOldest
        } else {
            PushOutcome::Queued
        })
    }

    /// Read **and clear** all queued instructions (one-shot delivery).
    pub fn drain(&self) -> Result<Vec<String>, InboxError> {
        let file = AtomicJsonFile::new(&self.path);
        let mut taken = Vec::new();
        file.update(|queue: &mut Vec<String>| {
            taken = std::mem::take(queue);
        })
        .map_err(|e| InboxError::Storage(e.to_string()))?;
        Ok(taken)
    }

    /// Number of currently-queued instructions.
    pub fn len(&self) -> Result<usize, InboxError> {
        let file = AtomicJsonFile::new(&self.path);
        let queue: Vec<String> = file
            .read()
            .map_err(|e| InboxError::Storage(e.to_string()))?
            .unwrap_or_default();
        Ok(queue.len())
    }

    /// Whether the inbox is currently empty.
    pub fn is_empty(&self) -> Result<bool, InboxError> {
        Ok(self.len()? == 0)
    }
}

/// Owns one per-session Signal group plus its file-backed [`Inbox`].
///
/// This is the in-process owner used by a single process that both connects to
/// the daemon and manages the session's inbox. The cross-process hook wiring
/// composes the lower-level [`crate::transport`], [`crate::gating`], and
/// [`Inbox`] pieces directly instead.
pub struct SignalSession {
    transport: crate::transport::SignalTransport,
    gate: crate::gating::Gate,
    group_id: crate::transport::GroupId,
    inbox: Inbox,
}

impl SignalSession {
    /// Bind an already-connected transport to an existing group and inbox.
    #[must_use]
    pub fn new(
        transport: crate::transport::SignalTransport,
        cfg: &crate::config::SignalConfig,
        group_id: crate::transport::GroupId,
        inbox: Inbox,
    ) -> Self {
        let gate = crate::gating::Gate::new(cfg, group_id.as_str());
        Self {
            transport,
            gate,
            group_id,
            inbox,
        }
    }

    /// The group this session owns.
    #[must_use]
    pub fn group_id(&self) -> &crate::transport::GroupId {
        &self.group_id
    }

    /// Post the "session started" message into the group.
    pub async fn announce(&mut self) -> std::io::Result<()> {
        self.post("session started").await
    }

    /// Post an outbound update at a meaningful transition and record it in the
    /// echo-suppression window so the synced-back copy is not re-ingested.
    pub async fn post(&mut self, update: &str) -> std::io::Result<()> {
        self.transport.send_group(&self.group_id, update).await?;
        self.gate.record_outbound(update);
        Ok(())
    }

    /// Read the next inbound envelope, gate it, and (if accepted) append the
    /// operator instruction to the file inbox. Returns the accepted instruction.
    pub async fn pump_once(&mut self) -> std::io::Result<Option<String>> {
        let Some(env) = self.transport.receive().await? else {
            return Ok(None);
        };
        if let Some(instruction) = self.gate.evaluate(&env) {
            self.inbox
                .push(&instruction)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            return Ok(Some(instruction));
        }
        Ok(Some(String::new()))
    }

    /// Drain queued inbound instructions from the file inbox.
    pub fn drain(&self) -> Result<Vec<String>, InboxError> {
        self.inbox.drain()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn push_then_drain_returns_in_fifo_order() {
        let dir = TempDir::new().unwrap();
        let inbox = Inbox::new(dir.path().join("inbox.json"), 8);
        assert_eq!(inbox.push("first").unwrap(), PushOutcome::Queued);
        assert_eq!(inbox.push("second").unwrap(), PushOutcome::Queued);
        assert_eq!(inbox.drain().unwrap(), vec!["first", "second"]);
    }

    #[test]
    fn drain_is_one_shot() {
        let dir = TempDir::new().unwrap();
        let inbox = Inbox::new(dir.path().join("inbox.json"), 8);
        inbox.push("only").unwrap();
        assert_eq!(inbox.drain().unwrap(), vec!["only"]);
        assert!(inbox.drain().unwrap().is_empty(), "second drain is empty");
    }

    #[test]
    fn drain_on_missing_file_is_empty() {
        let dir = TempDir::new().unwrap();
        let inbox = Inbox::new(dir.path().join("does-not-exist.json"), 8);
        assert!(inbox.drain().unwrap().is_empty());
    }

    #[test]
    fn bounded_capacity_evicts_oldest() {
        let dir = TempDir::new().unwrap();
        let inbox = Inbox::new(dir.path().join("inbox.json"), 2);
        assert_eq!(inbox.push("a").unwrap(), PushOutcome::Queued);
        assert_eq!(inbox.push("b").unwrap(), PushOutcome::Queued);
        // Third push overflows: oldest ("a") is evicted.
        assert_eq!(inbox.push("c").unwrap(), PushOutcome::EvictedOldest);
        assert_eq!(inbox.drain().unwrap(), vec!["b", "c"]);
    }

    #[test]
    fn len_tracks_pending_count() {
        let dir = TempDir::new().unwrap();
        let inbox = Inbox::new(dir.path().join("inbox.json"), 8);
        assert_eq!(inbox.len().unwrap(), 0);
        inbox.push("x").unwrap();
        assert_eq!(inbox.len().unwrap(), 1);
        inbox.drain().unwrap();
        assert_eq!(inbox.len().unwrap(), 0);
    }

    #[test]
    fn at_session_sanitizes_traversal_ids() {
        let dir = TempDir::new().unwrap();
        let inbox = Inbox::at_session("../../etc/passwd", dir.path());
        // The resolved path must stay under root — no traversal escape.
        assert!(
            inbox.path().starts_with(dir.path()),
            "inbox path {:?} must stay under root {:?}",
            inbox.path(),
            dir.path()
        );
    }
}
