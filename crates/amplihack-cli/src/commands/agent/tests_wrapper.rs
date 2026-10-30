//! Unit tests for the wrapper's id thread lookup (D4, philosophy review F2)
//! against the synthetic `/proc` of `tests_proc_tree.rs`.
//!
//! API under test:
//! - `wrapper::find_claude(root: &Path, dirs: impl FnOnce() -> Result<Vec<PathBuf>,
//!   String>) -> Result<Option<Found>, String>`: `Ok(None)` until
//!   a Claude process below `/proc/self` and its sessions file both exist;
//!   an error, kept as the reason of the log line, for an unreadable process
//!   table, a config-directory failure or a sessions directory failing S10.
//!   `Found` names the config directory whose `sessions/` held the file.

use super::tests_proc_tree::*;
use super::wrapper::{Found, find_claude};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

const ID: &str = "0b6c3f2e-8d4a-4f1b-9c7e-2a5d6e8f1a3b";

#[test]
fn the_claude_process_below_and_its_conversation_are_found() {
    let t = tree();
    add(&t, 300, 1, "claude", &["claude"], 66, 'S');
    session_file(&t, 300, ID, "66");
    add(&t, 200, 100, "claude", &["claude"], 77, 'S');
    session_file(&t, 200, ID, "77");
    let got = find_claude(&t.proc_root, || Ok(vec![t.config.clone()]));
    let want = Found {
        pid: 200,
        start: 77,
        conversation: ID.to_string(),
        config_dir: t.config.clone(),
    };
    assert_eq!(got, Ok(Some(want)));
}

/// The config directory that held the sessions file is reported, so a
/// launch under an inherited or default-pointer profile records it.
#[test]
fn the_config_directory_holding_the_sessions_file_is_reported() {
    let t = tree();
    add(&t, 200, 100, "claude", &["claude"], 77, 'S');
    let other = t.config.parent().unwrap().join("other-config");
    fs::create_dir_all(other.join("sessions")).unwrap();
    for d in [&other, &other.join("sessions")] {
        fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    session_file(&t, 200, ID, "77");
    fs::rename(
        t.config.join("sessions").join("200.json"),
        other.join("sessions").join("200.json"),
    )
    .unwrap();
    let got = find_claude(&t.proc_root, || Ok(vec![t.config.clone(), other.clone()]));
    assert_eq!(got.unwrap().map(|f| f.config_dir), Some(other));
}

#[test]
fn nothing_found_yet_is_not_an_error() {
    let t = tree();
    let unused = || -> Result<Vec<PathBuf>, String> { panic!("listed before a process was found") };
    assert_eq!(find_claude(&t.proc_root, unused), Ok(None));
    add(&t, 200, 100, "claude", &["claude"], 77, 'S');
    let dirs = || Ok(vec![t.config.clone()]);
    assert_eq!(
        find_claude(&t.proc_root, dirs),
        Ok(None),
        "no sessions file yet"
    );
    session_file(&t, 200, ID, "76");
    assert_eq!(
        find_claude(&t.proc_root, dirs),
        Ok(None),
        "another start time"
    );
}

#[test]
fn every_failure_is_a_reason() {
    let t = tree();
    add(&t, 200, 100, "claude", &["claude"], 77, 'S');
    session_file(&t, 200, ID, "77");
    let refused = find_claude(&t.proc_root, || Err("~/.claude is not private".into()));
    assert_eq!(refused, Err("~/.claude is not private".to_string()));
    let sessions = t.config.join("sessions");
    fs::set_permissions(&sessions, fs::Permissions::from_mode(0o777)).unwrap();
    let open = find_claude(&t.proc_root, || Ok(vec![t.config.clone()]));
    assert!(
        open.as_ref().is_err_and(|e| e.contains("is not private")),
        "{open:?}"
    );
    fs::remove_file(t.proc_root.join("self")).unwrap();
    let no_proc = find_claude(&t.proc_root, || Ok(vec![t.config.clone()]));
    assert_eq!(no_proc, Err("cannot read the process table".to_string()));
}
