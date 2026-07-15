//! Per-session Signal channel: a crash-safe file-backed [`Inbox`] for accepted
//! operator instructions, the [`format_injection_context`] helper that renders
//! drained entries into a labeled-untrusted, length-capped block for the hook
//! `additionalContext` seam, and the gated [`SignalSession`] that owns the
//! session's group over the async transport.

use amplihack_state::AtomicJsonFile;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Maximum bytes of injected context emitted per drain. Bounds prompt-injection
/// surface; longer drains are truncated with a marker.
pub const MAX_INJECTION_BYTES: usize = 4096;

/// One accepted, gate-approved operator instruction awaiting injection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxEntry {
    /// Monotonic per-session id (assigned on append).
    pub id: u64,
    /// Sender E.164 (already allowlist-checked).
    pub source: String,
    /// The instruction body.
    pub body: String,
}

/// Inbox persistence error.
#[derive(Debug, thiserror::Error)]
pub enum InboxError {
    /// Underlying atomic-file failure.
    #[error("inbox storage error: {0}")]
    Storage(String),
}

/// A crash-safe, append/drain file inbox backed by [`AtomicJsonFile`].
///
/// Appends assign a monotonically increasing id. `drain` returns all pending
/// entries in id order and atomically clears them; a concurrent append never
/// loses or reorders entries.
pub struct Inbox {
    #[allow(dead_code)] // consumed by the P4 implementation of the stubbed methods
    file: AtomicJsonFile,
}

impl Inbox {
    /// Open (or lazily create) an inbox at `path`.
    pub fn new(_path: impl Into<PathBuf>) -> Self {
        todo!("open atomic-json-backed inbox (P4)")
    }

    /// Append a gate-accepted instruction, returning the stored entry.
    pub fn append(&self, _source: &str, _body: &str) -> Result<InboxEntry, InboxError> {
        todo!("append with monotonic id (P4)")
    }

    /// Return pending entries in id order WITHOUT removing them.
    pub fn peek(&self) -> Result<Vec<InboxEntry>, InboxError> {
        todo!("peek pending (P4)")
    }

    /// Return all pending entries in id order and atomically clear them.
    pub fn drain(&self) -> Result<Vec<InboxEntry>, InboxError> {
        todo!("drain + atomic clear (P4)")
    }
}

/// Render drained entries into an `additionalContext` block.
///
/// Returns `None` for an empty slice (so the hook emits byte-identical output
/// when there is nothing to inject). Output is explicitly labeled as untrusted
/// operator input and truncated to [`MAX_INJECTION_BYTES`].
pub fn format_injection_context(_entries: &[InboxEntry]) -> Option<String> {
    todo!("format labeled-untrusted, capped context (P4/P6)")
}

/// Async per-session Signal channel over the signal-cli JSON-RPC transport.
#[cfg(feature = "signal")]
pub use gated::SignalSession;

#[cfg(feature = "signal")]
mod gated {
    use crate::config::SignalConfig;

    /// Owns one per-session Signal group over a live JSON-RPC TCP connection.
    pub struct SignalSession {
        #[allow(dead_code)]
        config: SignalConfig,
        #[allow(dead_code)]
        group_id: Option<String>,
    }

    impl SignalSession {
        /// Connect to the configured signal-cli daemon.
        pub async fn connect(_config: SignalConfig) -> std::io::Result<Self> {
            todo!("connect TCP JSON-RPC (P4)")
        }

        /// Create (per-session) or resolve (rolling) this session's group and
        /// return its group id. Posts nothing.
        pub async fn announce(&mut self, _name: &str) -> std::io::Result<String> {
            todo!("create/resolve group (P4)")
        }

        /// Post an update to the session group.
        pub async fn post(&self, _update: &str) -> std::io::Result<()> {
            todo!("send to group (P4)")
        }

        /// Leave the session group (per-session mode only).
        pub async fn quit(&self) -> std::io::Result<()> {
            todo!("quit group (P4)")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: u64, body: &str) -> InboxEntry {
        InboxEntry {
            id,
            source: "+15551239999".into(),
            body: body.into(),
        }
    }

    #[test]
    fn append_then_drain_roundtrips_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = Inbox::new(dir.path().join("inbox.json"));
        inbox.append("+15551239999", "first").unwrap();
        inbox.append("+15551239999", "second").unwrap();
        let drained = inbox.drain().unwrap();
        let bodies: Vec<&str> = drained.iter().map(|e| e.body.as_str()).collect();
        assert_eq!(bodies, vec!["first", "second"]);
    }

    #[test]
    fn append_assigns_monotonic_ids() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = Inbox::new(dir.path().join("inbox.json"));
        let a = inbox.append("+15551239999", "a").unwrap();
        let b = inbox.append("+15551239999", "b").unwrap();
        assert!(b.id > a.id, "ids must be monotonically increasing");
    }

    #[test]
    fn drain_clears_pending() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = Inbox::new(dir.path().join("inbox.json"));
        inbox.append("+15551239999", "only").unwrap();
        let first = inbox.drain().unwrap();
        assert_eq!(first.len(), 1);
        let second = inbox.drain().unwrap();
        assert!(second.is_empty(), "second drain must be empty");
    }

    #[test]
    fn peek_does_not_remove() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = Inbox::new(dir.path().join("inbox.json"));
        inbox.append("+15551239999", "keep").unwrap();
        assert_eq!(inbox.peek().unwrap().len(), 1);
        assert_eq!(
            inbox.peek().unwrap().len(),
            1,
            "peek must be non-destructive"
        );
    }

    #[test]
    fn drain_on_missing_inbox_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = Inbox::new(dir.path().join("nonexistent.json"));
        assert!(inbox.drain().unwrap().is_empty());
    }

    #[test]
    fn format_empty_entries_returns_none() {
        assert_eq!(format_injection_context(&[]), None);
    }

    #[test]
    fn format_labels_content_untrusted() {
        let ctx = format_injection_context(&[entry(1, "restart the build")]).expect("some context");
        assert!(
            ctx.to_lowercase().contains("untrusted"),
            "injected context must flag untrusted operator input, got: {ctx}"
        );
        assert!(ctx.contains("restart the build"));
    }

    #[test]
    fn format_caps_length() {
        let huge = "A".repeat(MAX_INJECTION_BYTES * 4);
        let ctx = format_injection_context(&[entry(1, &huge)]).expect("some context");
        assert!(
            ctx.len() <= MAX_INJECTION_BYTES + 256,
            "context must be capped near MAX_INJECTION_BYTES, was {}",
            ctx.len()
        );
    }
}
