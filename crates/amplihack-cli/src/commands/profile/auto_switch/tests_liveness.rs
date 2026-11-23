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

/// Issue #2820. One unreadable process must not make every profile live.
/// The old test asserted that it did; the protection is now decided per
/// profile (see `tests_unreadable.rs`), so this is a replaced contract and
/// not a weakened check.
#[test]
fn one_unreadable_process_leaves_the_other_profiles_readable() {
    let p = FakeProc::new();
    p.process(100, UID, "S (sleeping)", &[&p.config_dir("X")], &p.root);
    let dir = p.process(107, UID, "S (sleeping)", &[], &p.root);
    fs::write(
        dir.join("cmdline"),
        format!("claude\0--settings\0{}\0", p.root.join("Y").display()),
    )
    .unwrap();
    fs::remove_file(dir.join("environ")).unwrap();
    fs::create_dir(dir.join("environ")).unwrap(); // read fails
    let l = p.scan();
    assert!(matches!(l, Liveness::Partial { .. }));
    assert!(l.is_live(&n("X")), "readable process names X");
    assert!(
        l.is_live(&n("Y")),
        "unreadable claude names Y: doubtful is live"
    );
    assert!(!l.is_live(&n("Z")), "Z is named by nobody");
    assert_eq!(l.unreadable().len(), 1);
    assert_eq!(l.unreadable()[0].pid, 107);
}

/// A non-dumpable `tailscaled` whose command mentions amplihack is logged
/// but is no evidence for any profile.
#[test]
fn an_unreadable_non_claude_process_mentioning_amplihack_is_not_evidence() {
    let p = FakeProc::new();
    let dir = p.process(108, UID, "S (sleeping)", &[], &p.root);
    fs::write(
        dir.join("status"),
        format!("Name:\ttailscaled\nState:\tS\nUid:\t{UID}\t{UID}\t{UID}\t{UID}\n"),
    )
    .unwrap();
    fs::write(
        dir.join("cmdline"),
        b"tailscaled\0be-child\0ssh\0--cmd\0amplihack wait\0",
    )
    .unwrap();
    fs::remove_file(dir.join("environ")).unwrap();
    fs::create_dir(dir.join("environ")).unwrap();
    let l = p.scan();
    for name in ["X", "Y", "Z"] {
        assert!(!l.is_live(&n(name)), "{name}");
    }
    assert!(l.supported());
    assert_eq!(l.unreadable().len(), 1, "logged so an operator can see it");
}

/// Unreadable processes come out in pid order whatever the directory order.
#[test]
fn unreadable_processes_are_sorted_by_pid() {
    let p = FakeProc::new();
    for pid in [900, 200, 500] {
        let dir = p.process(pid, UID, "S (sleeping)", &[], &p.root);
        fs::remove_file(dir.join("environ")).unwrap();
        fs::create_dir(dir.join("environ")).unwrap();
    }
    let l = p.scan();
    let pids: Vec<u32> = l.unreadable().iter().map(|u| u.pid).collect();
    assert_eq!(pids, vec![200, 500, 900]);
}

/// Nothing unreadable: the plain `Known` shape, no unreadable notes.
#[test]
fn nothing_unreadable_stays_known() {
    let p = FakeProc::new();
    p.process(100, UID, "S (sleeping)", &[&p.config_dir("X")], &p.root);
    let l = p.scan();
    assert!(matches!(l, Liveness::Known(_)));
    assert!(l.unreadable().is_empty());
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

/// Issue #2935, the second macOS refusal. A system without `/proc` gives the
/// host's view no process table, and its answer is then that sessions
/// cannot be seen: every profile counts as live, so nothing is moved. With a
/// table, the same root gives the sessions the table shows.
#[test]
fn without_a_process_table_sessions_cannot_be_seen() {
    let p = FakeProc::new();
    p.process(
        100,
        UID,
        "S (sleeping)",
        &[&p.config_dir("X")],
        Path::new("/"),
    );
    let none = liveness::seen_in(None, &p.root, SELF_PID, UID);
    assert_eq!(none, Liveness::Unsupported);
    assert!(!none.supported());
    assert!(none.is_live(&n("X")) && none.is_live(&n("Y")));

    let some = liveness::seen_in(Some(&p.proc_root), &p.root, SELF_PID, UID);
    assert!(some.supported());
    assert!(some.is_live(&n("X")) && !some.is_live(&n("Y")), "{some:?}");
}

/// The in-memory view answers what it was built with, whatever the root, and
/// a plain function is a view too (`Ctx::scan` holds one).
#[test]
fn a_fake_view_and_a_function_answer_as_they_were_built() {
    use liveness::{FakeSessions, SessionView};
    let anywhere = Path::new("/nowhere");
    let running = FakeSessions::running(&["X"]).liveness(anywhere);
    assert!(running.is_live(&n("X")) && !running.is_live(&n("Y")));
    assert_eq!(
        FakeSessions::unseen().liveness(anywhere),
        Liveness::Unsupported
    );
    let function: fn(&Path) -> Liveness = |_| Liveness::Unsupported;
    assert_eq!(function.liveness(anywhere), Liveness::Unsupported);
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
