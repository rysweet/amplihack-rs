//! Threads open independent OS lock handles. The barrier establishes ordering;
//! there are no sleep-based assertions about which producer wins or its speed.
use crate::support::KEY;
use lbug_cache::cache::KeyLock;
use std::fs;
use std::sync::{
    Arc, Barrier,
    atomic::{AtomicUsize, Ordering},
};

#[test]
fn key_lock_serializes_independent_handles_for_the_same_key() {
    let dir = tempfile::tempdir().unwrap();
    let producers = 4;
    let ready = Arc::new(Barrier::new(producers));
    let builds = Arc::new(AtomicUsize::new(0));
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for _ in 0..producers {
            let ready = ready.clone();
            let builds = builds.clone();
            let root = dir.path();
            handles.push(scope.spawn(move || {
                ready.wait();
                let _lock = KeyLock::acquire(root, KEY).unwrap();
                // This tests the lock primitive. Full producer waiter rechecks
                // still need the native/concurrent producer acceptance run.
                let winner = root.join("selected");
                if !winner.exists() {
                    builds.fetch_add(1, Ordering::SeqCst);
                    fs::write(&winner, "verified winner").unwrap();
                }
                assert_eq!(fs::read_to_string(winner).unwrap(), "verified winner");
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
    });
    assert_eq!(builds.load(Ordering::SeqCst), 1);
}

#[test]
fn lock_files_stay_stable_after_release_and_different_keys_do_not_block() {
    let dir = tempfile::tempdir().unwrap();
    let first = KeyLock::acquire(dir.path(), KEY).unwrap();
    let path = first.path().to_owned();
    let (send, recv) = std::sync::mpsc::channel();
    let root = dir.path().to_owned();
    let waiter = std::thread::spawn(move || {
        let second =
            KeyLock::acquire(&root, "lbug-0.15.5-aarch64-apple-darwin-appleclang17").unwrap();
        send.send(()).unwrap();
        drop(second);
    });
    let independent = recv.recv_timeout(std::time::Duration::from_secs(10));
    drop(first);
    waiter.join().unwrap();
    independent.expect("different keys must not share one global lock");
    assert!(
        path.is_file(),
        "unlinking lock files allows simultaneous owners"
    );
    let again = KeyLock::acquire(dir.path(), KEY).unwrap();
    assert_eq!(again.path(), path);
}
