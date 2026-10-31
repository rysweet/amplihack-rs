//! Test helper (no tests): a synthetic `/proc` and Claude config directory
//! in a temp dir, read through the real `procfs::snapshot` and
//! `sessions::index`. `/proc/self` links to pid 100, an `amplihack` process.
//! Declared from `mod.rs` like the test modules; used by `tests_scan.rs` and
//! `tests_scan_observe.rs`.

use super::procfs::{Snapshot, snapshot};
use super::sessions::{Index, index};
use amplihack_utils::secure_files::euid;
use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

pub(crate) struct Tree {
    pub(crate) _tmp: tempfile::TempDir,
    pub(crate) proc_root: PathBuf,
    pub(crate) config: PathBuf,
}

pub(crate) fn tree() -> Tree {
    let tmp = tempfile::tempdir().unwrap();
    let proc_root = tmp.path().join("proc");
    let config = tmp.path().join("claude");
    fs::create_dir_all(config.join("sessions")).unwrap();
    for d in [&config, &config.join("sessions")] {
        fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::create_dir_all(&proc_root).unwrap();
    let t = Tree {
        _tmp: tmp,
        proc_root,
        config,
    };
    add(
        &t,
        100,
        1,
        "amplihack",
        &["amplihack", "agent", "supervise"],
        5,
        'S',
    );
    std::os::unix::fs::symlink("100", t.proc_root.join("self")).unwrap();
    t
}

pub(crate) fn add(
    t: &Tree,
    pid: u32,
    ppid: u32,
    comm: &str,
    argv: &[&str],
    start: u64,
    state: char,
) {
    add_as(t, pid, ppid, comm, argv, start, state, (euid(), euid()));
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn add_as(
    t: &Tree,
    pid: u32,
    ppid: u32,
    comm: &str,
    argv: &[&str],
    start: u64,
    state: char,
    (ruid, uid): (u32, u32),
) {
    let d = t.proc_root.join(pid.to_string());
    fs::create_dir_all(&d).unwrap();
    let stat = format!(
        "{pid} ({comm}) {state} {ppid} {pid} {pid} 0 -1 4194560 0 0 0 0 0 0 0 0 20 0 1 0 {start} 1000 100\n"
    );
    fs::write(d.join("stat"), stat).unwrap();
    let status = format!("Name:\t{comm}\nState:\t{state} (x)\nUid:\t{ruid}\t{uid}\t{uid}\t{uid}\n");
    fs::write(d.join("status"), status).unwrap();
    let cmdline: String = argv.iter().map(|a| format!("{a}\0")).collect();
    fs::write(d.join("cmdline"), cmdline).unwrap();
    fs::write(d.join("environ"), "PATH=/usr/bin\0").unwrap();
    std::os::unix::fs::symlink("/", d.join("cwd")).unwrap();
}

pub(crate) fn session_file(t: &Tree, pid: u32, id: &str, start: &str) {
    let body = json!({ "pid": pid, "sessionId": id, "procStart": start, "cwd": "/w", "kind": "x" });
    let p = t.config.join("sessions").join(format!("{pid}.json"));
    fs::write(p, body.to_string()).unwrap();
}

pub(crate) fn snap(t: &Tree) -> Snapshot {
    snapshot(&t.proc_root, euid()).expect("the synthetic /proc can be read")
}

pub(crate) fn idx(t: &Tree) -> Index {
    index(std::slice::from_ref(&t.config)).expect("the synthetic sessions directory can be read")
}

/// Makes the process's `cmdline` unreadable (it becomes a directory).
pub(crate) fn unreadable_cmdline(t: &Tree, pid: u32) {
    let cmdline = t.proc_root.join(pid.to_string()).join("cmdline");
    fs::remove_file(&cmdline).unwrap();
    fs::create_dir(&cmdline).unwrap();
}
