// crates/amplihack-cli/src/commands/recipe/run/tests_parent_watch.rs
//
// Issue #1976 (R8, D8-D10): a nested `amplihack recipe run` watches the
// parent `recipe run` named by AMPLIHACK_PARENT_RECIPE_PID and
// AMPLIHACK_PARENT_RECIPE_STARTTIME, and records an orphan stop in
// `orphaned.json` under AMPLIHACK_AUTODRIVE_STATE_DIR.
//
// Contract pinned here:
// - `parent_watch::proc_starttime(stat_text)` takes field 22 of a
//   `/proc/<pid>/stat` line, counted after the LAST `)`, so a crafted comm
//   cannot shift the fields (C-S2).
// - `parent_watch::parse_parent_env(pid, starttime)` is the strict env
//   parser (C-S1): both set and valid gives `Parsed`, neither set gives
//   `Absent`, anything else gives `Invalid` (warned, treated as top level).
// - `parent_watch::inspect_parent(pid, starttime)` returns `Gone` for a dead
//   PID or a start-time mismatch and `Alive` for the matching live process.
// - `orphan_record::write_orphan_record(dir, &record)` writes only into an
//   absolute, non-symlink, uid-owned directory with no group or other write
//   bit (C-S9), as a 0600 file holding only the minimal fields (C-S10/C-S11).
//
// No sleeps and no wall-clock windows: every process here is started, read
// and reaped by this test, and the only waits are `wait()` on a child.
#![cfg(target_os = "linux")]

use super::orphan_record::{OrphanRecord, write_orphan_record};
use super::parent_watch::{
    ParentEnv, ParentState, inspect_parent, parse_parent_env, proc_starttime,
};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::process::{Child, Command, Stdio};

// -------------------------------------------------------------------------
// proc_starttime: field 22 after the last ')'
// -------------------------------------------------------------------------

/// A `/proc/<pid>/stat` line with `comm` and `starttime` at field 22.
fn stat_line(pid: i32, comm: &str, starttime: u64) -> String {
    // Fields 3..=21 after comm, then field 22 = starttime, then a tail.
    let mid: Vec<String> = (3..=21).map(|n| n.to_string()).collect();
    format!(
        "{pid} ({comm}) {} {starttime} 23 24 25\n",
        mid.join(" ").replacen('3', "S", 1)
    )
}

#[test]
fn starttime_is_field_22() {
    assert_eq!(
        proc_starttime(&stat_line(42, "amplihack", 987_654)),
        Some(987_654)
    );
}

#[test]
fn a_comm_with_parens_and_spaces_cannot_shift_the_fields() {
    let line = stat_line(42, "evil) x y 1 2 3 4 5 6 7 8 9", 555);
    assert_eq!(proc_starttime(&line), Some(555));
}

#[test]
fn a_truncated_or_garbled_stat_gives_none() {
    for text in [
        "",
        "42 (x)",
        "42 (x) S 1 2",
        "no parens at all 1 2 3",
        "42 (x) S",
    ] {
        assert_eq!(proc_starttime(text), None, "{text:?}");
    }
    let mut bad = stat_line(42, "x", 1);
    bad = bad.replace(" 1 23 24 25", " notanumber 23 24 25");
    assert_eq!(proc_starttime(&bad), None, "{bad:?}");
}

#[test]
fn the_parser_agrees_with_this_process_own_stat() {
    let text = fs::read_to_string("/proc/self/stat").unwrap();
    let parsed = proc_starttime(&text).expect("own stat parses");
    // Cross-check with a naive split, which is safe for this process's comm.
    let naive: u64 = text
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .nth(19)
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(parsed, naive);
}

// -------------------------------------------------------------------------
// parse_parent_env: strict (C-S1)
// -------------------------------------------------------------------------

#[test]
fn neither_variable_set_is_a_top_level_run() {
    assert_eq!(parse_parent_env(None, None), ParentEnv::Absent);
}

#[test]
fn both_valid_values_parse() {
    assert_eq!(
        parse_parent_env(Some("4242"), Some("123456")),
        ParentEnv::Parsed {
            pid: 4242,
            starttime: 123_456
        }
    );
}

#[test]
fn bad_or_partial_values_are_invalid_never_a_stop() {
    for (pid, start, why) in [
        (Some("0"), Some("1"), "pid 0"),
        (Some("1"), Some("1"), "pid 1 (init)"),
        (Some("-5"), Some("1"), "negative pid"),
        (Some("abc"), Some("1"), "non-numeric pid"),
        (Some("99999999999"), Some("1"), "pid overflows i32"),
        (Some(""), Some("1"), "empty pid"),
        (Some("4242"), Some("x"), "non-numeric start time"),
        (Some("4242"), Some("-1"), "negative start time"),
        (Some("4242"), None, "only the pid set"),
        (None, Some("123"), "only the start time set"),
    ] {
        assert!(
            matches!(parse_parent_env(pid, start), ParentEnv::Invalid(_)),
            "{why}: {:?}",
            parse_parent_env(pid, start)
        );
    }
}

// -------------------------------------------------------------------------
// inspect_parent: pidfd, then stat, then compare (C-S3)
// -------------------------------------------------------------------------

/// A live stand-in process that blocks on its stdin pipe (no sleep).
fn blocked_child() -> Child {
    Command::new("cat")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn cat")
}

fn starttime_of(pid: u32) -> u64 {
    proc_starttime(&fs::read_to_string(format!("/proc/{pid}/stat")).unwrap()).unwrap()
}

fn end(mut child: Child) {
    drop(child.stdin.take());
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn a_live_parent_with_the_matching_start_time_is_alive() {
    let child = blocked_child();
    let pid = child.id();
    let start = starttime_of(pid);
    let state = inspect_parent(pid as i32, start);
    end(child);
    assert!(matches!(state, ParentState::Alive(_)), "{state:?}");
}

#[test]
fn a_live_pid_with_another_start_time_is_gone() {
    let child = blocked_child();
    let pid = child.id();
    let start = starttime_of(pid);
    let state = inspect_parent(pid as i32, start.wrapping_add(1));
    end(child);
    assert!(matches!(state, ParentState::Gone), "{state:?}");
}

#[test]
fn a_reaped_pid_is_gone() {
    let child = blocked_child();
    let pid = child.id();
    let start = starttime_of(pid);
    end(child);
    let state = inspect_parent(pid as i32, start);
    assert!(matches!(state, ParentState::Gone), "{state:?}");
}

// -------------------------------------------------------------------------
// orphaned.json (C-S9, C-S10, C-S11)
// -------------------------------------------------------------------------

fn record() -> OrphanRecord {
    OrphanRecord {
        parent_pid: 4242,
        tree_id: "tree-1976".to_string(),
        run_id: "run-1976".to_string(),
    }
}

fn private_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

#[test]
fn a_private_state_dir_gets_a_0600_record_with_only_the_minimal_fields() {
    let dir = private_dir();
    assert!(write_orphan_record(dir.path(), &record()).is_ok());
    let path = dir.path().join("orphaned.json");
    let meta = fs::symlink_metadata(&path).unwrap();
    assert!(meta.file_type().is_file());
    assert_eq!(meta.mode() & 0o777, 0o600);
    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(json["parent_pid"], 4242);
    assert_eq!(json["tree_id"], "tree-1976");
    assert_eq!(json["run_id"], "run-1976");
    assert_eq!(json["reason"], "parent recipe run exited");
    assert!(json["timestamp"].is_string(), "{json}");
    let mut keys: Vec<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["parent_pid", "reason", "run_id", "timestamp", "tree_id"]
    );
    // No temp file is left behind.
    let names: Vec<String> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["orphaned.json"]);
}

#[test]
fn a_relative_state_dir_is_skipped() {
    let rel = std::path::Path::new("relative-state-dir-1976");
    assert!(write_orphan_record(rel, &record()).is_err());
    assert!(!rel.exists());
}

#[test]
fn a_symlinked_state_dir_is_skipped() {
    let real = private_dir();
    let holder = private_dir();
    let link = holder.path().join("link");
    std::os::unix::fs::symlink(real.path(), &link).unwrap();
    assert!(write_orphan_record(&link, &record()).is_err());
    assert!(!real.path().join("orphaned.json").exists());
}

#[test]
fn a_group_or_other_writable_state_dir_is_skipped() {
    for mode in [0o720, 0o702, 0o777] {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(mode)).unwrap();
        assert!(
            write_orphan_record(dir.path(), &record()).is_err(),
            "{mode:o}"
        );
        assert!(!dir.path().join("orphaned.json").exists(), "{mode:o}");
    }
}

#[test]
fn a_missing_state_dir_or_a_file_is_skipped() {
    let dir = private_dir();
    assert!(write_orphan_record(&dir.path().join("missing"), &record()).is_err());
    let file = dir.path().join("plain-file");
    fs::write(&file, "x").unwrap();
    assert!(write_orphan_record(&file, &record()).is_err());
}

#[test]
fn a_planted_symlink_named_orphaned_json_is_not_followed() {
    let dir = private_dir();
    let target = private_dir();
    let victim = target.path().join("victim");
    fs::write(&victim, "keep").unwrap();
    std::os::unix::fs::symlink(&victim, dir.path().join("orphaned.json")).unwrap();
    // The rename replaces the link; the victim is never written through it.
    let _ = write_orphan_record(dir.path(), &record());
    assert_eq!(fs::read_to_string(&victim).unwrap(), "keep");
}
