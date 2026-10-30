//! Issue #1760, R9.2(1): `agent supervise --once` resumes a lost agent
//! exactly once, with its recorded options and directory.
//!
//! - One `tmux [-S SOCKET] new-session -d -s S -n W -c DIR -- PANE_ARGV`, with
//!   `-S` only when a socket is recorded, and PANE_ARGV in the R5.6/api 7
//!   order: `env`, the `-u` list (SESSION_MARKERS first, D6, A3), sorted
//!   `KEY=VALUE`, the amplihack executable, `claude`, flags, `--supervise
//!   NAME`, `--`, `--resume ID`, the R5.7 message, options.
//! - The tmux client environment has none of the R5.6 and A3 names; children
//!   see the temp HOME and profile root only; S18 no inherited agents file.
//! - The `resumed` line on stdout and in `supervisor.log`; `restarts`,
//!   `last_recovery` and `lost_at` updated; `launch_id` kept (A1).
//! - A second pass starts nothing. A failed resume still exits 0 (R1.8).

#![cfg(target_os = "linux")]

use crate::issue_1760_fake_proc::Proc;
use crate::issue_1760_fixture::{
    CONV_A, CONV_B, Fx, LAUNCH_ID, exe, expect_code, now, parse_ts, stdout,
};
use crate::issue_1760_fixture_files::{MESSAGE_HEAD, message_times, pane_unset};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Output;

/// Removed from the tmux client's environment (R5.6, A3); each is planted
/// on the supervisor with the value `planted`.
const CLIENT_REMOVED: &str = "TMUX TMUX_PANE CLAUDECODE CLAUDE_CODE_SESSION_ID \
     CLAUDE_CODE_ENTRYPOINT CLAUDE_PROJECT_DIR CODEX_THREAD_ID CODEX_SESSION_ID CLAUDE_CONFIG_DIR \
     AMPLIHACK_CLAUDE_PROFILE COPILOT_CLI COPILOT_AGENT_TOKEN GITHUB_COPILOT GITHUB_COPILOT_TOKEN";

/// `env`, then `-u NAME` for each `pane_unset()` name.
fn env_unset() -> Vec<String> {
    let mut v = vec!["env".to_string()];
    for name in pane_unset() {
        v.extend(["-u".to_string(), name]);
    }
    v
}

fn words(s: &str) -> Vec<String> {
    s.split_whitespace().map(String::from).collect()
}

struct Resumed {
    out: Output,
    dir: PathBuf,
    docs: PathBuf,
    before: i64,
    after: i64,
    original: Value,
}

/// A lost `coordinator` with every option recorded, a socket and profile
/// `work`; one supervise pass with the client variables planted.
fn resume_full(fx: &Fx) -> Resumed {
    let dir = fx.dir("mistt");
    let docs = fx.dir("mistt-docs");
    let profile = fx.profile_dir("work");
    fx.transcript_in(&profile, &dir, CONV_A);
    let mut e = fx.entry("coord", &dir, Some(CONV_A));
    e["tmux_socket"] = json!(fx.socket);
    e["model"] = json!("opus");
    e["effort"] = json!("high");
    e["add_dirs"] = json!([docs]);
    e["amplihack_flags"] =
        json!({ "subprocess_safe": true, "no_reflection": true, "profile": "work" });
    e["env"] = json!({ "CLAUDE_CODE_EFFORT_LEVEL": "high", "ANTHROPIC_MODEL": "claude-opus-5-5" });
    fx.write_agents(&[("coordinator", e.clone())]);
    let mut c = fx.cmd(&["agent", "supervise", "--once", "--gap", "0"]);
    for k in CLIENT_REMOVED.split_whitespace() {
        c.env(k, "planted");
    }
    let before = now();
    let out = fx.output(&mut c);
    let after = now();
    expect_code(&out, 0);
    Resumed {
        out,
        dir,
        docs,
        before,
        after,
        original: e,
    }
}

#[test]
fn a_lost_agent_is_resumed_once_with_the_full_pane_argv() {
    let fx = Fx::new();
    let r = resume_full(&fx);
    let news = fx.new_sessions();
    assert_eq!(news.len(), 1, "{news:?}");
    let n = &news[0];
    let (socket, dir) = (fx.socket.display(), r.dir.display());
    assert_eq!(
        n.argv[..11].join(" "),
        format!("-S {socket} new-session -d -s coord -n main -c {dir} --")
    );
    let pane = n.pane_argv();
    let at = pane.iter().position(|a| a == "--resume").expect("--resume");
    let msg = pane[at + 2].clone();
    let (lost, resumed) = message_times(&msg);
    for t in [&lost, &resumed] {
        let t = parse_ts(t);
        assert!(
            t >= r.before - 1 && t <= r.after + 1,
            "{t} outside the pass"
        );
    }
    let mut expected = env_unset();
    expected.extend(words(
        "ANTHROPIC_MODEL=claude-opus-5-5 CLAUDE_CODE_EFFORT_LEVEL=high",
    ));
    expected.push(exe());
    expected.extend(words(
        "claude --subprocess-safe --no-reflection --profile work --supervise coordinator -- --resume",
    ));
    expected.extend([CONV_A.to_string(), msg.clone()]);
    expected.extend(words("--model opus --effort high --add-dir"));
    expected.push(r.docs.display().to_string());
    expected.push("--dangerously-skip-permissions".to_string());
    assert_eq!(pane, expected);
    assert!(
        !pane.iter().any(|a| a == "TMUX" || a == "TMUX_PANE"),
        "D6: the pane keeps its own TMUX"
    );
}

#[test]
fn the_resume_is_recorded_and_logged() {
    let fx = Fx::new();
    let r = resume_full(&fx);
    let e = fx.agent("coordinator");
    assert_eq!(e["state"], "active");
    assert!(e.get("lost_at").is_none(), "lost_at cleared: {e}");
    assert_eq!(
        e["launch_id"], LAUNCH_ID,
        "A1: the wrapper writes launch_id"
    );
    assert_eq!(e["launched_at"], r.original["launched_at"]);
    assert_eq!(e["registered_at"], r.original["registered_at"]);
    let restarts = e["restarts"].as_array().unwrap();
    assert_eq!(restarts.len(), 1, "{e}");
    let lr = &e["last_recovery"];
    assert_eq!(lr["result"], "resumed");
    assert_eq!(lr["conversation_id"], CONV_A);
    assert_eq!(lr["resumed"], restarts[0]);
    let lost = lr["lost"].as_str().expect("lost recorded");
    let line = format!(
        "{} agent=coordinator conversation={CONV_A} lost={lost} resumed={} result=resumed",
        lr["logged_at"].as_str().unwrap(),
        lr["resumed"].as_str().unwrap()
    );
    assert_eq!(fx.log_lines(), std::slice::from_ref(&line));
    assert_eq!(stdout(&r.out), format!("{line}\n"));
    let msg = fx.new_sessions()[0]
        .pane_argv()
        .into_iter()
        .find(|a| a.starts_with(MESSAGE_HEAD));
    assert_eq!(
        message_times(&msg.unwrap()).0,
        lost,
        "the message names lost_at"
    );
}

#[test]
fn tmux_and_git_run_without_session_markers_or_real_paths() {
    let fx = Fx::new();
    resume_full(&fx);
    let calls = fx.tmux_records();
    assert!(calls.len() >= 2, "has-session and new-session: {calls:?}");
    let agents = fx.agents_dir().display().to_string();
    for c in &calls {
        for k in CLIENT_REMOVED.split_whitespace() {
            assert!(!c.env.contains_key(k), "{k} reached `tmux {}`", c.command());
        }
        assert!(
            !c.env
                .keys()
                .any(|k| k.starts_with("COPILOT_") || k.starts_with("GITHUB_COPILOT")),
            "{:?}",
            c.env.keys()
        );
        assert_eq!(c.env.get("HOME"), Some(&fx.home.display().to_string()));
        if let Some(root) = c.env.get("AMPLIHACK_CLAUDE_PROFILE_ROOT") {
            assert_eq!(root, &fx.profiles.display().to_string());
        }
        assert!(
            !c.fds.iter().any(|f| f.starts_with(&agents)),
            "S18: `tmux {}` inherited {:?}",
            c.command(),
            c.fds
        );
    }
    for c in calls.iter().filter(|c| c.command() == "has-session") {
        assert_eq!(c.socket(), Some(fx.socket.display().to_string().as_str()));
        assert_eq!(c.opt("-t"), Some("=coord"));
    }
    for (_, env) in fx.git_calls() {
        assert_eq!(env.get("HOME"), Some(&fx.home.display().to_string()));
        if let Some(root) = env.get("AMPLIHACK_CLAUDE_PROFILE_ROOT") {
            assert_eq!(root, &fx.profiles.display().to_string());
        }
    }
}

#[test]
fn a_second_pass_starts_nothing() {
    let fx = Fx::new();
    resume_full(&fx);
    // The resumed Claude is running (signal b) in the new tmux session.
    let p = Proc::new(7000, 1, &["claude", "--resume", CONV_A], &fx.work);
    fx.add_proc(&p);
    let o = fx.supervise_once(&["--gap", "0"]);
    expect_code(&o, 0);
    assert_eq!(fx.new_sessions().len(), 1);
    assert!(
        stdout(&o).is_empty(),
        "a healthy agent writes nothing: {}",
        stdout(&o)
    );
    assert_eq!(fx.log_lines().len(), 1);
    assert_eq!(
        fx.agent("coordinator")["restarts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn without_a_recorded_socket_no_s_is_passed() {
    let fx = Fx::new();
    let d = fx.dir("plain");
    fx.transcript(&d, CONV_B);
    let mut e = fx.entry("plain", &d, Some(CONV_B));
    e["skip_permissions"] = json!(false);
    e["permission_mode"] = json!("acceptEdits");
    fx.write_agents(&[("minimal", e)]);
    expect_code(&fx.supervise_once(&["--gap", "0"]), 0);
    let news = fx.new_sessions();
    assert_eq!(news.len(), 1, "{news:?}");
    assert!(fx.tmux_records().iter().all(|c| c.socket().is_none()));
    assert_eq!(news[0].command(), "new-session");
    let pane = news[0].pane_argv();
    let msg = pane
        .iter()
        .find(|a| a.starts_with(MESSAGE_HEAD))
        .unwrap()
        .clone();
    let mut expected = env_unset();
    expected.extend([
        exe(),
        "claude".into(),
        "--supervise".into(),
        "minimal".into(),
    ]);
    expected.extend(words("-- --resume"));
    expected.extend([CONV_B.to_string(), msg]);
    expected.extend(words("--permission-mode acceptEdits"));
    assert_eq!(pane, expected);
}

#[test]
fn a_failed_resume_still_completes_the_pass() {
    let fx = Fx::new();
    let d = fx.dir("d");
    fx.transcript(&d, CONV_A);
    fx.write_agents(&[("coordinator", fx.entry("coord", &d, Some(CONV_A)))]);
    fx.tmux_fail_new("fake tmux refused the session");
    let o = fx.supervise_once(&["--gap", "0"]);
    expect_code(&o, 0);
    let lines = fx.log_lines();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].ends_with(" resumed=- result=failed: tmux: fake tmux refused the session"),
        "{}",
        lines[0]
    );
    assert_eq!(stdout(&o), format!("{}\n", lines[0]));
    let e = fx.agent("coordinator");
    assert_eq!(e["restarts"], json!([]), "a failure is not a restart");
    assert!(e.get("lost_at").is_some(), "still lost: {e}");
    assert_eq!(
        e["last_recovery"]["result"],
        "failed: tmux: fake tmux refused the session"
    );
}
