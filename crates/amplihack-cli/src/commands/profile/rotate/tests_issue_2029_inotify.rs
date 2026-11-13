//! Linux-only decoder contract. Feed exact inotify records through a pipe
//! into the production reader; overflowing a real kernel queue would make
//! acceptance depend on host limits and concurrent activity.
use super::*;
use std::fs::File;
use std::io::Write;

#[test]
fn overflow_and_invalidated_watch_records_both_reach_recovery() {
    let mut fds = [-1; 2];
    // SAFETY: fds has room for both newly owned descriptors.
    assert_eq!(
        unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK | libc::O_CLOEXEC) },
        0
    );
    // SAFETY: pipe2 succeeded; each descriptor is adopted exactly once.
    let fd = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let mut writer = unsafe { File::from_raw_fd(fds[1]) };
    let tmp = tempfile::tempdir_in("/tmp").unwrap();
    let directory = tmp.path().join("observations");
    let mut backend = Backend {
        fd,
        dirs: BTreeMap::from([(directory.clone(), (17, 0, 0))]),
        wake: WakePipe::open().unwrap(),
        skipped: Skipped::default(),
    };
    for (wd, mask) in [
        (-1, libc::IN_Q_OVERFLOW),
        (17, libc::IN_IGNORED),
        (-1, libc::IN_Q_OVERFLOW),
    ] {
        let event = libc::inotify_event {
            wd,
            mask,
            cookie: 0,
            len: 0,
        };
        // SAFETY: inotify_event has four initialised 32-bit fields and no
        // padding. The byte slice is used only while event remains alive.
        let bytes = unsafe {
            std::slice::from_raw_parts(
                (&event as *const libc::inotify_event).cast::<u8>(),
                std::mem::size_of_val(&event),
            )
        };
        writer.write_all(bytes).unwrap();
    }
    let events = backend.read_events().unwrap().unwrap();
    assert_eq!(events.iter().filter(|e| **e == Event::Overflow).count(), 1);
    assert!(events.contains(&Event::Changed));
    assert!(
        !backend.watched().contains(&directory),
        "invalidated subscriptions survived overflow recovery input"
    );
    assert!(
        backend.read_events().unwrap().is_none(),
        "consumed events were replayed"
    );
}

#[test]
fn a_closed_native_event_source_is_terminal_instead_of_repeatedly_ready() {
    let mut fds = [-1; 2];
    // SAFETY: fds has room for the two newly owned descriptors.
    assert_eq!(
        unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK | libc::O_CLOEXEC) },
        0
    );
    // SAFETY: pipe2 succeeded; adopt each descriptor exactly once.
    let fd = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let writer = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    let backend = Backend {
        fd,
        dirs: BTreeMap::new(),
        wake: WakePipe::open().unwrap(),
        skipped: Skipped::default(),
    };
    drop(writer);
    assert!(backend.poll(true).is_err());
}

#[test]
fn releasing_an_unchanged_state_lock_does_not_notify_the_watcher() {
    use amplihack_utils::usage_observation::ObservationStore;
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir_in("/tmp").unwrap();
    std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = ObservationStore::open(tmp.path()).unwrap();
    drop(store.lock().unwrap());
    let state = tmp.path().join(".auto-switch.d");
    let mut backend = Backend::open(&[state]).unwrap();
    assert!(backend.drain().unwrap().is_empty());
    drop(store.lock().unwrap());
    assert!(
        backend.drain().unwrap().is_empty(),
        "a read's writable lock close created another read event"
    );
}
