//! Unit tests for `sessions::for_process`: one process's sessions file, read
//! only from a `sessions/` directory that passes S10 and only when this user
//! owns it (security review finding 2, review issue 2).
//!
//! API under test:
//! - `for_process(dirs: &[PathBuf], pid: u32, start: u64)
//!   -> Result<Option<SessionFile>, String>`: a missing `sessions/` is
//!   skipped; one that is not a private directory is an error naming it
//! - `owned_session(raw: &[u8], meta: &fs::Metadata) -> Option<SessionFile>`:
//!   the owner check every sessions file passes, for `index` as well

use super::sessions::*;
use amplihack_utils::secure_files::euid;
use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const ID: &str = "0b6c3f2e-8d4a-4f1b-9c7e-2a5d6e8f1a3b";
const ID2: &str = "6f3b9d2e-4a7c-4e1b-8c5d-2e9a7f1b4d36";

fn chmod(p: &Path, m: u32) {
    fs::set_permissions(p, fs::Permissions::from_mode(m)).unwrap();
}

fn private_dir(p: &Path) -> PathBuf {
    fs::create_dir_all(p).unwrap();
    chmod(p, 0o700);
    p.to_path_buf()
}

fn write_session(dir: &Path, pid: u32, id: &str, start: u64) {
    let s = private_dir(&dir.join("sessions"));
    let body = json!({"pid": pid, "sessionId": id, "procStart": start.to_string(), "cwd": "/w"});
    fs::write(s.join(format!("{pid}.json")), body.to_string()).unwrap();
}

/// `home/.claude` and a profile directory `profiles/a`, both private.
fn dirs() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let claude = private_dir(&tmp.path().join("home/.claude"));
    let a = private_dir(&tmp.path().join("profiles/a"));
    (tmp, claude, a)
}

#[test]
fn for_process_matches_pid_and_start() {
    let (_t, claude, a) = dirs();
    write_session(&a, 20, ID, 200);
    let other = json!({"pid": 30, "sessionId": ID2, "procStart": "300"});
    let s = private_dir(&claude.join("sessions"));
    fs::write(s.join("other.json"), other.to_string()).unwrap();
    let dirs = [claude, a];
    let s = for_process(&dirs, 20, 200)
        .unwrap()
        .expect("found in the second directory");
    assert_eq!(
        (s.session_id.as_str(), s.cwd),
        (ID, Some(PathBuf::from("/w")))
    );
    assert_eq!(for_process(&dirs, 20, 201), Ok(None), "pid reused");
    assert_eq!(
        for_process(&dirs, 30, 300),
        Ok(None),
        "only <pid>.json is read"
    );
}

#[test]
fn a_missing_sessions_directory_is_skipped() {
    let (_t, claude, a) = dirs();
    assert_eq!(for_process(&[claude.clone(), a.clone()], 20, 200), Ok(None));
    write_session(&a, 20, ID, 200);
    let found = for_process(&[claude, a], 20, 200).unwrap();
    assert_eq!(found.map(|s| s.session_id), Some(ID.to_string()));
}

#[test]
fn a_sessions_directory_others_can_write_is_an_error() {
    let (_t, claude, a) = dirs();
    write_session(&claude, 20, ID, 200);
    chmod(&claude.join("sessions"), 0o777);
    let err = for_process(std::slice::from_ref(&claude), 20, 200).unwrap_err();
    assert_eq!(
        err,
        "~/.claude/sessions is not private: it must be owned by you and not writable by other users"
    );
    write_session(&a, 20, ID, 200);
    chmod(&a.join("sessions"), 0o777);
    let err = for_process(&[a], 20, 200).unwrap_err();
    assert!(
        err.starts_with("the sessions directory of profile a is not private: "),
        "{err}"
    );
}

#[test]
fn a_symlinked_sessions_directory_is_an_error() {
    let (tmp, claude, _a) = dirs();
    let elsewhere = private_dir(&tmp.path().join("elsewhere"));
    write_session(&elsewhere, 20, ID, 200);
    std::os::unix::fs::symlink(elsewhere.join("sessions"), claude.join("sessions")).unwrap();
    let got = for_process(&[claude], 20, 200);
    assert!(
        got.as_ref().is_err_and(|e| e.contains("is not private")),
        "{got:?}"
    );
}

#[test]
fn a_sessions_file_of_another_user_is_ignored() {
    let raw = json!({"pid": 20, "sessionId": ID, "procStart": "200"}).to_string();
    let (_t, claude, _a) = dirs();
    let own = fs::metadata(&claude).unwrap();
    assert!(owned_session(raw.as_bytes(), &own).is_some());
    if euid() != 0 {
        let root_owned = fs::metadata("/").unwrap();
        assert_eq!(owned_session(raw.as_bytes(), &root_owned), None);
    }
}
