//! Unit tests for [`super::step`] (issue #3435): step ids, the per-step
//! delivered set, single delivery under concurrent hooks, and the liveness
//! check. Every test works in its own temporary folder.

use super::log_state::read_log_state;
use super::step::{
    deliver, judge_alive, new_step_id, parse_start_time, process_start, undelivered, valid_step_id,
};
use super::*;
use std::collections::{HashMap, HashSet};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

fn private_root() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    (tmp, root)
}

fn run_dir(root: &Path, id: &str) -> PathBuf {
    let run = root.join("runs").join(id);
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&run)
        .unwrap();
    fs::set_permissions(root.join("runs"), fs::Permissions::from_mode(0o700)).unwrap();
    run.join(GUIDANCE_DIR)
}

fn name_of(appended: &Appended) -> String {
    appended
        .path
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string()
}

fn injected(dir: &Path) -> Vec<serde_json::Value> {
    let log = dir.parent().unwrap().join(GUIDANCE_LOG);
    fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .filter(|v| v["event"] == "guidance_injected")
        .collect()
}

#[test]
fn a_step_id_is_new_for_every_launch_and_keeps_a_valid_run_id() {
    let a = new_step_id("run-1");
    let b = new_step_id("run-1");
    assert_ne!(a, b);
    assert!(a.starts_with("run-1-"), "{a}");
    assert!(valid_step_id(&a));
    for bad in ["", "../x", "a b", "x/y"] {
        let id = new_step_id(bad);
        assert!(valid_step_id(&id), "{id}");
        assert!(!id.contains(bad) || bad.is_empty(), "{id}");
    }
    assert!(!valid_step_id(""));
    assert!(!valid_step_id(".hidden"));
    assert!(!valid_step_id("a/b"));
    assert!(!valid_step_id(&"x".repeat(201)));
}

#[test]
fn undelivered_leaves_out_entries_given_to_the_step_in_any_copy() {
    let (_tmp, root) = private_root();
    let one = run_dir(&root, "one");
    let two = run_dir(&root, "two");
    let first = append_entry(&one, "first", "alice").unwrap();
    let now = chrono::Utc::now();
    let a = append_entry_at(&one, "copied", "alice", now).unwrap();
    let b = append_entry_at(&two, "copied", "alice", now).unwrap();
    let entries = read_entries(&[one.clone(), two.clone()]);
    assert_eq!(entries.len(), 2);

    let none: HashMap<PathBuf, HashSet<String>> = HashMap::new();
    assert_eq!(undelivered(&entries, &none).len(), 2);

    // The copy in the second folder was given: the entry was given.
    let mut given = HashMap::new();
    given.insert(two.clone(), HashSet::from([name_of(&b)]));
    let left = undelivered(&entries, &given);
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].name, name_of(&first));

    let mut given = HashMap::new();
    given.insert(one.clone(), HashSet::from([name_of(&a), name_of(&first)]));
    assert!(undelivered(&entries, &given).is_empty());
}

#[test]
fn an_entry_is_delivered_to_a_step_once_and_to_another_step_again() {
    let (_tmp, root) = private_root();
    let dir = run_dir(&root, "run-d");
    let first = append_entry(&dir, "do X", "alice").unwrap();

    let block = deliver("run-d-step1", "run-d", std::slice::from_ref(&dir))
        .unwrap()
        .unwrap();
    assert!(block.contains("do X"));
    assert_eq!(
        deliver("run-d-step1", "run-d", std::slice::from_ref(&dir)).unwrap(),
        None
    );

    // A second entry: only it is given, at the next call.
    let second = append_entry(&dir, "then Y", "alice").unwrap();
    let block = deliver("run-d-step1", "run-d", std::slice::from_ref(&dir))
        .unwrap()
        .unwrap();
    assert!(block.contains("then Y"));
    assert!(!block.contains("do X"));

    // Another step is given both.
    let block = deliver("run-d-step2", "run-d", std::slice::from_ref(&dir))
        .unwrap()
        .unwrap();
    assert!(block.contains("do X") && block.contains("then Y"));

    let records = injected(&dir);
    assert_eq!(records.len(), 3);
    assert_eq!(records[0]["step_id"], "run-d-step1");
    assert_eq!(
        records[0]["entry_names"],
        serde_json::json!([name_of(&first)])
    );
    assert_eq!(
        records[1]["entry_names"],
        serde_json::json!([name_of(&second)])
    );
    assert_eq!(records[2]["step_id"], "run-d-step2");
    let state = read_log_state(&dir);
    assert_eq!(state.delivered_by_step["run-d-step1"].len(), 2);
    assert!(state.read.contains(&name_of(&first)));
}

#[test]
fn entries_given_at_launch_are_not_given_again_by_a_hook() {
    let (_tmp, root) = private_root();
    let dir = run_dir(&root, "run-l");
    append_entry(&dir, "at launch", "alice").unwrap();
    let launch = GuidanceForLaunch::from_dirs(vec![dir.clone()]);
    launch.record_injection("run-l", Some("run-l-s"));
    assert_eq!(
        deliver("run-l-s", "run-l", std::slice::from_ref(&dir)).unwrap(),
        None
    );
    // A launch record with no step id says nothing about this step.
    launch.record_injection("run-l", None);
    assert!(
        deliver("run-l-other", "run-l", std::slice::from_ref(&dir))
            .unwrap()
            .is_some()
    );
}

#[test]
fn hooks_that_run_at_the_same_moment_deliver_an_entry_once() {
    let (_tmp, root) = private_root();
    let dir = run_dir(&root, "run-c");
    append_entry(&dir, "only once", "alice").unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let dir = dir.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                deliver("run-c-s", "run-c", &[dir]).unwrap()
            })
        })
        .collect();
    let given: Vec<String> = handles
        .into_iter()
        .filter_map(|h| h.join().unwrap())
        .collect();
    assert_eq!(given.len(), 1, "{given:?}");
    assert_eq!(injected(&dir).len(), 1);
}

#[test]
fn nothing_to_deliver_without_entries_or_with_an_invalid_step_id() {
    let (_tmp, root) = private_root();
    let dir = run_dir(&root, "run-e");
    assert_eq!(deliver("run-e-s", "run-e", &[]).unwrap(), None);
    fs_safety::ensure_private_dir(&dir).unwrap();
    assert_eq!(
        deliver("run-e-s", "run-e", std::slice::from_ref(&dir)).unwrap(),
        None
    );
    assert!(deliver("bad id", "run-e", &[dir]).is_err());
}

#[test]
fn the_start_time_is_field_22_counted_after_the_command_name() {
    let stat = "42 (a) b) c) S 1 42 42 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 987654 1000 10";
    assert_eq!(parse_start_time(stat), Some(987654));
    assert_eq!(parse_start_time("42 (x) S 1"), None);
    assert_eq!(parse_start_time("no parens"), None);
}

#[test]
fn a_step_is_alive_only_while_its_launcher_runs_with_its_start_time() {
    assert!(judge_alive(Some(5), Some(5), true));
    // The pid was reused by a later process.
    assert!(!judge_alive(Some(5), Some(9), true));
    // The process is gone.
    assert!(!judge_alive(Some(5), None, false));
    // No start time on this system: the pid decides.
    assert!(judge_alive(None, None, true));
    assert!(!judge_alive(None, None, false));
}

#[cfg(target_os = "linux")]
#[test]
fn this_process_has_a_start_time() {
    let pid = std::process::id();
    let start = process_start(pid);
    assert!(start.is_some());
    assert_eq!(start, process_start(pid));
}
