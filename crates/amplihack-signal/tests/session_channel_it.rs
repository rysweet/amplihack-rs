//! Integration tests for the file-backed inbox across the process boundary.
//!
//! The subscriber process and the hook process share the inbox purely through
//! the filesystem, so these tests exercise two independent [`Inbox`] handles to
//! the **same** path (mirroring "writer process" vs "reader process").
#![cfg(feature = "signal")]

use amplihack_signal::session_channel::{Inbox, PushOutcome};
use tempfile::TempDir;

#[test]
fn two_handles_same_path_writer_reader() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("inbox.json");

    // "subscriber process" writes.
    let writer = Inbox::new(&path, 16);
    writer.push("investigate the flaky test").unwrap();
    writer.push("prefer the smaller refactor").unwrap();

    // "hook process" drains.
    let reader = Inbox::new(&path, 16);
    let drained = reader.drain().unwrap();
    assert_eq!(
        drained,
        vec!["investigate the flaky test", "prefer the smaller refactor"]
    );

    // Drain cleared the shared file for the writer's next view too.
    assert!(writer.drain().unwrap().is_empty());
}

#[test]
fn bounded_inbox_survives_a_flood() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("inbox.json");
    let inbox = Inbox::new(&path, 4);

    let mut evictions = 0usize;
    for i in 0..100 {
        if inbox.push(&format!("msg-{i}")).unwrap() == PushOutcome::EvictedOldest {
            evictions += 1;
        }
    }
    // Never grows beyond capacity, and the most-recent entries survive.
    let remaining = inbox.drain().unwrap();
    assert_eq!(remaining.len(), 4);
    assert_eq!(
        remaining,
        vec!["msg-96", "msg-97", "msg-98", "msg-99"],
        "bounded queue keeps the newest entries"
    );
    assert!(evictions > 0, "a flood must have triggered evictions");
}

#[test]
fn at_session_derives_stable_sanitized_path() {
    let dir = TempDir::new().unwrap();
    let a = Inbox::at_session("session-123", dir.path());
    let b = Inbox::at_session("session-123", dir.path());
    assert_eq!(a.path(), b.path(), "same id → same path (stable)");
    assert!(a.path().starts_with(dir.path()));

    let c = Inbox::at_session("session-456", dir.path());
    assert_ne!(a.path(), c.path(), "different ids → different paths");
}
