//! Issue #1760, R3 and design 4.9: `agent register` reads a running `claude`
//! process from the synthetic `/proc`.
//!
//! - With neither `--pid` nor `--pane`: the nearest `claude` ancestor, its
//!   session file (matched on pid and `procStart`), command line, working
//!   directory, `TMUX`/`TMUX_PANE`, and the `amplihack claude` ancestor's flags.
//! - `--pid PID`: the same, for a named process.
//! - B2: a `-p` process is refused. S4: a missing pid, another user's process
//!   and a non-Claude process give one message.
//! - With `--conversation ID` and no `--pid`/`--pane`, the ancestor is used
//!   only when its known conversation equals ID; otherwise nothing is read
//!   from it.

#![cfg(target_os = "linux")]

use crate::issue_1760_fake_proc::Proc;
use crate::issue_1760_fixture::{CONV_A, CONV_B, Fx, expect_code, stderr, stdout};
use serde_json::{Value, json};
use std::path::PathBuf;

const CLAUDE_PID: u32 = 5000;
const AMP_PID: u32 = 4000;
const START: u64 = 4242;
const OPTIONS: &[&str] = &[
    "--model",
    "opus",
    "--effort=high",
    "--add-dir",
    "docs",
    "--permission-mode",
    "acceptEdits",
    "--dangerously-skip-permissions",
];

/// `self -> claude (CLAUDE_PID) -> amplihack claude (AMP_PID) -> 1`, the
/// claude process running `claude <args>` in `work/mistt`, in pane `%3` of
/// session `coord`, window `main`, on the fixture's socket.
fn tree(fx: &Fx, args: &[&str]) -> PathBuf {
    let d = fx.dir("mistt");
    fx.profile_dir("work");
    let amp = [
        "/home/u/.local/bin/amplihack",
        "claude",
        "--profile",
        "work",
        "--no-reflection",
        "--",
        "--model",
        "opus",
    ];
    fx.add_proc(&Proc::new(AMP_PID, 1, &amp, &d).env("CLAUDE_CODE_EFFORT_LEVEL", "high"));
    let mut argv = vec!["claude"];
    argv.extend(args);
    let claude = Proc::new(CLAUDE_PID, AMP_PID, &argv, &d)
        .start(START)
        .env("TMUX", &fx.tmux_env())
        .env("TMUX_PANE", "%3");
    fx.add_proc(&claude);
    fx.set_self_parent(CLAUDE_PID);
    fx.tmux_pane("%3", "coord", "main", CLAUDE_PID);
    d
}

fn assert_discovered(fx: &Fx, e: &Value, d: &std::path::Path) {
    assert_eq!(e["state"], "active");
    assert_eq!(e["tmux_session"], "coord");
    assert_eq!(e["tmux_window"], "main");
    assert_eq!(
        e["tmux_socket"],
        json!(fx.socket),
        "A9: the socket from TMUX"
    );
    assert_eq!(e["launch_dir"], json!(d));
    assert_eq!(e["conversation_id"], CONV_A);
    assert_eq!(e["claude_pid"], json!(CLAUDE_PID));
    assert_eq!(e["proc_start"], json!(START));
    assert_eq!(e["model"], "opus");
    assert_eq!(e["effort"], "high");
    assert_eq!(e["add_dirs"], json!([d.join("docs")]));
    assert_eq!(e["permission_mode"], "acceptEdits");
    assert_eq!(
        e["skip_permissions"],
        json!(false),
        "a permission mode wins over --dangerously-skip-permissions"
    );
    assert_eq!(
        e["amplihack_flags"],
        json!({ "subprocess_safe": false, "no_reflection": true, "profile": "work" })
    );
    assert_eq!(e["env"], json!({ "CLAUDE_CODE_EFFORT_LEVEL": "high" }));
}

#[test]
fn the_claude_ancestor_is_read() {
    let fx = Fx::new();
    let d = tree(&fx, OPTIONS);
    fx.sessions_file(&fx.claude_dir, CLAUDE_PID, CONV_A, START, &d);
    let o = fx.run(&["agent", "register", "coordinator"]);
    expect_code(&o, 0);
    assert_eq!(
        stdout(&o),
        format!(
            "registered coordinator: session coord, window main, conversation {CONV_A}, directory {}\n",
            d.display()
        )
    );
    assert_discovered(&fx, &fx.agent("coordinator"), &d);
    let display: Vec<_> = fx
        .tmux_records()
        .into_iter()
        .filter(|c| c.command() == "display-message")
        .collect();
    assert_eq!(display.len(), 1, "{display:?}");
    let socket = fx.socket.display().to_string();
    assert_eq!(display[0].socket(), Some(socket.as_str()));
    assert_eq!(display[0].opt("-t"), Some("%3"));
    assert_eq!(
        display[0].argv.last().map(String::as_str),
        Some("#{session_name}\t#{window_name}")
    );
}

#[test]
fn pid_names_the_process() {
    let fx = Fx::new();
    let d = tree(&fx, OPTIONS);
    fx.sessions_file(&fx.claude_dir, CLAUDE_PID, CONV_A, START, &d);
    fx.set_self_parent(1);
    let o = fx.run(&["agent", "register", "coordinator", "--pid", "5000"]);
    expect_code(&o, 0);
    assert_discovered(&fx, &fx.agent("coordinator"), &d);
}

#[test]
fn without_a_matching_session_file_the_resume_value_and_cwd_are_used() {
    let fx = Fx::new();
    let d = tree(&fx, &["--resume", CONV_B]);
    // procStart differs: another process that had this pid.
    fx.sessions_file(&fx.claude_dir, CLAUDE_PID, CONV_A, START + 1, &d);
    let o = fx.run(&["agent", "register", "coordinator"]);
    expect_code(&o, 0);
    let e = fx.agent("coordinator");
    assert_eq!(e["conversation_id"], CONV_B);
    assert_eq!(e["launch_dir"], json!(d));
}

#[test]
fn a_forked_resume_is_not_the_conversation() {
    let fx = Fx::new();
    tree(&fx, &["--resume", CONV_B, "--fork-session"]);
    let o = fx.run(&["agent", "register", "coordinator"]);
    expect_code(&o, 1);
    let err = stderr(&o);
    assert!(
        err.contains("claude process 5000 found, but ")
            && err.contains(" could not be discovered; pass ")
            && err.contains("--conversation"),
        "{err}"
    );
    assert!(!fx.registry_path().exists());
}

#[test]
fn a_print_run_is_refused() {
    let fx = Fx::new();
    tree(&fx, &["-p", "summarise the logs"]);
    fx.set_self_parent(1);
    let o = fx.run(&["agent", "register", "oneshot", "--pid", "5000"]);
    expect_code(&o, 1);
    assert!(
        stderr(&o).contains("process 5000 is a claude -p run, not an interactive session"),
        "{}",
        stderr(&o)
    );
    assert!(!fx.registry_path().exists());
}

#[test]
fn pids_that_cannot_be_read_get_one_message() {
    let fx = Fx::new();
    let d = fx.dir("d");
    let other = Proc::new(5001, 1, &["claude"], &d).uids(0, 0);
    fx.add_proc(&other);
    fx.add_proc(&Proc::new(5002, 1, &["bash"], &d));
    fx.add_proc(&Proc::new(5003, 1, &["claude"], &d).state('Z'));
    let mixed = Proc::new(5004, 1, &["claude"], &d).uids(fx.uid, 0);
    fx.add_proc(&mixed);
    for pid in ["7777", "5001", "5002", "5003", "5004"] {
        let o = fx.run(&["agent", "register", "x", "--pid", pid]);
        expect_code(&o, 1);
        let expected = format!("process {pid} is not a claude process of this user");
        assert!(stderr(&o).contains(&expected), "{pid}: {}", stderr(&o));
    }
    assert!(!fx.registry_path().exists());
}

#[test]
fn a_matching_conversation_uses_the_ancestor() {
    let fx = Fx::new();
    let d = tree(&fx, OPTIONS);
    fx.sessions_file(&fx.claude_dir, CLAUDE_PID, CONV_A, START, &d);
    let o = fx.run(&["agent", "register", "coordinator", "--conversation", CONV_A]);
    expect_code(&o, 0);
    assert_discovered(&fx, &fx.agent("coordinator"), &d);
}

/// The coordinator registers a lost area from its own session: nothing of
/// the coordinator's process may end up in the area's entry.
fn assert_nothing_from_the_ancestor(fx: &Fx, conv: &str) {
    let d2 = fx.dir("area");
    let dir = d2.display().to_string();
    let o = fx.run(&[
        "agent",
        "register",
        "area-search",
        "--conversation",
        conv,
        "--session",
        "area-search",
        "--window",
        "main",
        "--dir",
        &dir,
    ]);
    expect_code(&o, 0);
    let e = fx.agent("area-search");
    assert_eq!(e["conversation_id"], conv);
    assert_eq!(e["launch_dir"], json!(d2));
    for absent in [
        "claude_pid",
        "proc_start",
        "model",
        "effort",
        "permission_mode",
        "tmux_socket",
    ] {
        assert!(
            e.get(absent).is_none(),
            "{absent} came from the ancestor: {e}"
        );
    }
    assert_eq!(e["add_dirs"], json!([]));
    assert_eq!(e["skip_permissions"], json!(false));
    assert_eq!(
        e["amplihack_flags"],
        json!({ "subprocess_safe": false, "no_reflection": false })
    );
    assert_eq!(e["env"], json!({}));
    assert!(
        fx.tmux_records().is_empty(),
        "tmux asked about the ancestor's pane: {:?}",
        fx.tmux_records()
    );
}

#[test]
fn a_different_conversation_reads_nothing_from_the_ancestor() {
    let fx = Fx::new();
    let d = tree(&fx, OPTIONS);
    fx.sessions_file(&fx.claude_dir, CLAUDE_PID, CONV_A, START, &d);
    assert_nothing_from_the_ancestor(&fx, CONV_B);
}

#[test]
fn an_ancestor_with_an_unknown_conversation_is_not_used() {
    let fx = Fx::new();
    tree(&fx, OPTIONS);
    assert_nothing_from_the_ancestor(&fx, CONV_B);
    let o = fx.run(&["agent", "register", "other", "--conversation", CONV_A]);
    expect_code(&o, 1);
    assert!(
        stderr(&o).contains(
            "no claude process found; pass --session, --window, --dir and --conversation"
        ),
        "{}",
        stderr(&o)
    );
}
