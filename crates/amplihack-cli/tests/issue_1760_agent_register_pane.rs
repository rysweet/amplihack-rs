//! Issue #1760: `agent register --pane`, socket recording and refusals.
//!
//! - `--pane TARGET`: `display-message -p -t TARGET '#{pane_pid}'`, then the
//!   nearest `claude` below that pid (design 4.9).
//! - Socket (A9): `--tmux-socket`, else the first field of the caller's
//!   `$TMUX`, both passed as `-S`; else no `-S`, and the recorded socket is
//!   `TMUX_TMPDIR/tmux-UID/default`.
//! - Explicit flags override discovered values (R3.4); with both `--session`
//!   and `--window`, tmux is not asked for names.
//! - R3.5 no process; D9 tmux session in use; C3 conversation in use; S12 a
//!   socket in a directory other users can enter is refused.

#![cfg(target_os = "linux")]

use crate::issue_1760_fake_proc::Proc;
use crate::issue_1760_fakes::TmuxCall;
use crate::issue_1760_fixture::{CONV_A, CONV_B, CONV_C, Fx, chmod, expect_code, stderr, stdout};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

const PANE_PID: u32 = 6000;
const CLAUDE_PID: u32 = 6001;

/// Pane `%12` of session `area`, window `main`: a shell running
/// `claude --model opus` in `work/area`, conversation `CONV_B`.
fn pane(fx: &Fx) -> PathBuf {
    let d = fx.dir("area");
    fx.add_proc(&Proc::new(PANE_PID, 1, &["-bash"], &d).comm("bash"));
    let claude = Proc::new(CLAUDE_PID, PANE_PID, &["claude", "--model", "opus"], &d).start(99);
    fx.add_proc(&claude);
    fx.sessions_file(&fx.claude_dir, CLAUDE_PID, CONV_B, 99, &d);
    fx.tmux_pane("%12", "area", "main", PANE_PID);
    d
}

fn displays(fx: &Fx) -> Vec<TmuxCall> {
    fx.tmux_records()
        .into_iter()
        .filter(|c| c.command() == "display-message")
        .collect()
}

/// A real socket file at `dir/sock`, `dir` created with `mode`.
fn socket_in(fx: &Fx, name: &str, mode: u32) -> PathBuf {
    let dir = fx.root.join(name);
    fs::create_dir_all(&dir).unwrap();
    chmod(&dir, mode);
    let s = dir.join("sock");
    drop(std::os::unix::net::UnixListener::bind(&s).unwrap());
    s
}

fn path_str(p: &Path) -> String {
    p.display().to_string()
}

#[test]
fn pane_reads_the_claude_below_the_pane_process() {
    let fx = Fx::new();
    let d = pane(&fx);
    let o = fx.run(&["agent", "register", "area", "--pane", "%12"]);
    expect_code(&o, 0);
    let e = fx.agent("area");
    assert_eq!(e["claude_pid"], json!(CLAUDE_PID));
    assert_eq!(e["proc_start"], json!(99));
    assert_eq!(e["conversation_id"], CONV_B);
    assert_eq!(e["launch_dir"], json!(d));
    assert_eq!(e["model"], "opus");
    assert_eq!(
        (&e["tmux_session"], &e["tmux_window"]),
        (&json!("area"), &json!("main"))
    );
    assert_eq!(
        e["tmux_socket"],
        json!(fx.socket),
        "no -S: the recorded socket is TMUX_TMPDIR/tmux-UID/default"
    );
    let calls = displays(&fx);
    assert!(calls.iter().all(|c| c.socket().is_none()), "{calls:?}");
    assert!(
        calls.iter().all(|c| c.opt("-t") == Some("%12")),
        "{calls:?}"
    );
    let formats: Vec<_> = calls
        .iter()
        .filter_map(|c| c.argv.last().cloned())
        .collect();
    assert!(formats.contains(&"#{pane_pid}".to_string()), "{formats:?}");
    assert!(
        formats.contains(&"#{session_name}\t#{window_name}".to_string()),
        "{formats:?}"
    );
}

#[test]
fn tmux_socket_is_passed_and_recorded() {
    let fx = Fx::new();
    pane(&fx);
    let s = socket_in(&fx, "private", 0o700);
    let o = fx.run(&[
        "agent",
        "register",
        "area",
        "--pane",
        "%12",
        "--tmux-socket",
        &path_str(&s),
    ]);
    expect_code(&o, 0);
    assert_eq!(fx.agent("area")["tmux_socket"], json!(s));
    let calls = displays(&fx);
    assert!(!calls.is_empty());
    assert!(
        calls
            .iter()
            .all(|c| c.socket() == Some(path_str(&s).as_str())),
        "{calls:?}"
    );
}

#[test]
fn the_callers_tmux_names_the_socket() {
    let fx = Fx::new();
    pane(&fx);
    let s = socket_in(&fx, "mine", 0o700);
    let mut c = fx.cmd(&["agent", "register", "area", "--pane", "%12"]);
    c.env("TMUX", format!("{},77,0", s.display()))
        .env("TMUX_PANE", "%1");
    let o = fx.output(&mut c);
    expect_code(&o, 0);
    assert_eq!(fx.agent("area")["tmux_socket"], json!(s));
    assert!(
        displays(&fx)
            .iter()
            .all(|c| c.socket() == Some(path_str(&s).as_str())),
        "{:?}",
        displays(&fx)
    );
}

#[test]
fn explicit_flags_override_what_was_found() {
    let fx = Fx::new();
    pane(&fx);
    let d2 = path_str(&fx.dir("elsewhere"));
    let o = fx.run(&[
        "agent",
        "register",
        "area",
        "--pane",
        "%12",
        "--session",
        "other",
        "--window",
        "w2",
        "--dir",
        &d2,
        "--conversation",
        CONV_C,
        "--model",
        "sonnet",
    ]);
    expect_code(&o, 0);
    let e = fx.agent("area");
    assert_eq!(e["tmux_session"], "other");
    assert_eq!(e["tmux_window"], "w2");
    assert_eq!(e["launch_dir"], json!(d2));
    assert_eq!(e["conversation_id"], CONV_C);
    assert_eq!(e["model"], "sonnet");
    assert_eq!(
        e["claude_pid"],
        json!(CLAUDE_PID),
        "the process is still the pane's"
    );
    let calls = displays(&fx);
    assert!(
        calls
            .iter()
            .all(|c| c.argv.last().map(String::as_str) == Some("#{pane_pid}")),
        "tmux was asked for names already given: {calls:?}"
    );
}

#[test]
fn a_pane_without_claude_is_an_error() {
    let fx = Fx::new();
    let d = fx.dir("d");
    fx.add_proc(&Proc::new(PANE_PID, 1, &["bash"], &d));
    fx.tmux_pane("%12", "area", "main", PANE_PID);
    let o = fx.run(&["agent", "register", "area", "--pane", "%12"]);
    expect_code(&o, 1);
    assert!(!fx.registry_path().exists());
    let o = fx.run(&["agent", "register", "area", "--pane", "%99"]);
    expect_code(&o, 1);
    assert!(
        stderr(&o).contains("tmux display-message failed: can't find pane: %99"),
        "{}",
        stderr(&o)
    );
}

#[test]
fn no_process_and_missing_flags_is_refused() {
    let fx = Fx::new();
    let d = path_str(&fx.dir("d"));
    for args in [
        vec!["agent", "register", "x"],
        vec![
            "agent",
            "register",
            "x",
            "--session",
            "s",
            "--window",
            "w",
            "--dir",
            &d,
        ],
    ] {
        let o = fx.run(&args);
        expect_code(&o, 1);
        assert!(
            stderr(&o).contains(
                "no claude process found; pass --session, --window, --dir and --conversation"
            ),
            "{args:?}: {}",
            stderr(&o)
        );
    }
    assert!(!fx.registry_path().exists());
}

#[test]
fn a_tmux_session_used_by_another_active_agent_is_refused() {
    let fx = Fx::new();
    let (da, db) = (fx.dir("a"), fx.dir("b"));
    expect_code(&fx.register("a", "shared", &da, CONV_A, &[]), 0);
    expect_code(&fx.register("a", "shared", &da, CONV_A, &[]), 0);
    let before = fx.registry_raw();
    let o = fx.register("b", "shared", &db, CONV_B, &[]);
    expect_code(&o, 1);
    assert!(
        stderr(&o).contains("tmux session shared is already used by registered agent a"),
        "{}",
        stderr(&o)
    );
    assert!(stdout(&o).is_empty());
    assert_eq!(fx.registry_raw(), before);
    // Only active agents hold a session.
    let mut ended = fx.agent("a");
    ended["state"] = json!("ended");
    ended["ended_at"] = ended["launched_at"].clone();
    ended["ended_by"] = json!("wrapper");
    fx.write_agents(&[("a", ended)]);
    expect_code(&fx.register("b", "shared", &db, CONV_B, &[]), 0);
}

#[test]
fn a_conversation_used_by_another_active_agent_is_refused() {
    let fx = Fx::new();
    expect_code(&fx.register("a", "sa", &fx.dir("a"), CONV_A, &[]), 0);
    let before = fx.registry_raw();
    let o = fx.register("b", "sb", &fx.dir("b"), CONV_A, &[]);
    expect_code(&o, 1);
    assert!(
        stderr(&o).contains("this conversation is already used by registered agent a"),
        "{}",
        stderr(&o)
    );
    assert_eq!(fx.registry_raw(), before);
    let mut stopped = fx.agent("a");
    stopped["state"] = json!("stopped");
    stopped["alert"] = json!("stopped: restart limit (3 restarts in the last hour)");
    fx.write_agents(&[("a", stopped)]);
    expect_code(&fx.register("b", "sb", &fx.dir("b"), CONV_A, &[]), 0);
}

#[test]
fn a_socket_in_a_directory_others_can_enter_is_refused() {
    let fx = Fx::new();
    let d = fx.dir("d");
    for (name, mode) in [("open", 0o777), ("others-rx", 0o705)] {
        let s = socket_in(&fx, name, mode);
        let o = fx.register("a", "s", &d, CONV_A, &["--tmux-socket", &path_str(&s)]);
        expect_code(&o, 1);
        assert!(
            stderr(&o).contains(
                "tmux socket: its directory must be owned by you and closed to other users"
            ),
            "{name}: {}",
            stderr(&o)
        );
    }
    assert!(!fx.registry_path().exists());
    assert!(fx.tmux_records().is_empty(), "tmux never contacted");
}
