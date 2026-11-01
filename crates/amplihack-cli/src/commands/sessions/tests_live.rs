//! `sessions::live` (issue #1746): which session ids are running. The main
//! safety property: a session whose pid file says it is live is never
//! archived, whichever Claude config directory holds the pid file.
//!
//! Contract:
//! - `trait Probe { fn alive(&self, pid: i32) -> bool; fn config_dirs(&self)
//!   -> io::Result<Vec<PathBuf>>; }`; production calls `kill(pid, 0)` and
//!   reads `/proc`. A `config_dirs` error fails the build closed.
//! - `build_from(home, profile_root, &process_dirs(probe)?, probe)
//!   -> Result<LiveSet, (PathBuf, Why)>` (as `build` here): reads
//!   `<home>/.claude/sessions`, `<profile>/sessions` for every profile
//!   listed by `ProfileStore::list`, and `<dir>/sessions` for each of
//!   `probe.config_dirs()`, each canonical directory once. NotFound is
//!   empty; any other error fails closed with that directory and kind.
//! - `LiveSet::contains(&self, id: &str) -> bool`.
//! - Only regular `*.json` files up to 64 KiB, not followed through
//!   symlinks. Candidate pids: body `pid` (a JSON integer) and the stem
//!   (ASCII digits only), each only in `1..=i32::MAX`. Live if either is
//!   alive. `sessionId` must pass `scan::valid_name`.
//! - `crate::commands::profile::claude_config_dirs(proc_root, self_pid,
//!   uid) -> io::Result<Vec<PathBuf>>`: `CLAUDE_CONFIG_DIR` of same-uid
//!   processes; an unreadable `proc_root`, or an unreadable environment of
//!   a process whose cmdline names claude or amplihack, is an error.

use super::live::{LiveError, LiveSet, Probe, build_from, process_dirs};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

/// Both steps of a full build, as the archive does it at the start of a run.
fn build(home: &Path, profile_root: &Path, probe: &dyn Probe) -> Result<LiveSet, LiveError> {
    build_from(home, profile_root, &process_dirs(probe)?, probe)
}

/// Alive pids are listed; every pid asked about is recorded.
#[derive(Default)]
struct FakeProbe {
    alive: BTreeSet<i32>,
    dirs: Vec<PathBuf>,
    dirs_error: Option<io::ErrorKind>,
    asked: RefCell<Vec<i32>>,
}

impl Probe for FakeProbe {
    fn alive(&self, pid: i32) -> bool {
        self.asked.borrow_mut().push(pid);
        self.alive.contains(&pid)
    }
    fn config_dirs(&self) -> io::Result<Vec<PathBuf>> {
        match self.dirs_error {
            Some(kind) => Err(kind.into()),
            None => Ok(self.dirs.clone()),
        }
    }
}

fn probe(alive: &[i32]) -> FakeProbe {
    FakeProbe {
        alive: alive.iter().copied().collect(),
        ..Default::default()
    }
}

struct T {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
}

fn setup() -> T {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let home = base.join("home");
    let root = base.join("profiles");
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::create_dir_all(&root).unwrap();
    T {
        _tmp: tmp,
        home,
        root,
    }
}

fn pid_file(dir: &Path, stem: &str, body: &str) {
    fs::create_dir_all(dir.join("sessions")).unwrap();
    fs::write(dir.join("sessions").join(format!("{stem}.json")), body).unwrap();
}

fn body(id: &str, pid: &str) -> String {
    format!(r#"{{"pid":{pid},"sessionId":"{id}","cwd":"/x","kind":"interactive"}}"#)
}

#[test]
fn nothing_anywhere_is_empty() {
    let t = setup();
    let set = build(&t.home, &t.root, &probe(&[])).unwrap();
    assert!(!set.contains("anything"));
}

#[test]
fn missing_profile_root_is_empty_not_an_error() {
    let t = setup();
    fs::remove_dir(&t.root).unwrap();
    assert!(build(&t.home, &t.root, &probe(&[])).is_ok());
}

#[test]
fn home_sessions_dir_is_read() {
    let t = setup();
    pid_file(&t.home.join(".claude"), "100", &body("s-home", "100"));
    let set = build(&t.home, &t.root, &probe(&[100])).unwrap();
    assert!(set.contains("s-home"));
}

#[test]
fn every_profile_sessions_dir_is_read() {
    let t = setup();
    for (p, pid) in [("work", "201"), ("spare", "202")] {
        let dir = t.root.join(p);
        fs::create_dir_all(&dir).unwrap();
        pid_file(&dir, pid, &body(&format!("s-{p}"), pid));
    }
    let set = build(&t.home, &t.root, &probe(&[201, 202])).unwrap();
    assert!(set.contains("s-work"));
    assert!(set.contains("s-spare"));
}

#[test]
fn process_config_dirs_are_read() {
    let t = setup();
    let cfg = t.home.join("custom-config");
    pid_file(&cfg, "300", &body("s-proc", "300"));
    let mut pr = probe(&[300]);
    pr.dirs = vec![cfg];
    let set = build(&t.home, &t.root, &pr).unwrap();
    assert!(set.contains("s-proc"));
}

#[test]
fn the_same_directory_is_read_once() {
    let t = setup();
    // A non-numeric stem, so each read of the file probes the body pid once.
    pid_file(&t.home.join(".claude"), "x", &body("s", "400"));
    let mut pr = probe(&[]);
    // ~/.claude reached again through the process list and a symlink.
    symlink(t.home.join(".claude"), t.home.join("alias")).unwrap();
    pr.dirs = vec![t.home.join(".claude"), t.home.join("alias")];
    build(&t.home, &t.root, &pr).unwrap();
    let asked = pr.asked.borrow();
    assert_eq!(asked.iter().filter(|&&p| p == 400).count(), 1, "{asked:?}");
}

#[test]
fn dead_pids_protect_nothing() {
    let t = setup();
    pid_file(&t.home.join(".claude"), "500", &body("s", "500"));
    let set = build(&t.home, &t.root, &probe(&[])).unwrap();
    assert!(!set.contains("s"));
}

#[test]
fn either_the_stem_or_the_body_pid_alive_is_enough() {
    let t = setup();
    let c = t.home.join(".claude");
    pid_file(&c, "601", &body("stem-alive", "602"));
    pid_file(&c, "603", &body("body-alive", "604"));
    let set = build(&t.home, &t.root, &probe(&[601, 604])).unwrap();
    assert!(set.contains("stem-alive"));
    assert!(set.contains("body-alive"));
}

#[test]
fn out_of_range_and_non_integer_pids_are_never_probed() {
    let t = setup();
    let c = t.home.join(".claude");
    let cases = [
        ("a", "0"),
        ("b", "-1"),
        ("c", "-42"),
        ("d", "2147483648"),
        ("e", "1.5"),
        ("f", "\"77\""),
        ("g", "null"),
        ("h", "1e3"),
        ("i", "99999999999999999999999"),
    ];
    for (i, (id, pid)) in cases.iter().enumerate() {
        pid_file(&c, &format!("x{i}"), &body(id, pid));
    }
    // Stems that are not plain ASCII digits in range.
    for stem in ["+1", "-1", "0", "2147483648", "１２", " 7"] {
        pid_file(&c, stem, &body("stem-case", "0"));
    }
    let pr = probe(&[0, -1, -42, 1, 7, 12, 77, 1000]);
    let set = build(&t.home, &t.root, &pr).unwrap();
    for (id, _) in cases {
        assert!(!set.contains(id), "{id}");
    }
    assert!(!set.contains("stem-case"));
    for p in pr.asked.borrow().iter() {
        assert!(*p >= 1, "probed pid {p}");
    }
}

#[test]
fn i32_max_is_in_range() {
    let t = setup();
    pid_file(&t.home.join(".claude"), "x", &body("max", "2147483647"));
    let set = build(&t.home, &t.root, &probe(&[i32::MAX])).unwrap();
    assert!(set.contains("max"));
}

#[test]
fn unparseable_oversized_symlinked_or_non_json_files_protect_nothing() {
    let t = setup();
    let c = t.home.join(".claude");
    let s = c.join("sessions");
    pid_file(&c, "700", "not json at all");
    // 700 is alive but the body is garbage: the stem alone does not name a
    // session, so nothing is live.
    let big = format!(
        r#"{{"pid":701,"sessionId":"big","pad":"{}"}}"#,
        "x".repeat(65 * 1024)
    );
    fs::write(s.join("big.json"), big).unwrap();
    fs::write(s.join("702.txt"), body("txt", "702")).unwrap();
    fs::write(t.home.join("real.json"), body("linked", "703")).unwrap();
    symlink(t.home.join("real.json"), s.join("703.json")).unwrap();
    fs::create_dir_all(s.join("704.json")).unwrap();
    let set = build(&t.home, &t.root, &probe(&[700, 701, 702, 703, 704])).unwrap();
    for id in ["big", "txt", "linked"] {
        assert!(!set.contains(id), "{id}");
    }
}

#[test]
fn invalid_session_ids_are_ignored() {
    let t = setup();
    let c = t.home.join(".claude");
    pid_file(&c, "800", &body("../escape", "800"));
    pid_file(&c, "801", &body("", "801"));
    let set = build(&t.home, &t.root, &probe(&[800, 801])).unwrap();
    assert!(!set.contains("../escape"));
    assert!(!set.contains(""));
}

#[test]
fn unknown_fields_are_ignored() {
    let t = setup();
    pid_file(
        &t.home.join(".claude"),
        "900",
        r#"{"sessionId":"s","pid":900,"extra":{"nested":[1,2]},"z":null}"#,
    );
    assert!(
        build(&t.home, &t.root, &probe(&[900]))
            .unwrap()
            .contains("s")
    );
}

fn as_root() -> bool {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() == 0 }
}

#[test]
fn unreadable_profile_sessions_dir_fails_closed() {
    if as_root() {
        return;
    }
    let t = setup();
    let dir = t.root.join("work");
    pid_file(&dir, "1000", &body("s", "1000"));
    let s = dir.join("sessions");
    fs::set_permissions(&s, fs::Permissions::from_mode(0o000)).unwrap();
    let got = build(&t.home, &t.root, &probe(&[1000]));
    fs::set_permissions(&s, fs::Permissions::from_mode(0o700)).unwrap();
    let (path, kind) = got.expect_err("must fail closed");
    assert_eq!(path, s);
    assert_eq!(kind, std::io::ErrorKind::PermissionDenied);
}

#[test]
fn unreadable_profile_root_fails_closed() {
    if as_root() {
        return;
    }
    let t = setup();
    fs::create_dir_all(t.root.join("work")).unwrap();
    fs::set_permissions(&t.root, fs::Permissions::from_mode(0o000)).unwrap();
    let got = build(&t.home, &t.root, &probe(&[]));
    fs::set_permissions(&t.root, fs::Permissions::from_mode(0o700)).unwrap();
    let (_, kind) = got.expect_err("must fail closed");
    assert_eq!(kind, std::io::ErrorKind::PermissionDenied);
}

#[test]
fn sessions_path_that_is_a_file_fails_closed() {
    let t = setup();
    fs::write(t.home.join(".claude/sessions"), b"x").unwrap();
    assert!(build(&t.home, &t.root, &probe(&[])).is_err());
}

// --------------------------------------------- liveness::claude_config_dirs

fn fake_proc(proc_root: &Path, pid: u32, uid: u32, environ: &[u8]) {
    let d = proc_root.join(pid.to_string());
    fs::create_dir_all(&d).unwrap();
    fs::write(
        d.join("status"),
        format!("Name:\tclaude\nState:\tS (sleeping)\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n"),
    )
    .unwrap();
    fs::write(d.join("environ"), environ).unwrap();
}

#[test]
fn claude_config_dirs_reads_same_uid_processes_only() {
    use crate::commands::profile::claude_config_dirs;
    let t = tempfile::tempdir().unwrap();
    let p = t.path();
    fs::create_dir_all(p.join("self")).unwrap();
    fs::write(p.join("self/environ"), b"A=1\0").unwrap();
    fake_proc(p, 10, 1000, b"X=SECRET\0CLAUDE_CONFIG_DIR=/cfg/a\0");
    fake_proc(p, 11, 2000, b"CLAUDE_CONFIG_DIR=/cfg/other-user\0");
    fake_proc(p, 12, 1000, b"PATH=/bin\0");
    fake_proc(p, 13, 1000, b"CLAUDE_CONFIG_DIR=/cfg/self\0");
    // Unreadable status: skipped silently.
    fs::create_dir_all(p.join("14")).unwrap();
    let mut got = claude_config_dirs(p, 13, 1000).unwrap();
    got.sort();
    assert_eq!(got, [PathBuf::from("/cfg/a")]);
}

#[test]
fn unparseable_pid_files_are_recorded_once_each() {
    let t = setup();
    let c = t.home.join(".claude");
    let s = c.join("sessions");
    pid_file(&c, "900", "not json at all");
    pid_file(&c, "901", r#"{"pid":901}"#);
    pid_file(&c, "902", &body("dead", "902"));
    fs::write(t.home.join("real.json"), "garbage").unwrap();
    symlink(t.home.join("real.json"), s.join("903.json")).unwrap();
    let set = build(&t.home, &t.root, &probe(&[])).unwrap();
    let want: BTreeSet<PathBuf> = [s.join("900.json"), s.join("901.json")].into();
    assert_eq!(set.ignored, want);
    let mut lines = Vec::new();
    set.report_ignored(&mut |l| lines.push(l.to_string()));
    assert_eq!(
        lines,
        [
            format!(
                "ignored unreadable pid file {}",
                s.join("900.json").display()
            ),
            format!(
                "ignored unreadable pid file {}",
                s.join("901.json").display()
            ),
        ]
    );
}

#[test]
fn dead_or_symlinked_pid_files_are_not_recorded() {
    let t = setup();
    let c = t.home.join(".claude");
    pid_file(&c, "902", &body("dead", "902"));
    fs::write(t.home.join("real.json"), "garbage").unwrap();
    symlink(t.home.join("real.json"), c.join("sessions/903.json")).unwrap();
    let set = build(&t.home, &t.root, &probe(&[])).unwrap();
    assert!(set.ignored.is_empty(), "{:?}", set.ignored);
}

#[test]
fn unreadable_config_dirs_fail_the_build_closed() {
    let t = setup();
    let p = FakeProbe {
        dirs_error: Some(io::ErrorKind::PermissionDenied),
        ..Default::default()
    };
    let err = build(&t.home, &t.root, &p).unwrap_err();
    assert_eq!(
        err,
        (
            PathBuf::from("/proc"),
            io::ErrorKind::PermissionDenied.into()
        )
    );
}

#[test]
fn claude_config_dirs_fails_closed() {
    use crate::commands::profile::claude_config_dirs;
    let t = tempfile::tempdir().unwrap();
    let p = t.path();
    assert!(claude_config_dirs(&p.join("missing"), 1, 1000).is_err());
    // A non-Claude process whose environment cannot be read is skipped.
    let d = p.join("20");
    fs::create_dir_all(&d).unwrap();
    fs::write(
        d.join("status"),
        "State:\tS\nUid:\t1000\t1000\t1000\t1000\n",
    )
    .unwrap();
    fs::write(d.join("cmdline"), b"ssh-agent\0").unwrap();
    fs::create_dir(d.join("environ")).unwrap(); // reading a directory fails
    assert_eq!(
        claude_config_dirs(p, 1, 1000).unwrap(),
        Vec::<PathBuf>::new()
    );
    // A Claude process whose environment cannot be read is an error.
    fs::write(d.join("cmdline"), b"/usr/bin/claude\0--resume\0").unwrap();
    assert!(claude_config_dirs(p, 1, 1000).is_err());
}

// ------------------------------------------------ registered agent sources

/// `<home>/.amplihack/agents/registry.json` holding one agent per
/// `(name, state, conversation)`.
fn agents(home: &Path, list: &[(&str, &str, &str)]) -> PathBuf {
    let top = home.join(".amplihack");
    let dir = top.join("agents");
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&top, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let mut map = serde_json::Map::new();
    for (name, state, id) in list {
        map.insert(
            (*name).to_string(),
            serde_json::json!({
                "state": state, "tmux_session": "s", "tmux_window": "w",
                "launch_dir": "/srv/p", "conversation_id": id,
                "registered_at": "2026-10-30T09:12:04Z",
                "launched_at": "2026-10-30T09:12:04Z",
                "launch_id": "6f1c0d8e2a4b4f0e9c7d3b5a1e2f4c6d", "restarts": []
            }),
        );
    }
    let path = dir.join("registry.json");
    fs::write(
        &path,
        serde_json::json!({"version": 1, "agents": map}).to_string(),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    path
}

#[test]
fn unended_registered_agents_are_live_without_a_pid_file() {
    let t = setup();
    agents(
        &t.home,
        &[
            ("a", "active", "conv-a"),
            ("b", "stopped", "conv-b"),
            ("c", "ended", "conv-c"),
        ],
    );
    let set = build(&t.home, &t.root, &probe(&[])).unwrap();
    assert!(set.contains("conv-a"));
    assert!(set.contains("conv-b"));
    assert!(!set.contains("conv-c"));
}

#[test]
fn a_registry_that_cannot_be_loaded_fails_closed() {
    let t = setup();
    let path = agents(&t.home, &[("a", "stopped", "conv-a")]);
    fs::write(&path, b"{").unwrap();
    let (got, _) = build(&t.home, &t.root, &probe(&[])).expect_err("must fail closed");
    assert_eq!(got, path);
}
