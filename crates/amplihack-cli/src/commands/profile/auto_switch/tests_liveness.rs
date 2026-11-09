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

// --- Issue #2040: the scan split into collect and derive, and the D9 gate ----
//
// `collect(proc_root, canonical_root, euid)` records every same-uid process,
// the caller included; `derive(&collected, reader_pid)` drops the reader and
// gives the same `Liveness` that `scan` gave. For each process the scan reads
// `status`, then the `exe` link, then `cmdline` only for
// `node`, `bun` or `deno`, then `environ` only for a candidate. The fixtures
// above have no `exe` link, and must keep their meaning (AC7): an unreadable
// `exe` is a candidate. `stat` is never read.
// An unreadable `environ` or `cmdline` is simulated with a directory in its
// place, so these tests also hold when run as root.

use super::liveness::{Collected, ProcRecord};
use std::cell::Cell;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;

const NATIVE: &str = "/home/u/.local/share/claude/versions/2.1.3";

/// `/proc/<pid>/stat` with `comm` and the start time (field 22).
fn stat_line(pid: u32, comm: &str, start: u64) -> String {
    format!("{pid} ({comm}) S 1 {pid} {pid} 0 -1 4194304 0 0 0 0 0 0 0 0 20 0 1 0 {start} 0 0")
}

fn set_exe(dir: &Path, exe: &[u8]) {
    std::os::unix::fs::symlink(OsStr::from_bytes(exe), dir.join("exe")).unwrap();
}

fn unreadable(path: &Path) {
    let _ = fs::remove_file(path);
    fs::create_dir(path).unwrap();
}

impl FakeProc {
    /// A process of `UID` on profile `X`, with the given `exe` and `cmdline`.
    fn on_x(&self, pid: u32, exe: Option<&[u8]>, cmdline: &[u8]) -> PathBuf {
        let env = self.config_dir("X");
        let dir = self.process(pid, UID, "S (sleeping)", &[&env], &self.root);
        fs::write(dir.join("stat"), stat_line(pid, "claude", 801_122)).unwrap();
        fs::write(dir.join("cmdline"), cmdline).unwrap();
        if let Some(exe) = exe {
            set_exe(&dir, exe);
        }
        dir
    }

    fn collect(&self) -> Collected {
        liveness::collect(&self.proc_root, &self.root, UID)
    }

    fn record(&self, pid: u32) -> Option<ProcRecord> {
        match self.collect() {
            Collected::Procs(procs) => procs.into_iter().find(|r| r.pid == pid),
            Collected::Unsupported => panic!("the fake /proc is usable"),
        }
    }
}

#[test]
fn a_native_install_with_a_version_basename_is_a_candidate() {
    let p = FakeProc::new();
    p.on_x(200, Some(NATIVE.as_bytes()), b"2.1.3\0");
    assert!(p.scan().is_live(&n("X")));
}

#[test]
fn an_amplihack_exe_is_a_candidate_ignoring_case() {
    let p = FakeProc::new();
    p.on_x(201, Some(b"/opt/AmpliHack/bin/runner"), b"runner\0");
    assert!(p.scan().is_live(&n("X")));
}

/// The D9 change: a process whose `exe` shows it is something else no
/// longer makes the directory it inherited live.
#[test]
fn an_unrelated_binary_that_inherited_the_config_dir_is_not_live() {
    let p = FakeProc::new();
    p.on_x(202, Some(b"/usr/bin/bash"), b"bash\0-c\0claude-wrapper\0");
    let l = p.scan();
    assert_eq!(l, Liveness::Known(Default::default()));
}

/// R7: a process that is not a candidate never has its environment read, so
/// an unreadable one cannot turn the answer into `Everything`, even when its
/// command line mentions Claude.
#[test]
fn a_non_candidates_environment_is_never_opened() {
    let p = FakeProc::new();
    let dir = p.on_x(203, Some(b"/usr/bin/python3"), b"python3\0claude.py\0");
    unreadable(&dir.join("environ"));
    assert_eq!(p.scan(), Liveness::Known(Default::default()));
}

#[test]
fn node_running_claude_is_a_candidate() {
    let p = FakeProc::new();
    p.on_x(
        204,
        Some(b"/usr/bin/node"),
        b"node\0/usr/lib/node_modules/@anthropic-ai/claude-code/cli.js\0",
    );
    assert!(p.scan().is_live(&n("X")));
}

#[test]
fn node_running_something_else_is_not_a_candidate() {
    let p = FakeProc::new();
    let dir = p.on_x(205, Some(b"/usr/bin/node"), b"node\0server.js\0");
    unreadable(&dir.join("environ"));
    assert_eq!(p.scan(), Liveness::Known(Default::default()));
}

#[test]
fn an_interpreter_with_an_unreadable_command_line_is_a_candidate() {
    for exe in ["/usr/bin/node", "/usr/local/bin/bun", "/opt/deno/deno"] {
        let p = FakeProc::new();
        let dir = p.on_x(206, Some(exe.as_bytes()), b"");
        unreadable(&dir.join("cmdline"));
        assert!(p.scan().is_live(&n("X")), "{exe}");
    }
}

#[test]
fn a_deleted_exe_still_matches() {
    let p = FakeProc::new();
    p.on_x(207, Some(format!("{NATIVE} (deleted)").as_bytes()), b"x\0");
    assert!(
        p.scan().is_live(&n("X")),
        "native install replaced by an update"
    );
    let p = FakeProc::new();
    p.on_x(
        208,
        Some(b"/usr/bin/node (deleted)"),
        b"node\0/x/claude/cli.js\0",
    );
    assert!(p.scan().is_live(&n("X")), "node replaced by an update");
}

#[test]
fn a_non_utf8_exe_path_still_matches_on_its_components() {
    let p = FakeProc::new();
    p.on_x(209, Some(b"/opt/\xffClaude\xfe/bin/x"), b"x\0");
    assert!(p.scan().is_live(&n("X")));
}

/// `candidate(exe, cmdline)`: `cmdline` is a lazy read, asked for only when
/// the exe's base name is `node`, `bun` or `deno`.
#[test]
fn the_candidate_table_reads_cmdline_only_for_interpreters() {
    type Row = (Option<&'static str>, Option<&'static str>, bool, usize);
    let rows: &[Row] = &[
        // (exe, cmdline, candidate, cmdline reads)
        (Some(NATIVE), Some("x\0"), true, 0),
        (Some("/usr/bin/claude"), None, true, 0),
        (Some("/x/AMPLIHACK/y"), None, true, 0),
        (None, Some("vim\0"), true, 0),
        (Some("/usr/bin/bash"), Some("claude\0"), false, 0),
        (Some("/usr/bin/python3"), Some("claude\0"), false, 0),
        (
            Some("/usr/bin/node"),
            Some("node\0/x/claude/cli.js\0"),
            true,
            1,
        ),
        (Some("/usr/bin/node"), Some("node\0amplihack\0"), true, 1),
        (Some("/usr/bin/node"), Some("node\0server.js\0"), false, 1),
        (Some("/usr/bin/node"), None, true, 1),
        (Some("/usr/bin/bun"), Some("bun\0x.ts\0"), false, 1),
        (Some("/usr/bin/bun"), Some("bun\0Claude\0"), true, 1),
        (Some("/usr/bin/deno"), None, true, 1),
        (Some("/usr/bin/nodejs-helper"), Some("claude\0"), false, 0),
        (
            Some("/usr/bin/node (deleted)"),
            Some("node\0claude\0"),
            true,
            1,
        ),
    ];
    for (exe, cmdline, want, reads) in rows {
        let calls = Cell::new(0usize);
        let got = liveness::candidate(exe.map(Path::new), || {
            calls.set(calls.get() + 1);
            cmdline.map(|c| c.as_bytes().to_vec())
        });
        assert_eq!(got, *want, "exe {exe:?}");
        assert_eq!(calls.get(), *reads, "cmdline reads for exe {exe:?}");
    }
}

#[test]
fn collect_records_the_fields_of_each_same_uid_process_and_the_caller() {
    let p = FakeProc::new();
    p.on_x(210, None, b"claude\0");
    p.on_x(SELF_PID, None, b"claude\0");
    p.process(211, UID + 1, "S (sleeping)", &[&p.config_dir("Y")], &p.root);
    let r = p.record(210).expect("a same-uid process is recorded");
    assert_eq!(r.profile.as_deref(), Some("X"));
    assert!(r.claude_like && !r.zombie && !r.environ_unreadable);
    assert!(p.record(SELF_PID).is_some(), "the writer records itself");
    assert!(p.record(211).is_none(), "another uid is not recorded");
}

#[test]
fn derive_drops_the_reader_and_zombies() {
    let p = FakeProc::new();
    p.on_x(SELF_PID, None, b"claude\0");
    let dir = p.process(212, UID, "Z (zombie)", &[&p.config_dir("Y")], &p.root);
    fs::write(dir.join("stat"), stat_line(212, "claude", 5)).unwrap();
    let c = p.collect();
    assert_eq!(
        liveness::derive(&c, SELF_PID),
        Liveness::Known(Default::default())
    );
    let l = liveness::derive(&c, 1);
    assert!(l.is_live(&n("X")), "another reader sees the process on X");
    assert!(!l.is_live(&n("Y")), "a zombie is never live");
}

#[test]
fn an_unreadable_claude_environment_is_recorded_and_derives_everything() {
    let p = FakeProc::new();
    let dir = p.on_x(213, None, b"claude\0-p\0x\0");
    unreadable(&dir.join("environ"));
    let r = p.record(213).unwrap();
    assert!(r.claude_like && r.environ_unreadable && r.profile.is_none());
    assert_eq!(
        liveness::derive(&p.collect(), SELF_PID),
        Liveness::Everything
    );
    assert_eq!(
        liveness::derive(&p.collect(), 213),
        Liveness::Known(Default::default()),
        "a process never counts itself"
    );
}

#[test]
fn collect_is_unsupported_when_its_own_environment_cannot_be_read() {
    let p = FakeProc::new();
    unreadable(&p.proc_root.join("self/environ"));
    assert!(matches!(p.collect(), Collected::Unsupported));
    assert_eq!(
        liveness::derive(&Collected::Unsupported, SELF_PID),
        Liveness::Unsupported
    );
}

/// `stat` is not read, so a crafted or missing one changes nothing: the
/// process still counts.
#[test]
fn the_stat_file_does_not_affect_liveness() {
    for stat in [
        Some(
            "214 (a) b (c) S 1 214 214 0 -1 4194304 0 0 0 0 0 0 0 0 20 0 1 0 99001 0 0".to_string(),
        ),
        Some("214 (claude S 1 2 3".to_string()),
        Some(String::from_utf8_lossy(b"\xff\xfe)").into_owned()),
        Some(String::new()),
        None,
    ] {
        let p = FakeProc::new();
        let dir = p.on_x(214, None, b"claude\0");
        match &stat {
            Some(s) => fs::write(dir.join("stat"), s).unwrap(),
            None => fs::remove_file(dir.join("stat")).unwrap(),
        }
        assert!(p.record(214).is_some(), "{stat:?}");
        assert!(p.scan().is_live(&n("X")), "{stat:?} still counts");
    }
}

#[test]
fn malformed_status_lines_skip_the_process_without_panicking() {
    for status in [
        "Name:\tclaude\nState:\tS\n".to_string(),
        format!("Name:\tclaude\nUid:\t{UID}\n"),
        "State:\tS\nUid:\tnobody\n".to_string(),
        "Uid:\t99999999999999999999\nState:\tS\n".to_string(),
        String::new(),
    ] {
        let p = FakeProc::new();
        let dir = p.on_x(216, None, b"claude\0");
        fs::write(dir.join("status"), &status).unwrap();
        assert!(!p.scan().is_live(&n("X")), "{status:?}");
    }
}

/// A Claude-like process that exits between its `exe` read and its
/// `environ` read gives `ESRCH`. That is an exit, not an unreadable
/// environment: it must not make every directory live. Found by outside-in
/// testing of #2040, where another session's short-lived `amplihack` exiting
/// mid-scan turned a whole snapshot into `Everything`.
#[test]
fn esrch_on_environ_is_an_exit_not_an_unreadable_environment() {
    use std::io::{Error, ErrorKind};
    assert!(liveness::exited_mid_read(&Error::from_raw_os_error(
        libc::ESRCH
    )));
    for refused in [
        Error::from_raw_os_error(libc::EACCES),
        Error::from_raw_os_error(libc::EPERM),
        Error::from_raw_os_error(libc::EIO),
        Error::from(ErrorKind::InvalidData),
    ] {
        assert!(!liveness::exited_mid_read(&refused), "{refused:?}");
    }
}
