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

/// Save/restore guard for a single process-global env var so this test never
/// leaks `AMPLIHACK_SIGNAL_INBOX_CAPACITY` state to its neighbours.
struct EnvGuard {
    key: &'static str,
    prev: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: single-threaded within this test; restored on drop.
        unsafe { std::env::set_var(key, value) };
        Self { key, prev }
    }

    fn unset(key: &'static str) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: single-threaded within this test; restored on drop.
        unsafe { std::env::remove_var(key) };
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.prev {
            // SAFETY: restoring the pre-test value on the same thread.
            Some(v) => unsafe { std::env::set_var(self.key, v) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

const CAPACITY_ENV: &str = "AMPLIHACK_SIGNAL_INBOX_CAPACITY";

// INV-4 (bounded turn queue evicts oldest at operator-configurable capacity).
// Complements the pre-existing fixed-capacity `bounded_capacity_evicts_oldest`
// by exercising the OPERATOR-CONFIG path: capacity is read from
// `AMPLIHACK_SIGNAL_INBOX_CAPACITY` via `Inbox::default_capacity()` /
// `Inbox::at_session`. A valid value is honoured and overflow evicts the oldest
// (`PushOutcome::EvictedOldest`, bounding on-disk memory); invalid / zero /
// negative / whitespace / non-numeric values fall back to
// `Inbox::DEFAULT_CAPACITY` (32) rather than disabling the inbox, panicking, or
// creating an unbounded file. Env mutation is save/restore-guarded.
#[test]
fn characterization_inv4_capacity_from_operator_config_evicts_oldest() {
    // --- Valid operator value is honoured, and overflow evicts the oldest. ---
    {
        let _guard = EnvGuard::set(CAPACITY_ENV, "3");
        assert_eq!(
            Inbox::default_capacity(),
            3,
            "a valid capacity env value must be honoured"
        );

        let dir = TempDir::new().unwrap();
        // `at_session` derives capacity from the env via `default_capacity()`.
        let inbox = Inbox::at_session("operator-cfg", dir.path());
        assert_eq!(inbox.push("a").unwrap(), PushOutcome::Queued);
        assert_eq!(inbox.push("b").unwrap(), PushOutcome::Queued);
        assert_eq!(inbox.push("c").unwrap(), PushOutcome::Queued);
        // The 4th push overflows the operator-configured cap of 3.
        assert_eq!(
            inbox.push("d").unwrap(),
            PushOutcome::EvictedOldest,
            "overflow past the configured capacity evicts the oldest"
        );
        // Bounded: the on-disk queue holds at most `capacity` newest entries.
        assert_eq!(inbox.len().unwrap(), 3, "queue stays bounded at capacity");
        assert_eq!(
            inbox.drain().unwrap(),
            vec!["b", "c", "d"],
            "the oldest ('a') was evicted; the newest survive"
        );
    }

    // --- Invalid / zero / negative / whitespace / non-numeric → default. ---
    for bad in ["0", "-1", "   ", "not-a-number", "3.5", ""] {
        let _guard = EnvGuard::set(CAPACITY_ENV, bad);
        assert_eq!(
            Inbox::default_capacity(),
            Inbox::DEFAULT_CAPACITY,
            "invalid env value {bad:?} must fall back to DEFAULT_CAPACITY, never unbounded/disabled"
        );
    }

    // --- Absent env var → default capacity. ---
    {
        let _guard = EnvGuard::unset(CAPACITY_ENV);
        assert_eq!(
            Inbox::default_capacity(),
            Inbox::DEFAULT_CAPACITY,
            "an absent env value must fall back to DEFAULT_CAPACITY"
        );
    }
}
