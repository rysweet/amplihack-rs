//! Issue #1811 (TDD): `.governor.d` state files (D3, D11, D13).
//!
//! Wire with `#[cfg(all(test, unix))] mod tests_store;` in `governor/mod.rs`.
//!
//! Expected API in `store`:
//! - `GOVERNOR_DIR == ".governor.d"`.
//! - `Store::open(root) -> Result<Store, FailOpen>`: creates the directory
//!   0700; refuses a symlink or another owner (`state-dir`), never chmods.
//! - `Store::lock_exclusive(&self, Duration) -> Result<StoreLock, FailOpen>`
//!   (`lock-timeout`).
//! - `Store::load::<T>(&self, &StoreLock) -> Result<T, FailOpen>` for
//!   `State`, `Queue` and `History`: missing gives `T::default()`; corrupt
//!   (bad JSON, unknown field, failed validation, over 1 MiB) is renamed to
//!   `<file>.corrupt` and gives `corrupt-state`; a newer version gives
//!   `state-version` and leaves the file alone.
//! - `Store::save(&self, &StoreLock, &History, &Queue, &State)`.
//! - `Store::peek::<T>(root) -> Peek<T>` (`Ok`, `Missing`, `Corrupt`,
//!   `NewerVersion`): never creates, renames or writes anything.
//! - `repair(&mut State, &Queue, max: u32)`: `next_seq` past every seq,
//!   `limit` clamped to `1..=max`.
//!
//! `model::FailOpen { code: FailOpenCode, message }`, `FailOpenCode::as_str()`.

use super::model::{Class, QueueEntry, Slot};
use super::store::{GOVERNOR_DIR, History, Peek, Queue, State, Store, repair};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::time::Duration;

const SEC: Duration = Duration::from_secs(1);

fn mode(p: &Path) -> u32 {
    fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777
}

fn entry(seq: u64) -> QueueEntry {
    QueueEntry {
        seq,
        workstream: "ws-1".into(),
        class: Class::Ordinary,
        pid: 4242,
        start_time: Some(7),
        enqueued_at: 1_800_000_000,
    }
}

fn state_with_slot() -> State {
    State {
        limit: 3,
        next_seq: 5,
        slots: vec![Slot {
            pid: 4242,
            start_time: Some(7),
            workstream: "ws-1".into(),
            class: Class::Review,
            tree_id: "t-1".into(),
            admitted_at: 1_800_000_000,
            nested_children: 2,
            start: super::model::Start::Agent,
        }],
        ..State::default()
    }
}

fn code<T>(r: Result<T, super::model::FailOpen>) -> &'static str {
    match r {
        Ok(_) => "ok",
        Err(e) => e.code.as_str(),
    }
}

#[test]
fn open_creates_a_private_directory() {
    let tmp = tempfile::tempdir().unwrap();
    Store::open(tmp.path()).unwrap();
    assert_eq!(mode(&tmp.path().join(GOVERNOR_DIR)), 0o700);
}

#[test]
fn round_trip_writes_private_versioned_files() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path()).unwrap();
    let lock = store.lock_exclusive(SEC).unwrap();
    let queue = Queue {
        entries: vec![entry(4)],
        ..Queue::default()
    };
    store
        .save(&lock, &History::default(), &queue, &state_with_slot())
        .unwrap();
    assert_eq!(store.load::<State>(&lock).unwrap(), state_with_slot());
    assert_eq!(store.load::<Queue>(&lock).unwrap(), queue);
    let dir = tmp.path().join(GOVERNOR_DIR);
    for name in ["state.json", "queue.json", "history.json"] {
        assert_eq!(mode(&dir.join(name)), 0o600, "{name}");
        let v: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.join(name)).unwrap()).unwrap();
        assert_eq!(v["version"], 1, "{name}");
    }
}

#[test]
fn missing_files_load_as_defaults() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path()).unwrap();
    let lock = store.lock_exclusive(SEC).unwrap();
    assert_eq!(store.load::<State>(&lock).unwrap(), State::default());
    assert_eq!(store.load::<Queue>(&lock).unwrap(), Queue::default());
}

fn load_raw(raw: &[u8]) -> (&'static str, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path()).unwrap();
    let path = tmp.path().join(GOVERNOR_DIR).join("state.json");
    fs::write(&path, raw).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let lock = store.lock_exclusive(SEC).unwrap();
    (code(store.load::<State>(&lock)), tmp)
}

#[test]
fn corrupt_files_are_quarantined_and_rebuilt() {
    let unknown = br#"{"version":1,"limit":2,"last_evaluated_at":null,"next_seq":1,"slots":[],"admissions":[],"fail_open":null,"extra":1}"#;
    let big = vec![b' '; 2 << 20];
    let cases: [&[u8]; 4] = [b"{not json", unknown, br#"{"version":1,"limit":65,"last_evaluated_at":null,"next_seq":1,"slots":[],"admissions":[],"fail_open":null}"#, &big];
    for raw in cases {
        let (c, tmp) = load_raw(raw);
        assert_eq!(
            c,
            "corrupt-state",
            "{}",
            String::from_utf8_lossy(&raw[..raw.len().min(40)])
        );
        let dir = tmp.path().join(GOVERNOR_DIR);
        assert!(dir.join("state.json.corrupt").is_file());
        assert!(!dir.join("state.json").exists());
        let store = Store::open(tmp.path()).unwrap();
        let lock = store.lock_exclusive(SEC).unwrap();
        assert_eq!(store.load::<State>(&lock).unwrap(), State::default());
    }
}

#[test]
fn duplicate_queue_seqs_fail_validation() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path()).unwrap();
    let lock = store.lock_exclusive(SEC).unwrap();
    let q = Queue {
        entries: vec![entry(3), entry(3)],
        ..Queue::default()
    };
    let path = tmp.path().join(GOVERNOR_DIR).join("queue.json");
    fs::write(&path, serde_json::to_vec(&q).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(code(store.load::<Queue>(&lock)), "corrupt-state");
}

#[test]
fn newer_version_fails_open_and_is_left_untouched() {
    let raw = br#"{"version":2,"anything":true}"#;
    let (c, tmp) = load_raw(raw);
    assert_eq!(c, "state-version");
    let dir = tmp.path().join(GOVERNOR_DIR);
    assert_eq!(fs::read(dir.join("state.json")).unwrap(), raw);
    assert!(!dir.join("state.json.corrupt").exists());
}

#[test]
fn repair_moves_next_seq_past_the_queue_and_clamps_the_limit() {
    let mut s = State {
        next_seq: 2,
        limit: 9,
        ..State::default()
    };
    let q = Queue {
        entries: vec![entry(10), entry(4)],
        ..Queue::default()
    };
    repair(&mut s, &q, 4);
    assert_eq!((s.next_seq, s.limit), (11, 4));
    s.limit = 0;
    repair(&mut s, &Queue::default(), 4);
    assert_eq!(s.limit, 1);
}

#[test]
fn symlinked_governor_dir_fails_open_and_target_is_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("elsewhere");
    fs::create_dir(&target).unwrap();
    symlink(&target, tmp.path().join(GOVERNOR_DIR)).unwrap();
    assert_eq!(code(Store::open(tmp.path())), "state-dir");
    assert_eq!(fs::read_dir(&target).unwrap().count(), 0);
}

#[test]
fn symlinked_state_file_lock_and_corrupt_target_are_refused() {
    for link in ["state.json", "lock", "state.json.corrupt"] {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside");
        fs::write(&outside, "keep").unwrap();
        let store = Store::open(tmp.path()).unwrap();
        let dir = tmp.path().join(GOVERNOR_DIR);
        if link == "state.json.corrupt" {
            fs::write(dir.join("state.json"), "{garbage").unwrap();
            fs::set_permissions(dir.join("state.json"), fs::Permissions::from_mode(0o600)).unwrap();
        }
        symlink(&outside, dir.join(link)).unwrap();
        let result = store
            .lock_exclusive(SEC)
            .and_then(|lock| store.load::<State>(&lock).map(|_| ()));
        assert!(result.is_err(), "{link}: a symlink must fail open");
        assert_eq!(fs::read_to_string(&outside).unwrap(), "keep", "{link}");
        assert!(
            fs::symlink_metadata(dir.join(link))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}

#[test]
fn second_lock_times_out() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path()).unwrap();
    let _held = store.lock_exclusive(SEC).unwrap();
    let other = Store::open(tmp.path()).unwrap();
    assert_eq!(
        code(other.lock_exclusive(Duration::from_millis(50))),
        "lock-timeout"
    );
}

#[test]
fn peek_never_creates_renames_or_writes() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(matches!(Store::peek::<State>(tmp.path()), Peek::Missing));
    assert!(!tmp.path().join(GOVERNOR_DIR).exists());
    Store::open(tmp.path()).unwrap();
    let path = tmp.path().join(GOVERNOR_DIR).join("state.json");
    fs::write(&path, "{bad").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(Store::peek::<State>(tmp.path()), Peek::Corrupt));
    assert_eq!(fs::read_to_string(&path).unwrap(), "{bad");
    fs::write(&path, r#"{"version":9}"#).unwrap();
    assert!(matches!(
        Store::peek::<State>(tmp.path()),
        Peek::NewerVersion
    ));
    let names: Vec<_> = fs::read_dir(tmp.path().join(GOVERNOR_DIR))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n != "lock")
        .collect();
    assert_eq!(names, ["state.json"], "peek wrote or renamed a file");
}

#[test]
fn hostile_corrupt_content_never_makes_a_state_file_that_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path()).unwrap();
    let path = tmp.path().join(GOVERNOR_DIR).join("history.json");
    let field = format!("\x1b[31m{}", "x".repeat(600));
    fs::write(&path, format!(r#"{{"version":1,"{field}":1}}"#)).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let lock = store.lock_exclusive(SEC).unwrap();
    let fault = store.load::<History>(&lock).unwrap_err();
    let mut state = state_with_slot();
    state.fail_open = Some(super::admit_state::record(&fault, 1_800_000_000));
    let msg = &state.fail_open.as_ref().unwrap().message;
    assert!(msg.len() <= 512, "{} bytes", msg.len());
    assert!(!msg.chars().any(char::is_control), "{msg:?}");
    store
        .save(&lock, &History::default(), &Queue::default(), &state)
        .unwrap();
    assert_eq!(store.load::<State>(&lock).unwrap(), state);
    assert!(
        !tmp.path()
            .join(GOVERNOR_DIR)
            .join("state.json.corrupt")
            .exists()
    );
}
