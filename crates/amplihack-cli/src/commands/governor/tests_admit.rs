//! Issue #1811 (TDD): the admission loop (D1, D2, D3, D11).
//!
//! Wire with `#[cfg(all(test, unix))] mod tests_admit;` in `governor/mod.rs`.
//!
//! Expected API in `admit` (the public `admit(AdmitRequest)` reads the
//! environment and the profile root, then calls `admit_with`):
//! - `AdmitContext<'a> { root: Option<&'a Path>, settings: Settings, class:
//!   Class, workstream: String, wait_secs: u64, session_depth:
//!   Option<String>, tree_id: Option<String>, task_text: Option<String> }`
//! - `admit_with(&AdmitContext) -> Result<SlotGuard, Denied>`
//! - `SlotGuard::fail_open() -> Option<&'static str>`, `nested() -> bool`,
//!   `tree_id() -> Option<&str>`; dropping it removes its slot.
//! - `Denied::code() -> &'static str`; `Denied::report(self) ->
//!   anyhow::Error` with exit code `EXIT_GOVERNOR_DENIED` (80).
//! - `nested_parent(depth: Option<&str>, tree_id: Option<&str>, &[Slot])
//!   -> Option<usize>`.
//!
//! Every test passes `wait_secs: 0`, so nothing sleeps.

use super::EXIT_GOVERNOR_DENIED;
use super::admit::{AdmitContext, admit_with, nested_parent};
use super::config::Settings;
use super::liveness::start_time;
use super::model::{Class, QueueEntry, Slot};
use super::store::{History, Queue, State, Store};
use super::tests_seed::{Root, Seed};
use std::fs;
use std::time::Duration;

fn ctx(r: &Root) -> AdmitContext<'_> {
    AdmitContext {
        root: Some(&r.root),
        settings: Settings::default(),
        class: Class::Ordinary,
        workstream: "ws-test".into(),
        wait_secs: 0,
        session_depth: None,
        tree_id: None,
        task_text: None,
    }
}

fn me_slot(tree: &str) -> Slot {
    let pid = std::process::id();
    Slot {
        pid,
        start_time: start_time(pid),
        workstream: "ws-other".into(),
        class: Class::Ordinary,
        tree_id: tree.into(),
        admitted_at: 1_793_000_000,
        nested_children: 0,
    }
}

fn save(r: &Root, state: &State, queue: &Queue) {
    let store = Store::open(&r.root).unwrap();
    let lock = store.lock_exclusive(Duration::from_secs(1)).unwrap();
    store
        .save(&lock, &History::default(), queue, state)
        .unwrap();
}

fn load(r: &Root) -> (State, Queue) {
    let store = Store::open(&r.root).unwrap();
    let lock = store.lock_exclusive(Duration::from_secs(1)).unwrap();
    (store.load(&lock).unwrap(), store.load(&lock).unwrap())
}

const ROOMY: &[Seed] = &[("alpha", 10.0, 20.0)];

#[test]
fn admitted_start_holds_a_slot_until_the_guard_drops() {
    let r = Root::new(ROOMY);
    let guard = admit_with(&ctx(&r)).expect("room");
    assert_eq!(guard.fail_open(), None);
    let (state, _) = load(&r);
    assert_eq!(state.slots.len(), 1);
    assert_eq!(state.slots[0].pid, std::process::id());
    assert_eq!(state.slots[0].workstream, "ws-test");
    assert_eq!(state.admissions.len(), 1);
    drop(guard);
    assert!(load(&r).0.slots.is_empty());
}

#[test]
fn weekly_exhaustion_is_denied_with_exit_80() {
    let r = Root::new(&[("alpha", 10.0, 100.0), ("beta", -1.0, 100.0)]);
    let denied = admit_with(&ctx(&r)).err().expect("denied");
    assert_eq!(denied.code(), "weekly-exhausted");
    let err = denied.report();
    assert_eq!(
        crate::command_error::exit_code(&err),
        Some(EXIT_GOVERNOR_DENIED)
    );
    assert_eq!(EXIT_GOVERNOR_DENIED, 80);
    assert!(load(&r).0.slots.is_empty());
}

#[test]
fn an_expired_concurrency_wait_is_wait_expired_and_leaves_no_queue_entry() {
    let r = Root::new(ROOMY);
    let state = State {
        limit: 1,
        slots: vec![me_slot("t-held")],
        ..State::default()
    };
    save(&r, &state, &Queue::default());
    let mut c = ctx(&r);
    c.settings.max_concurrency = 1;
    let denied = admit_with(&c).err().expect("wait 0 ends at once");
    assert_eq!(denied.code(), "wait-expired");
    let (state, queue) = load(&r);
    assert_eq!(state.slots.len(), 1);
    assert!(queue.entries.is_empty(), "the queue ticket must be removed");
}

#[test]
fn nested_launch_runs_under_the_parent_slot() {
    let r = Root::new(ROOMY);
    let state = State {
        limit: 1,
        slots: vec![me_slot("t-parent")],
        ..State::default()
    };
    save(&r, &state, &Queue::default());
    let mut c = ctx(&r);
    c.session_depth = Some("1".into());
    c.tree_id = Some("t-parent".into());
    let guard = admit_with(&c).expect("nested is not limited by concurrency");
    assert!(guard.nested());
    let (state, _) = load(&r);
    assert_eq!(state.slots.len(), 1, "a nested launch takes no slot");
    assert_eq!(state.slots[0].nested_children, 1);
    drop(guard);
    assert_eq!(
        load(&r).0.slots.len(),
        1,
        "dropping a nested guard keeps the parent slot"
    );
}

#[test]
fn orphaned_nested_launch_is_top_level() {
    let r = Root::new(ROOMY);
    let mut c = ctx(&r);
    c.session_depth = Some("2".into());
    c.tree_id = Some("t-gone".into());
    let guard = admit_with(&c).expect("room");
    assert!(!guard.nested());
    assert_eq!(load(&r).0.slots.len(), 1);
}

#[test]
fn top_level_admission_settles_a_tree_id_and_records_it() {
    let r = Root::new(ROOMY);
    let guard = admit_with(&ctx(&r)).unwrap();
    let id = guard.tree_id().expect("a tree id").to_string();
    assert!(!id.is_empty() && id.len() <= 128);
    assert_eq!(load(&r).0.slots[0].tree_id, id);
    drop(guard);
    let mut c = ctx(&r);
    c.tree_id = Some("t-inherited".into());
    let guard = admit_with(&c).unwrap();
    assert_eq!(guard.tree_id(), Some("t-inherited"));
}

#[test]
fn nested_parent_detection() {
    let slots = [me_slot("t-a"), me_slot("t-b")];
    assert_eq!(nested_parent(Some("1"), Some("t-b"), &slots), Some(1));
    assert_eq!(nested_parent(Some("0"), Some("t-b"), &slots), None);
    assert_eq!(nested_parent(None, Some("t-b"), &slots), None);
    assert_eq!(nested_parent(Some("x"), Some("t-b"), &slots), None);
    assert_eq!(nested_parent(Some("1"), Some("t-c"), &slots), None);
    assert_eq!(nested_parent(Some("1"), Some("T-B"), &slots), None);
    assert_eq!(
        nested_parent(Some("1"), Some(&"t".repeat(129)), &slots),
        None
    );
}

fn fail_open_code(r: &Root, c: &AdmitContext) -> &'static str {
    let guard = admit_with(c).expect("a fault must admit");
    let code = guard.fail_open().expect("a fail-open code");
    drop(guard);
    let _ = r;
    code
}

#[test]
fn every_fault_admits_with_its_code() {
    let r = Root::new(ROOMY);
    let mut c = ctx(&r);
    c.root = None;
    assert_eq!(fail_open_code(&r, &c), "no-root");

    let r = Root::new(&[]);
    assert_eq!(fail_open_code(&r, &ctx(&r)), "no-readings");

    let r = Root::new(ROOMY);
    let outside = r.tmp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, r.gov()).unwrap();
    assert_eq!(fail_open_code(&r, &ctx(&r)), "state-dir");
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);

    let r = Root::new(ROOMY);
    Store::open(&r.root).unwrap();
    fs::write(r.gov().join("state.json"), "{bad").unwrap();
    assert_eq!(fail_open_code(&r, &ctx(&r)), "corrupt-state");
    assert!(r.gov().join("state.json.corrupt").exists());

    let r = Root::new(ROOMY);
    Store::open(&r.root).unwrap();
    fs::write(r.gov().join("state.json"), r#"{"version":7}"#).unwrap();
    assert_eq!(fail_open_code(&r, &ctx(&r)), "state-version");
    assert_eq!(
        fs::read_to_string(r.gov().join("state.json")).unwrap(),
        r#"{"version":7}"#
    );
}

#[test]
fn a_full_queue_admits_without_queueing() {
    let r = Root::new(ROOMY);
    let pid = std::process::id();
    let entries = (1..=4096)
        .map(|seq| QueueEntry {
            seq,
            workstream: "ws-full".into(),
            class: Class::Coordination,
            pid,
            start_time: start_time(pid),
            enqueued_at: 1_793_000_000,
        })
        .collect();
    let queue = Queue {
        entries,
        ..Queue::default()
    };
    let state = State {
        next_seq: 4097,
        ..State::default()
    };
    save(&r, &state, &queue);
    assert_eq!(fail_open_code(&r, &ctx(&r)), "queue-full");
}

#[test]
fn canary_secrets_never_reach_governor_state() {
    let r = Root::new(ROOMY);
    let auto = r.root.join(".auto-switch.d");
    let ledger = r#"{"version":1,"hosts":{"alpha":{"live":"acct-SECRET","swapped_at":1793000000,"pre_swap_fingerprint":null}}}"#;
    fs::write(auto.join("ledger.json"), ledger).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(auto.join("ledger.json"), fs::Permissions::from_mode(0o600)).unwrap();
    let mut c = ctx(&r);
    c.task_text = Some("TASKSECRET do the thing".into());
    let guard = admit_with(&c);
    let mut text = String::new();
    for entry in fs::read_dir(r.gov()).unwrap() {
        text.push_str(&String::from_utf8_lossy(
            &fs::read(entry.unwrap().path()).unwrap_or_default(),
        ));
    }
    drop(guard);
    for secret in ["acct-SECRET", "TASKSECRET", r.root.to_str().unwrap()] {
        assert!(!text.contains(secret), "{secret} leaked into .governor.d");
    }
}
