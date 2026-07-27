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

// -----------------------------------------------------------------------------
// INV-4 — Bounded turn queue, capacity from OPERATOR CONFIG, evict-oldest.
//
// The pre-existing tests bound the inbox via an explicit `Inbox::new(_, N)`.
// This characterization locks the refactor-critical binding that the
// operator-configurable capacity flows from `AMPLIHACK_SIGNAL_INBOX_CAPACITY`
// through `Inbox::default_capacity()` into `Inbox::at_session`, and that a flood
// evicts the OLDEST pending prompt (FIFO backpressure). A later shared-Session
// extraction MUST preserve this operator control and eviction order.
//
// Env mutation is process-global, so a save/restore guard serializes it against
// any sibling test that reads the same variable.
// -----------------------------------------------------------------------------

use std::sync::Mutex as StdMutex;

// Serializes AMPLIHACK_SIGNAL_INBOX_CAPACITY mutation across parallel tests.
static ENV_GUARD: StdMutex<()> = StdMutex::new(());

#[test]
fn characterization_inv4_capacity_from_operator_config_evicts_oldest() {
    const KEY: &str = "AMPLIHACK_SIGNAL_INBOX_CAPACITY";
    // A small, unlikely-to-collide operator-chosen capacity.
    const CAP: usize = 3;

    let _guard = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    let previous = std::env::var(KEY).ok();

    // SAFETY: mutation is serialized by ENV_GUARD and restored below.
    unsafe {
        std::env::set_var(KEY, CAP.to_string());
    }

    // Confirm the operator config is honored by the default-capacity path.
    assert_eq!(
        Inbox::default_capacity(),
        CAP,
        "operator-set AMPLIHACK_SIGNAL_INBOX_CAPACITY must drive default_capacity()"
    );

    let dir = TempDir::new().unwrap();
    // Construct through the OPERATOR-CONFIG path (at_session → default_capacity),
    // not an explicit new(_, N).
    let inbox = Inbox::at_session("inv4-session", dir.path());

    // Push exactly capacity prompts: all queued, no eviction.
    for i in 0..CAP {
        assert_eq!(
            inbox.push(&format!("prompt-{i}")).unwrap(),
            PushOutcome::Queued,
            "prompt within capacity must queue without eviction"
        );
    }
    // One more overflows: the OLDEST is evicted (backpressure).
    assert_eq!(
        inbox.push(&format!("prompt-{CAP}")).unwrap(),
        PushOutcome::EvictedOldest,
        "exceeding operator-configured capacity must evict the oldest prompt"
    );

    // Restore the environment before asserting (so a panic can't leak state).
    // SAFETY: still under ENV_GUARD.
    unsafe {
        match &previous {
            Some(v) => std::env::set_var(KEY, v),
            None => std::env::remove_var(KEY),
        }
    }

    // The surviving queue is the newest CAP prompts, oldest-first (FIFO): the
    // very first prompt ("prompt-0") was the one evicted.
    let remaining = inbox.drain().unwrap();
    assert_eq!(
        remaining,
        vec![
            "prompt-1".to_string(),
            "prompt-2".to_string(),
            "prompt-3".to_string()
        ],
        "bounded queue keeps the newest {CAP} prompts in FIFO order; the oldest is evicted"
    );
}
