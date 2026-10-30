//! Issue #1760, R9.2(2) and R4: the supervisor never starts a second copy of a
//! conversation. Each running signal on its own stops a resume: (a) a
//! sessions file whose pid and `procStart` are live, (b) a `claude` process
//! with the id in its command line, (c) the recorded `claude_pid` and
//! `proc_start` (C5: only when that process is Claude). So does an existing
//! tmux session. Doubt counts as running (R4.2, D21, B5), two entries sharing
//! one id are resumed once (C3), and an unsafe socket directory is never
//! handed to tmux (S12). A missing default socket is no session, whatever
//! tmux would say about it.

#![cfg(target_os = "linux")]

use crate::issue_1760_fake_proc::Proc;
use crate::issue_1760_fixture::*;
use serde_json::json;
use std::path::PathBuf;

/// The `result=` text of the latest log line for `name`.
fn result_of(fx: &Fx, name: &str) -> Option<String> {
    let tag = format!(" agent={name} ");
    let lines = fx.log_lines();
    let line = lines.iter().rev().find(|l| l.contains(&tag))?;
    line.split_once(" result=").map(|(_, r)| r.to_string())
}

/// A launch directory with a transcript for `conv` in `~/.claude`.
fn lost_dir(fx: &Fx, name: &str, conv: &str) -> PathBuf {
    let d = fx.dir(name);
    fx.transcript(&d, conv);
    d
}

/// One pass; asserts exit 0 and returns the pass's stdout.
fn pass(fx: &Fx) -> String {
    let out = fx.supervise_once(&["--gap", "0"]);
    expect_code(&out, 0);
    stdout(&out)
}

#[test]
fn signal_a_a_live_sessions_file_in_any_config_dir_means_running() {
    let fx = Fx::new();
    let (a, b) = (
        lost_dir(&fx, "home-a", CONV_A),
        lost_dir(&fx, "prof-b", CONV_B),
    );
    fx.add_proc(&Proc::new(5100, 1, &["claude"], &a).start(4242));
    fx.sessions_file(&fx.claude_dir, 5100, CONV_A, 4242, &a);
    fx.add_proc(&Proc::new(5101, 1, &["claude"], &b).start(4343));
    fx.sessions_file(&fx.profile_dir("work"), 5101, CONV_B, 4343, &b);
    fx.write_agents(&[
        ("home-a", fx.entry("home-a", &a, Some(CONV_A))),
        ("prof-b", fx.entry("prof-b", &b, Some(CONV_B))),
    ]);
    let out = pass(&fx);
    assert!(fx.new_sessions().is_empty(), "{:?}", fx.new_sessions());
    for name in ["home-a", "prof-b"] {
        let r = result_of(&fx, name);
        assert_eq!(r.as_deref(), Some("skipped: already running"), "{name}");
        assert!(
            out.contains(&format!(" agent={name} ")),
            "A15: stdout:\n{out}"
        );
    }
}

#[test]
fn signal_b_a_claude_command_line_holding_the_id_means_running() {
    let fx = Fx::new();
    let d = lost_dir(&fx, "coord", CONV_A);
    fx.add_proc(&Proc::new(5200, 1, &["claude", "--resume", CONV_A], &d));
    fx.write_agents(&[("coord", fx.entry("coord", &d, Some(CONV_A)))]);
    pass(&fx);
    assert!(fx.new_sessions().is_empty());
    assert_eq!(
        result_of(&fx, "coord").as_deref(),
        Some("skipped: already running")
    );
}

#[test]
fn signal_c_the_recorded_claude_process_means_running() {
    let fx = Fx::new();
    let d = lost_dir(&fx, "coord", CONV_A);
    fx.add_proc(&Proc::new(5300, 1, &["claude"], &d).start(999));
    let mut e = fx.entry("coord", &d, Some(CONV_A));
    e["claude_pid"] = json!(5300);
    e["proc_start"] = json!(999);
    fx.write_agents(&[("coord", e)]);
    pass(&fx);
    assert!(fx.new_sessions().is_empty());
    assert_eq!(
        result_of(&fx, "coord").as_deref(),
        Some("skipped: already running")
    );
}

/// Look-alikes are not running: a zombie, another user's process, a program
/// that is not Claude, a sessions file whose `procStart` is stale, a
/// recorded pid that now runs another program (C5) or started at another
/// time (pid reuse).
#[test]
fn processes_that_only_look_like_the_conversation_do_not_block_a_resume() {
    const IDS: [&str; 6] = [
        CONV_A,
        CONV_B,
        CONV_C,
        CONV_D,
        "0aaa1111-2b2b-4c3c-8d4d-5e5e5e5e5e01",
        "0aaa1111-2b2b-4c3c-8d4d-5e5e5e5e5e02",
    ];
    let names = [
        "zombie",
        "other-uid",
        "not-claude",
        "stale-file",
        "reused-pid",
        "bash-pid",
    ];
    let fx = Fx::new();
    let dirs: Vec<PathBuf> = names
        .iter()
        .zip(IDS)
        .map(|(n, id)| lost_dir(&fx, n, id))
        .collect();
    fx.add_proc(&Proc::new(5201, 1, &["claude", "--resume", IDS[0]], &dirs[0]).state('Z'));
    let other = fx.uid + 1;
    fx.add_proc(&Proc::new(5202, 1, &["claude", "--resume", IDS[1]], &dirs[1]).uids(other, other));
    fx.add_proc(&Proc::new(5203, 1, &["vim", IDS[2]], &dirs[2]));
    fx.add_proc(&Proc::new(5204, 1, &["claude"], &dirs[3]).start(1000));
    fx.sessions_file(&fx.claude_dir, 5204, IDS[3], 999, &dirs[3]);
    fx.add_proc(&Proc::new(5205, 1, &["claude"], &dirs[4]).start(1001));
    fx.add_proc(&Proc::new(5206, 1, &["bash"], &dirs[5]).start(1002));
    let mut agents: Vec<(&str, serde_json::Value)> = names
        .iter()
        .zip(IDS)
        .zip(&dirs)
        .map(|((n, id), d)| (*n, fx.entry(n, d, Some(id))))
        .collect();
    agents[4].1["claude_pid"] = json!(5205);
    agents[4].1["proc_start"] = json!(1000);
    agents[5].1["claude_pid"] = json!(5206);
    agents[5].1["proc_start"] = json!(1002);
    fx.write_agents(&agents);
    pass(&fx);
    let mut started: Vec<String> = fx
        .new_sessions()
        .iter()
        .map(|c| c.opt("-s").unwrap_or("").to_string())
        .collect();
    started.sort();
    let mut want: Vec<String> = names.iter().map(|s| s.to_string()).collect();
    want.sort();
    assert_eq!(started, want, "every look-alike is resumed exactly once");
    for name in names {
        assert_eq!(result_of(&fx, name).as_deref(), Some("resumed"), "{name}");
    }
}

#[test]
fn an_existing_tmux_session_is_never_duplicated() {
    let fx = Fx::new();
    let d = lost_dir(&fx, "coord", CONV_A);
    fx.tmux_session("coord", "main");
    fx.write_agents(&[("coord", fx.entry("coord", &d, Some(CONV_A)))]);
    pass(&fx);
    assert!(fx.new_sessions().is_empty());
    assert_eq!(
        result_of(&fx, "coord").as_deref(),
        Some("skipped: tmux session exists")
    );
}

fn assert_unknown_liveness(fx: &Fx, reason: Option<&str>) {
    pass(fx);
    assert!(fx.new_sessions().is_empty(), "doubt counts as running");
    let r = result_of(fx, "coord").expect("a line for coord");
    let prefix = "skipped: already running (liveness unknown: ";
    assert!(r.starts_with(prefix) && r.ends_with(')'), "{r}");
    if let Some(reason) = reason {
        assert_eq!(r, format!("{prefix}{reason})"));
    }
}

#[test]
fn an_unreadable_proc_makes_liveness_unknown() {
    let fx = Fx::new();
    let d = lost_dir(&fx, "coord", CONV_A);
    fx.write_agents(&[("coord", fx.entry("coord", &d, Some(CONV_A)))]);
    fx.break_proc();
    assert_unknown_liveness(&fx, None);
}

#[test]
fn a_claude_process_whose_command_line_cannot_be_read_makes_liveness_unknown() {
    let fx = Fx::new();
    let d = lost_dir(&fx, "coord", CONV_A);
    fx.add_proc(&Proc::new(5400, 1, &["claude"], &d));
    fx.unreadable_cmdline(5400);
    fx.write_agents(&[("coord", fx.entry("coord", &d, Some(CONV_A)))]);
    assert_unknown_liveness(&fx, None);
}

#[test]
fn a_profile_directory_other_users_can_write_makes_liveness_unknown() {
    let fx = Fx::new();
    let d = lost_dir(&fx, "coord", CONV_A);
    chmod(&fx.profile_dir("loose"), 0o777);
    fx.write_agents(&[("coord", fx.entry("coord", &d, Some(CONV_A)))]);
    assert_unknown_liveness(&fx, Some("Claude config directory is not private"));
}

/// B5: a broken tmux is unknown, which counts as a present session.
#[test]
fn a_broken_tmux_is_reported_with_its_reason_and_starts_nothing() {
    let fx = Fx::new();
    let d = lost_dir(&fx, "coord", CONV_A);
    fx.tmux_broken();
    fx.write_agents(&[("coord", fx.entry("coord", &d, Some(CONV_A)))]);
    pass(&fx);
    assert!(fx.new_sessions().is_empty());
    let r = result_of(&fx, "coord").expect("a line for coord");
    let prefix = "skipped: tmux session exists (tmux state unknown: ";
    assert!(r.starts_with(prefix) && r.len() > prefix.len() + 1, "{r}");
}

/// C3: two active entries on one conversation (a hand edit) start it once.
#[test]
fn two_entries_sharing_a_conversation_are_resumed_once() {
    let fx = Fx::new();
    let (one, two) = (lost_dir(&fx, "one", CONV_A), lost_dir(&fx, "two", CONV_A));
    fx.write_agents(&[
        ("one", fx.entry("one", &one, Some(CONV_A))),
        ("two", fx.entry("two", &two, Some(CONV_A))),
    ]);
    pass(&fx);
    let started = fx.new_sessions();
    assert_eq!(started.len(), 1, "{started:?}");
    assert_eq!(started[0].opt("-s"), Some("one"), "equal dirt: name order");
    assert_eq!(result_of(&fx, "one").as_deref(), Some("resumed"));
    assert_eq!(
        result_of(&fx, "two").as_deref(),
        Some("skipped: already running")
    );
}

/// S12: a recorded socket whose directory other users can enter is never
/// passed to tmux, because a tmux client sends its environment there.
#[test]
fn a_socket_in_a_directory_open_to_others_is_never_contacted() {
    let fx = Fx::new();
    let d = lost_dir(&fx, "coord", CONV_A);
    let loose = fx.tmux_tmpdir.join("loose");
    std::fs::create_dir(&loose).unwrap();
    chmod(&loose, 0o777);
    let mut e = fx.entry("coord", &d, Some(CONV_A));
    e["tmux_socket"] = json!(loose.join("default"));
    fx.write_agents(&[("coord", e)]);
    pass(&fx);
    assert!(fx.tmux_records().is_empty(), "{:?}", fx.tmux_records());
    assert_eq!(
        result_of(&fx, "coord").as_deref(),
        Some("skipped: tmux session exists (tmux state unknown: socket directory is not private)")
    );
}

/// Review issue 1: an entry registered without `--tmux-socket`, after a
/// reboot. No tmux server has started, so tmux would print `error connecting
/// to ...`, which is not one of its "absent" messages; the missing default
/// socket decides first, and the agent is resumed.
#[test]
fn a_missing_default_socket_means_the_session_is_gone() {
    let fx = Fx::new();
    let d = lost_dir(&fx, "coord", CONV_A);
    expect_code(&fx.register("coord", "coord", &d, CONV_A, &[]), 0);
    assert!(fx.agent("coord")["tmux_socket"].is_null());
    let empty = fx.root.join("empty-tt");
    std::fs::create_dir(&empty).unwrap();
    chmod(&empty, 0o700);
    fx.tmux_no_default_server();
    let mut cmd = fx.cmd(&["agent", "supervise", "--once", "--gap", "0"]);
    expect_code(&fx.output(cmd.env("TMUX_TMPDIR", &empty)), 0);
    assert_eq!(result_of(&fx, "coord").as_deref(), Some("resumed"));
    let started = fx.new_sessions();
    assert_eq!(started.len(), 1, "{started:?}");
    assert_eq!(started[0].socket(), None);
}
