// crates/amplihack-reflection/tests/reflection.rs
//
// TDD: failing tests for the top-level reflection orchestrator
// (port of amplifier-bundle/tools/amplihack/reflection/reflection.py)
// and supporting semaphore + lightweight analyzer.

use amplihack_reflection::lightweight_analyzer::{LightweightAnalyzer, Message, Role};
use amplihack_reflection::semaphore::ReflectionLock;
use tempfile::TempDir;

#[test]
fn semaphore_acquire_and_release() {
    let dir = TempDir::new().unwrap();
    let lock = ReflectionLock::new(dir.path()).unwrap();
    assert!(lock.acquire("sess", "analysis").unwrap());
    assert!(lock.is_locked());
    lock.release().unwrap();
    assert!(!lock.is_locked());
}

#[test]
fn semaphore_second_acquire_blocked_until_released() {
    let dir = TempDir::new().unwrap();
    let a = ReflectionLock::new(dir.path()).unwrap();
    let b = ReflectionLock::new(dir.path()).unwrap();
    assert!(a.acquire("s1", "analysis").unwrap());
    assert!(!b.acquire("s2", "analysis").unwrap());
    a.release().unwrap();
    assert!(b.acquire("s2", "analysis").unwrap());
}

#[test]
fn semaphore_stale_lock_can_be_reacquired() {
    let dir = TempDir::new().unwrap();
    let lock = ReflectionLock::with_stale_timeout(dir.path(), std::time::Duration::from_millis(0))
        .unwrap();
    assert!(lock.acquire("s1", "analysis").unwrap());
    // Drop reference but file exists; another lock with 0ms timeout sees it stale.
    let other = ReflectionLock::with_stale_timeout(dir.path(), std::time::Duration::from_millis(0))
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    assert!(other.acquire("s2", "analysis").unwrap());
}

#[test]
fn lightweight_analyzer_handles_empty_input() {
    let analyzer = LightweightAnalyzer::new();
    let result = analyzer.analyze_recent_responses(&[], &[]).unwrap();
    assert!(result.patterns.is_empty());
    assert!(result.summary.to_lowercase().contains("not enough"));
}

#[test]
fn lightweight_analyzer_returns_within_budget() {
    let analyzer = LightweightAnalyzer::new();
    let msgs = vec![
        Message {
            role: Role::User,
            content: "do thing".into(),
        },
        Message {
            role: Role::Assistant,
            content: "I tried but failed three times".into(),
        },
        Message {
            role: Role::Assistant,
            content: "I tried again with same result".into(),
        },
    ];
    let r = analyzer.analyze_recent_responses(&msgs, &[]).unwrap();
    assert!(r.elapsed_seconds < 5.0);
}
