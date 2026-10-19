//! Liveness from a fake `/proc`: which profile directories a running process
//! is using through `CLAUDE_CONFIG_DIR` (issue #1569). Any doubt counts as live.

use super::liveness::{self, Liveness};
use crate::commands::profile::name::ProfileName;
use std::fs;
use std::path::{Path, PathBuf};

const SELF_PID: u32 = 4242;
const UID: u32 = 1000;

struct FakeProc {
    _tmp: tempfile::TempDir,
    proc_root: PathBuf,
    root: PathBuf,
}

impl FakeProc {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let proc_root = tmp.path().join("proc");
        let root = tmp.path().join("profiles");
        for name in ["X", "Y", "Z"] {
            fs::create_dir_all(root.join(name)).unwrap();
        }
        fs::create_dir_all(proc_root.join("self")).unwrap();
        fs::write(proc_root.join("self/environ"), b"HOME=/h\0").unwrap();
        let root = root.canonicalize().unwrap();
        Self {
            _tmp: tmp,
            proc_root,
            root,
        }
    }

    fn process(&self, pid: u32, uid: u32, state: &str, env: &[&[u8]], cwd: &Path) -> PathBuf {
        let dir = self.proc_root.join(pid.to_string());
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("status"),
            format!("Name:\tclaude\nState:\t{state}\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n"),
        )
        .unwrap();
        let mut environ = Vec::new();
        for e in env {
            environ.extend_from_slice(e);
            environ.push(0);
        }
        fs::write(dir.join("environ"), environ).unwrap();
        fs::write(dir.join("cmdline"), b"claude\0-p\0x\0").unwrap();
        std::os::unix::fs::symlink(cwd, dir.join("cwd")).unwrap();
        dir
    }

    fn config_dir(&self, name: &str) -> Vec<u8> {
        format!("CLAUDE_CONFIG_DIR={}", self.root.join(name).display()).into_bytes()
    }

    fn scan(&self) -> Liveness {
        liveness::scan(&self.proc_root, &self.root, SELF_PID, UID)
    }
}

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

#[test]
fn a_process_with_the_config_dir_makes_it_live() {
    let p = FakeProc::new();
    p.process(
        100,
        UID,
        "S (sleeping)",
        &[b"A=1", &p.config_dir("X")],
        &p.root,
    );
    let l = p.scan();
    assert!(matches!(l, Liveness::Known(_)));
    assert!(l.is_live(&n("X")));
    assert!(!l.is_live(&n("Y")));
}

#[test]
fn self_zombies_and_other_users_are_skipped() {
    let p = FakeProc::new();
    p.process(SELF_PID, UID, "R (running)", &[&p.config_dir("X")], &p.root);
    p.process(101, UID, "Z (zombie)", &[&p.config_dir("Y")], &p.root);
    p.process(102, UID + 1, "S (sleeping)", &[&p.config_dir("Z")], &p.root);
    let l = p.scan();
    for name in ["X", "Y", "Z"] {
        assert!(!l.is_live(&n(name)), "{name}");
    }
}

#[test]
fn a_huge_environment_is_read_to_the_end() {
    let p = FakeProc::new();
    let big = format!("BIG={}", "v".repeat(300 * 1024));
    p.process(
        103,
        UID,
        "S (sleeping)",
        &[big.as_bytes(), &p.config_dir("Y")],
        &p.root,
    );
    assert!(p.scan().is_live(&n("Y")));
}

#[test]
fn a_relative_config_dir_is_resolved_against_the_process_cwd() {
    let p = FakeProc::new();
    p.process(
        104,
        UID,
        "S (sleeping)",
        &[b"CLAUDE_CONFIG_DIR=./Z"],
        &p.root,
    );
    assert!(p.scan().is_live(&n("Z")));
}

#[test]
fn a_same_named_directory_under_another_root_is_not_live() {
    let p = FakeProc::new();
    let other = p._tmp.path().join("other/X");
    fs::create_dir_all(&other).unwrap();
    let env = format!("CLAUDE_CONFIG_DIR={}", other.display());
    p.process(105, UID, "S (sleeping)", &[env.as_bytes()], &p.root);
    assert!(!p.scan().is_live(&n("X")));
}

#[test]
fn an_unresolvable_path_falls_back_to_its_last_component() {
    let p = FakeProc::new();
    p.process(
        106,
        UID,
        "S (sleeping)",
        &[b"CLAUDE_CONFIG_DIR=/gone/away/Y"],
        &p.root,
    );
    assert!(p.scan().is_live(&n("Y")), "doubt counts as live");
}

#[test]
fn an_unreadable_claude_environment_makes_every_directory_live() {
    let p = FakeProc::new();
    let dir = p.process(107, UID, "S (sleeping)", &[], &p.root);
    fs::remove_file(dir.join("environ")).unwrap();
    fs::create_dir(dir.join("environ")).unwrap(); // read fails
    let l = p.scan();
    assert!(l.is_live(&n("X")) && l.is_live(&n("Y")) && l.is_live(&n("Z")));
}

#[test]
fn an_unreadable_unrelated_environment_is_ignored() {
    let p = FakeProc::new();
    let dir = p.process(108, UID, "S (sleeping)", &[], &p.root);
    fs::write(dir.join("cmdline"), b"vim\0notes.txt\0").unwrap();
    fs::remove_file(dir.join("environ")).unwrap();
    fs::create_dir(dir.join("environ")).unwrap();
    assert!(!p.scan().is_live(&n("X")));
}

#[test]
fn non_numeric_entries_are_ignored() {
    let p = FakeProc::new();
    fs::create_dir_all(p.proc_root.join("sys")).unwrap();
    fs::write(p.proc_root.join("uptime"), "1 2").unwrap();
    assert!(!p.scan().is_live(&n("X")));
}

#[test]
fn failed_capability_probe_is_unsupported_and_everything_counts_as_live() {
    let p = FakeProc::new();
    fs::remove_file(p.proc_root.join("self/environ")).unwrap();
    let l = p.scan();
    assert!(matches!(l, Liveness::Unsupported));
    assert!(l.is_live(&n("X")), "unsupported must fail closed");
}

#[test]
fn missing_proc_is_unsupported() {
    let tmp = tempfile::tempdir().unwrap();
    let l = liveness::scan(&tmp.path().join("no-proc"), tmp.path(), SELF_PID, UID);
    assert!(matches!(l, Liveness::Unsupported));
}

#[test]
fn liveness_unresolvable_dir_counts_as_live() {
    // fail-safe: a value that cannot be resolved still names its directory,
    // so a session there blocks a swap rather than being missed.
    let p = FakeProc::new();
    let gone = p.root.join("missing-parent").join("X");
    p.process(
        102,
        UID,
        "S (sleeping)",
        &[format!("CLAUDE_CONFIG_DIR={}", gone.display()).as_bytes()],
        &p.root,
    );
    assert!(p.scan().is_live(&n("X")));
}

#[test]
fn liveness_never_prints_environ() {
    // Environments hold secrets: the scanner returns names and prints nothing.
    let src = include_str!("liveness.rs");
    for banned in ["print!", "println!", "eprint!", "eprintln!", "dbg!", "{:?}"] {
        assert!(!src.contains(banned), "liveness.rs must not use {banned}");
    }
}
