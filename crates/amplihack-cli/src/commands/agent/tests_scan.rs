//! Unit tests for `scan::liveness`: the three liveness signals (R4.1, design
//! 4.8), when liveness is unknown (R4.2, D21), and the C5 rule that a
//! recorded process counts only while it is Claude. The observation table
//! and the id refresh are in `tests_scan_observe.rs`.
//!
//! Snapshots and indexes are built by the real readers over a synthetic
//! `/proc` and config directory in a temp dir, never the host's.
//!
//! API under test:
//! - `procfs::snapshot(root: &Path, uid: u32) -> Result<Snapshot, String>`
//! - `procfs::CLAUDE: ClaudeNames`
//! - `sessions::index(dirs: &[PathBuf]) -> Result<Index, String>`
//! - `scan::liveness(snap: Result<&Snapshot, &str>, index: Result<&Index, &str>,
//!   names: &ClaudeNames, id: Option<&str>, recorded: Option<(u32, u64)>) -> Liveness`
//! - `enum Liveness { Running(&'static str), NotRunning, Unknown(String) }`
//!   (`Debug`, `PartialEq`)
//! - Helpers from `tests_proc_tree.rs` (a test-only module: `Tree`, `tree()`,
//!   `add`, `add_as`, `session_file`, `snap`, `idx`, `unreadable_cmdline`)

use super::procfs::{CLAUDE, snapshot};
use super::scan::*;
use super::tests_proc_tree::*;
use amplihack_utils::secure_files::euid;
use std::fs;
use std::path::Path;

const ID: &str = "0b6c3f2e-8d4a-4f1b-9c7e-2a5d6e8f1a3b";

fn live(t: &Tree, id: Option<&str>, recorded: Option<(u32, u64)>) -> Liveness {
    let (s, i) = (snap(t), idx(t));
    liveness(Ok(&s), Ok(&i), &CLAUDE, id, recorded)
}

fn running(l: &Liveness) -> bool {
    matches!(l, Liveness::Running(_))
}

#[test]
fn signal_a_a_sessions_file_whose_process_is_alive() {
    let t = tree();
    add(&t, 5000, 1, "claude", &["claude"], 777, 'S');
    session_file(&t, 5000, ID, "777");
    assert!(running(&live(&t, Some(ID), None)));
}

#[test]
fn signal_a_needs_the_matching_start_time() {
    let t = tree();
    add(&t, 5000, 1, "claude", &["claude"], 777, 'S');
    session_file(&t, 5000, ID, "778");
    assert_eq!(
        live(&t, Some(ID), None),
        Liveness::NotRunning,
        "the pid was reused"
    );
    session_file(&t, 5001, ID, "777");
    assert_eq!(
        live(&t, Some(ID), None),
        Liveness::NotRunning,
        "no such process"
    );
}

#[test]
fn signal_b_the_id_on_a_claude_command_line() {
    let t = tree();
    add(&t, 5000, 1, "claude", &["claude", "--resume", ID], 9, 'S');
    assert!(running(&live(&t, Some(ID), None)));
    let t = tree();
    let cli = "/usr/lib/node_modules/@anthropic-ai/claude-code/cli.js";
    add(&t, 5000, 1, "node", &["node", cli, "-r", ID], 9, 'S');
    assert!(
        running(&live(&t, Some(ID), None)),
        "a node-hosted Claude counts (A5)"
    );
}

#[test]
fn signal_b_ignores_other_programs_zombies_and_other_users() {
    let t = tree();
    add(&t, 5000, 1, "vim", &["vim", ID], 9, 'S');
    add(&t, 5001, 1, "claude", &["claude", "--resume", ID], 9, 'Z');
    let other = euid().wrapping_add(1);
    add_as(
        &t,
        5002,
        1,
        "claude",
        &["claude", "--resume", ID],
        9,
        'S',
        (other, other),
    );
    add_as(
        &t,
        5003,
        1,
        "claude",
        &["claude", "--resume", ID],
        9,
        'S',
        (other, euid()),
    );
    add_as(
        &t,
        5004,
        1,
        "claude",
        &["claude", "--resume", ID],
        9,
        'S',
        (euid(), other),
    );
    assert_eq!(live(&t, Some(ID), None), Liveness::NotRunning);
}

#[test]
fn signal_c_the_recorded_claude_process() {
    let t = tree();
    add(&t, 6000, 1, "claude", &["claude"], 900, 'S');
    assert!(running(&live(&t, Some(ID), Some((6000, 900)))));
    assert!(
        running(&live(&t, None, Some((6000, 900)))),
        "(c) applies without an id"
    );
    assert_eq!(live(&t, Some(ID), Some((6000, 901))), Liveness::NotRunning);
    assert_eq!(live(&t, Some(ID), Some((6001, 900))), Liveness::NotRunning);
}

#[test]
fn signal_c_needs_a_claude_process_c5() {
    let t = tree();
    add(&t, 6000, 1, "python3", &["python3", "server.py"], 900, 'S');
    assert_eq!(live(&t, Some(ID), Some((6000, 900))), Liveness::NotRunning);
}

#[test]
fn without_an_id_only_the_recorded_process_counts() {
    let t = tree();
    add(&t, 5000, 1, "claude", &["claude", "--resume", ID], 9, 'S');
    session_file(&t, 5000, ID, "9");
    assert_eq!(live(&t, None, None), Liveness::NotRunning);
}

#[test]
fn an_unreadable_snapshot_or_index_makes_liveness_unknown() {
    let t = tree();
    let i = idx(&t);
    let l = liveness(
        Err("cannot read /proc"),
        Ok(&i),
        &CLAUDE,
        Some(ID),
        Some((1, 1)),
    );
    assert!(
        matches!(&l, Liveness::Unknown(r) if r.contains("/proc")),
        "{l:?}"
    );
    let s = snap(&t);
    let reason = "Claude config directory is not private";
    let l = liveness(Ok(&s), Err(reason), &CLAUDE, Some(ID), None);
    assert_eq!(l, Liveness::Unknown(reason.into()));
}

#[test]
fn a_claude_process_with_an_unreadable_command_line_makes_it_unknown() {
    let t = tree();
    add(&t, 5000, 1, "claude", &["claude"], 9, 'S');
    unreadable_cmdline(&t, 5000);
    assert!(matches!(live(&t, Some(ID), None), Liveness::Unknown(_)));
}

#[test]
fn a_positive_signal_beats_unknown() {
    let t = tree();
    add(&t, 5000, 1, "claude", &["claude", "--resume", ID], 9, 'S');
    let s = snap(&t);
    let l = liveness(Ok(&s), Err("x"), &CLAUDE, Some(ID), None);
    assert!(running(&l), "{l:?}");
    add(&t, 5001, 1, "claude", &["claude"], 9, 'S');
    unreadable_cmdline(&t, 5001);
    assert!(running(&live(&t, Some(ID), None)));
}

#[test]
fn snapshot_fails_without_self() {
    let t = tree();
    fs::remove_file(t.proc_root.join("self")).unwrap();
    assert!(snapshot(&t.proc_root, euid()).is_err());
    assert!(snapshot(Path::new("/nonexistent-amplihack-proc"), euid()).is_err());
}
