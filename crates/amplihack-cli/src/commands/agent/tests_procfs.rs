//! Unit tests for `procfs.rs`: parsers on hostile input (nothing panics, a
//! malformed entry gives `None`), what counts as a `claude` process (A5,
//! R4.3), the snapshot filter (S2), tree walks, and the test-seam guard
//! (S27). Every tree is synthetic, built in a temp dir and passed as `root`.
//!
//! API under test:
//! - `parse_stat(&[u8]) -> Option<Stat>`, `Stat { comm: String, state: char,
//!   ppid: u32, starttime: u64 }` (fields after the last `)`; field 22)
//! - `parse_status(&[u8]) -> Option<Status>`, `Status { state: char, ruid:
//!   u32, euid: u32 }`; `split_nul(&[u8]) -> Vec<String>`
//! - `ClaudeNames { binaries: &'static [&'static str], node_scripts:
//!   &'static [&'static str] }`, `CLAUDE: ClaudeNames`,
//!   `is_claude(comm: &str, argv: &[String]) -> bool`,
//!   `is_claude_with(names: &ClaudeNames, comm: &str, argv: &[String]) -> bool`
//! - `snapshot(root: &Path, uid: u32) -> Result<Snapshot, String>`;
//!   `Snapshot { self_pid: u32, procs: map u32 -> Proc, claude: Vec<u32>, .. }`,
//!   `Proc { pid, ppid, state, starttime, comm, argv: Result<Vec<String>,
//!   String> }`; `Snapshot::{alive(pid, start) -> bool, ancestors(pid) ->
//!   Vec<u32>, descendants(pid) -> Vec<u32>}` (both exclude `pid`)
//! - `read_environ(root: &Path, pid: u32) -> Result<Vec<(String, String)>,
//!   String>`, `cwd(root: &Path, pid: u32) -> Option<PathBuf>`
//! - `proc_root_from(var: Option<&OsStr>, guard: Option<&OsStr>, temp_dir:
//!   &Path) -> PathBuf`: the pure part of `proc_root()`, which passes
//!   `AMPLIHACK_TEST_PROC_ROOT`, `AMPLIHACK_TEST_HOME_GUARD` and
//!   `std::env::temp_dir()`

use super::procfs::*;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const REST: &str = "S 4001 5000 5000 0 -1 4194560 0 0 0 0 0 0 0 0 20 0 1 0 777 1000000 100";

fn stat(comm: &str, rest: &str) -> Vec<u8> {
    format!("5000 ({comm}) {rest}\n").into_bytes()
}

fn v(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

#[test]
fn stat_uses_the_last_paren_so_comm_cannot_fake_fields() {
    let s = parse_stat(&stat("claude", REST)).expect("valid stat");
    assert_eq!(
        (s.comm, s.state, s.ppid, s.starttime),
        ("claude".into(), 'S', 4001, 777)
    );
    let s = parse_stat(&stat("a) Z 1 1 1 (b", REST)).expect("valid stat");
    assert_eq!(s.comm, "a) Z 1 1 1 (b");
    assert_eq!((s.state, s.ppid, s.starttime), ('S', 4001, 777));
    let mut raw = b"5000 (cl\xffde) ".to_vec();
    raw.extend_from_slice(REST.as_bytes());
    assert_eq!(parse_stat(&raw).expect("non-UTF-8 comm").starttime, 777);
}

#[test]
fn malformed_stat_lines_give_none() {
    let huge = REST.replace(" 777 ", " 99999999999999999999999 ");
    for raw in [
        Vec::new(),
        b"5000 claude S 1".to_vec(),
        stat("claude", "S 4001 5000"),
        stat("claude", ""),
        stat("claude", &huge),
        stat("claude", &REST.replace("4001", "-1")),
        b"\0\0\0".to_vec(),
    ] {
        let text = String::from_utf8_lossy(&raw);
        assert!(parse_stat(&raw).is_none(), "{text:?}");
    }
}

#[test]
fn status_reads_state_and_both_uids() {
    let raw = b"Name:\tclaude\nState:\tS (sleeping)\nUid:\t1000\t1001\t1000\t1000\n";
    let s = parse_status(raw).expect("valid status");
    assert_eq!((s.state, s.ruid, s.euid), ('S', 1000, 1001));
    assert_eq!(
        parse_status(b"State:\tZ (zombie)\nUid:\t1\t1\t1\t1\n")
            .unwrap()
            .state,
        'Z'
    );
    for raw in [
        &b""[..],
        b"State:\tS (sleeping)\n",
        b"Uid:\t1000\t1000\t1000\t1000\n",
        b"State:\tS\nUid:\tx\t1000\n",
        b"State:\tS\nUid:\t1000\n",
        b"State:\t\nUid:\t1\t1\t1\t1\n",
        b"\xff\xfe",
    ] {
        assert!(
            parse_status(raw).is_none(),
            "{:?}",
            String::from_utf8_lossy(raw)
        );
    }
}

#[test]
fn nul_separated_lists() {
    assert_eq!(
        split_nul(b"claude\0--model\0m\0"),
        ["claude", "--model", "m"]
    );
    assert_eq!(split_nul(b"a\0\0b"), ["a", "", "b"]);
    assert!(split_nul(b"").is_empty());
    assert_eq!(split_nul(b"x\xff\0").len(), 1);
}

#[test]
fn what_counts_as_claude() {
    let cli = "/usr/lib/node_modules/@anthropic-ai/claude-code/cli.js";
    for (comm, argv) in [
        ("claude", v(&["claude", "--resume", "x"])),
        ("MainThread", v(&["/home/u/.local/bin/claude"])),
        ("node", v(&["/usr/bin/node", cli])),
        ("claude", Vec::new()),
    ] {
        assert!(is_claude(comm, &argv), "{comm} {argv:?}");
    }
    for (comm, argv) in [
        ("amplihack", v(&["amplihack", "claude"])),
        ("bash", v(&["bash", "-c", "claude"])),
        ("claude-helper", v(&["claude-helper"])),
        ("node", v(&["node", "/srv/app/cli.js"])),
        ("node", v(&["node", "/srv/claude-code-fork/server.js"])),
        ("node", v(&["node"])),
        ("", Vec::new()),
    ] {
        assert!(!is_claude(comm, &argv), "{comm} {argv:?}");
    }
    let n = ClaudeNames {
        binaries: &["fake-claude"],
        node_scripts: &["x/cli.js"],
    };
    let node = v(&["node", "/srv/x/cli.js"]);
    assert!(is_claude_with(&n, "fake-claude", &[]) && is_claude_with(&n, "node", &node));
    assert!(
        !is_claude_with(&n, "claude", &v(&["claude"])),
        "names are injectable"
    );
}

/// Adds `/proc/<pid>` under `root`. `start` is `pid * 10`.
fn add(root: &Path, pid: u32, ppid: u32, argv: &[&str], state: char, uids: (u32, u32)) {
    let d = root.join(pid.to_string());
    fs::create_dir_all(&d).unwrap();
    let comm = argv.first().map_or("", |a| a.rsplit('/').next().unwrap());
    let line = format!(
        "{pid} ({comm}) {state} {ppid} 1 1 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 {} 1 1\n",
        pid * 10
    );
    fs::write(d.join("stat"), line).unwrap();
    let (r, e) = uids;
    let status = format!("Name:\t{comm}\nState:\t{state} (x)\nUid:\t{r}\t{e}\t{e}\t{e}\n");
    fs::write(d.join("status"), status).unwrap();
    let cmdline: String = argv.iter().map(|a| format!("{a}\0")).collect();
    fs::write(d.join("cmdline"), cmdline).unwrap();
    fs::write(d.join("environ"), "ANTHROPIC_MODEL=opus\0PATH=/bin\0").unwrap();
    std::os::unix::fs::symlink("/srv/project", d.join("cwd")).unwrap();
}

/// `self` -> 100 (amplihack) -> 90 (claude) -> 80 (amplihack claude) -> 1;
/// 95 is a second child of 80. 200 to 204 must all be left out.
fn tree() -> (tempfile::TempDir, PathBuf, u32) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let me = amplihack_utils::secure_files::euid();
    let other = me + 1;
    add(&root, 80, 1, &["amplihack", "claude", "--"], 'S', (me, me));
    add(&root, 90, 80, &["claude", "--model", "opus"], 'S', (me, me));
    add(&root, 95, 80, &["sleep", "9"], 'S', (me, me));
    add(
        &root,
        100,
        90,
        &["amplihack", "agent", "register"],
        'R',
        (me, me),
    );
    add(&root, 200, 1, &["claude"], 'S', (other, other));
    add(&root, 201, 1, &["claude"], 'S', (me, 0));
    add(&root, 202, 1, &["claude"], 'S', (0, me));
    add(&root, 203, 90, &["claude"], 'Z', (me, me));
    fs::create_dir(root.join("204")).unwrap(); // vanished: no stat or status
    add(&root, 300, 1, &["claude", "--resume", "x"], 'S', (me, me));
    fs::remove_file(root.join("300/cmdline")).unwrap();
    fs::create_dir(root.join("300/cmdline")).unwrap(); // unreadable
    fs::create_dir(root.join("sys")).unwrap();
    std::os::unix::fs::symlink("100", root.join("self")).unwrap();
    (tmp, root, me)
}

#[test]
fn snapshot_keeps_only_this_users_live_processes() {
    let (_t, root, me) = tree();
    let snap = snapshot(&root, me).expect("snapshot");
    assert_eq!(snap.self_pid, 100);
    for pid in [80, 90, 95, 100, 300] {
        assert!(snap.procs.contains_key(&pid), "{pid} missing");
    }
    for pid in [200, 201, 202, 203, 204] {
        assert!(
            !snap.procs.contains_key(&pid),
            "{pid} kept (S2, zombie or vanished)"
        );
    }
    assert!(
        snap.procs[&300].argv.is_err(),
        "an unreadable cmdline is an error, not empty"
    );
    assert_eq!(
        snap.procs[&90].argv.as_ref().unwrap(),
        &v(&["claude", "--model", "opus"])
    );
    assert!(snap.claude.contains(&90) && snap.claude.contains(&300));
    assert!(!snap.claude.contains(&80));
    assert!(snap.alive(90, 900) && !snap.alive(90, 901) && !snap.alive(200, 2000));
}

#[test]
fn tree_walks() {
    let (_t, root, me) = tree();
    let snap = snapshot(&root, me).unwrap();
    assert_eq!(snap.ancestors(100)[..2], [90, 80]);
    let d = snap.descendants(80);
    let at = |p: u32| {
        d.iter()
            .position(|&x| x == p)
            .unwrap_or_else(|| panic!("{p} in {d:?}"))
    };
    assert!(at(90) < at(100) && at(95) < at(100), "breadth first: {d:?}");
    assert!(!d.contains(&80) && !d.contains(&203));
    add(&root, 10, 11, &["a"], 'S', (me, me));
    add(&root, 11, 10, &["b"], 'S', (me, me));
    let snap = snapshot(&root, me).unwrap();
    assert!(snap.ancestors(10).len() <= 2, "a cycle ends the walk");
}

#[test]
fn snapshot_fails_without_a_listable_root_or_self() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(snapshot(&tmp.path().join("missing"), 1).is_err());
    add(tmp.path(), 5, 1, &["claude"], 'S', (1, 1));
    assert!(snapshot(tmp.path(), 1).is_err(), "no self/stat");
}

#[test]
fn environ_and_cwd() {
    let (_t, root, _) = tree();
    let env = read_environ(&root, 80).unwrap();
    assert!(env.contains(&("ANTHROPIC_MODEL".to_string(), "opus".to_string())));
    assert!(read_environ(&root, 4242).is_err());
    assert_eq!(cwd(&root, 80), Some(PathBuf::from("/srv/project")));
    assert_eq!(cwd(&root, 4242), None);
}

#[test]
fn the_proc_root_seam_needs_the_guard_a_temp_path_and_a_private_root() {
    let tmp = tempfile::tempdir().unwrap();
    let temp = tmp.path().canonicalize().unwrap();
    let inside = temp.join("proc");
    fs::create_dir(&inside).unwrap();
    fs::set_permissions(&inside, fs::Permissions::from_mode(0o700)).unwrap();
    let real = PathBuf::from("/proc");
    let g = Some(OsStr::new("1"));
    let seam = |var: &Path, guard, temp: &Path| proc_root_from(Some(var.as_os_str()), guard, temp);
    assert_eq!(seam(&inside, g, &temp), inside);
    assert_eq!(seam(&inside, None, &temp), real, "no guard");
    assert_eq!(
        seam(&inside, Some(OsStr::new("")), &temp),
        real,
        "empty guard"
    );
    assert_eq!(
        seam(&inside, g, &temp.join("elsewhere")),
        real,
        "outside temp_dir"
    );
    assert_eq!(seam(&temp.join("missing"), g, &temp), real);
    assert_eq!(proc_root_from(None, g, &temp), real);
    let escape = temp.join("link");
    std::os::unix::fs::symlink("/", &escape).unwrap();
    assert_eq!(
        seam(&escape, g, &temp),
        real,
        "the canonical path is checked"
    );
    fs::set_permissions(&inside, fs::Permissions::from_mode(0o777)).unwrap();
    assert_eq!(
        seam(&inside, g, &temp),
        real,
        "S27: a root others can write"
    );
}
