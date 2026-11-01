//! Issue #1760, R6.1, R6.2 and R9.2(3), wrapper path: `amplihack claude
//! --supervise NAME` records the session before the launch and marks it
//! `ended` only after a clean return with no SIGHUP or SIGTERM (D1). A lost
//! terminal (SIGHUP) or a non-zero Claude exit leaves it `active`, and the
//! next pass resumes it. A `stopped` entry becomes `active` with its restart
//! history kept (D3). The id comes from `--resume` at once, never under
//! `--fork-session` (B3), or later from the id thread (D4). Nothing is
//! written to the terminal while Claude runs (B4).

#![cfg(target_os = "linux")]

use crate::issue_1760_fake_proc::Proc;
use crate::issue_1760_fixture::*;
use serde_json::{Value, json};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};

const NAME: &str = "area-docs";
/// R2.4: the only variables that may be recorded.
const ALLOWLIST: &str = "ANTHROPIC_MODEL ANTHROPIC_SMALL_FAST_MODEL ANTHROPIC_DEFAULT_OPUS_MODEL \
    ANTHROPIC_DEFAULT_SONNET_MODEL ANTHROPIC_DEFAULT_HAIKU_MODEL CLAUDE_CODE_SUBAGENT_MODEL \
    CLAUDE_CODE_EFFORT_LEVEL CLAUDE_CODE_MAX_OUTPUT_TOKENS MAX_THINKING_TOKENS \
    AMPLIHACK_DEFAULT_MODEL AMPLIHACK_AGENT_BINARY";

/// `amplihack claude --subprocess-safe --no-reflection --supervise NAME --
/// <args>` in `dir`, inside pane `%7` of session `area`, window `main`, on
/// the fixture's socket.
fn wrapped(fx: &Fx, dir: &Path, claude_args: &[&str]) -> Command {
    fx.tmux_pane("%7", "area", "main", 4321);
    let mut args = vec!["claude", "--subprocess-safe", "--no-reflection"];
    args.extend(["--supervise", NAME, "--"]);
    args.extend(claude_args);
    let mut c = fx.cmd(&args);
    c.current_dir(dir)
        .env("TMUX", fx.tmux_env())
        .env("TMUX_PANE", "%7");
    c
}

/// A running wrapper. Dropping it releases the fake claude and kills the
/// wrapper, so a failed test leaves no process behind.
struct Running<'a> {
    fx: &'a Fx,
    child: Option<Child>,
}

impl<'a> Running<'a> {
    fn spawn(fx: &'a Fx, mut c: Command) -> Self {
        c.stdout(Stdio::piped()).stderr(Stdio::piped());
        for _ in 0..50 {
            match c.spawn() {
                Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                r => {
                    let child = Some(r.expect("spawn amplihack"));
                    return Self { fx, child };
                }
            }
        }
        panic!("the fake claude stayed busy");
    }
    fn pid(&self) -> i32 {
        self.child.as_ref().unwrap().id() as i32
    }
    /// Waits until the fake claude has started, or the wrapper has exited.
    fn started(&mut self) -> bool {
        let (fx, child) = (self.fx, self.child.as_mut().unwrap());
        wait_until(30, || {
            fx.claude_started() || child.try_wait().unwrap().is_some()
        });
        fx.claude_started()
    }
    fn finish(mut self, secs: u64) -> Output {
        let mut child = self.child.take().unwrap();
        if !wait_until(secs, || child.try_wait().unwrap().is_some()) {
            let _ = child.kill();
            panic!("amplihack did not exit within {secs} s");
        }
        child.wait_with_output().unwrap()
    }
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.fx.claude_release();
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// The entry, or `None` while the registry does not hold it yet.
fn try_agent(fx: &Fx, name: &str) -> Option<Value> {
    let raw = std::fs::read(fx.registry_path()).ok()?;
    let v: Value = serde_json::from_slice(&raw).ok()?;
    v["agents"].get(name).cloned()
}

fn absent(e: &Value, key: &str) -> bool {
    e.get(key).is_none_or(Value::is_null)
}

#[test]
fn a_clean_exit_records_the_session_and_ends_it() {
    let fx = Fx::new();
    let dir = fx.dir("docs");
    fx.claude_mode("exit:0");
    let args = ["--model", "opus", "--effort=high", "--add-dir", "notes"];
    let mut c = wrapped(&fx, &dir, &args);
    c.args(["--permission-mode", "acceptEdits"])
        .env("CLAUDE_CODE_EFFORT_LEVEL", "high")
        .env("ANTHROPIC_API_KEY", "sk-planted-1760")
        .env("ANTHROPIC_BASE_URL", "https://planted-1760.invalid")
        .env("GH_TOKEN", "gh-planted-1760");
    let out = fx.output(&mut c);
    expect_code(&out, 0);
    assert!(fx.claude_started(), "{}", stderr(&out));
    let e = fx.agent(NAME);
    let want = json!({
        "state": "ended", "ended_by": "wrapper", "tmux_socket": fx.socket,
        "tmux_session": "area", "tmux_window": "main", "launch_dir": dir,
        "model": "opus", "effort": "high", "add_dirs": [dir.join("notes")],
        "permission_mode": "acceptEdits", "skip_permissions": false,
        "amplihack_flags": { "subprocess_safe": true, "no_reflection": true }
    });
    for (k, v) in want.as_object().unwrap() {
        assert_eq!(&e[k], v, "{k}: {e}");
    }
    parse_ts(e["ended_at"].as_str().expect("ended_at"));
    assert!(absent(&e, "conversation_id"), "{e}");
    let id = e["launch_id"].as_str().unwrap();
    assert!(
        id.len() == 32 && id.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
        "{id}"
    );
    assert_eq!(e["env"]["CLAUDE_CODE_EFFORT_LEVEL"], "high");
    for k in e["env"].as_object().unwrap().keys() {
        assert!(ALLOWLIST.split_whitespace().any(|a| a == k), "{k} recorded");
    }
    let raw = String::from_utf8_lossy(&fx.registry_raw()).into_owned();
    assert!(
        !raw.contains("planted-1760"),
        "a credential was recorded:\n{raw}"
    );

    let argv = fx.claude_argv();
    assert!(
        !argv.iter().any(|a| a == "--supervise" || a == NAME),
        "{argv:?}"
    );
    assert!(
        argv.windows(2).any(|w| w == ["--model", "opus"]),
        "{argv:?}"
    );
    assert_eq!(
        fx.claude_env().get("HOME"),
        Some(&fx.home.display().to_string())
    );

    let disp = fx
        .tmux_records()
        .into_iter()
        .find(|c| c.command() == "display-message");
    let disp = disp.expect("tmux was asked for the pane's names");
    assert_eq!(disp.socket(), fx.socket.to_str());
    assert_eq!(disp.opt("-t"), Some("%7"));
    assert!(!disp.env.contains_key("TMUX") && !disp.env.contains_key("TMUX_PANE"));

    // B4: nothing from the wrapper once Claude owns the terminal.
    let err = stderr(&out);
    let after = err
        .split_once("FAKE-CLAUDE-STARTED")
        .map(|(_, a)| a)
        .unwrap_or("");
    assert!(!after.to_lowercase().contains("supervis"), "{after}");

    expect_code(&fx.supervise_once(&["--gap", "0"]), 0);
    assert!(fx.new_sessions().is_empty() && fx.log_lines().is_empty());
}

#[test]
fn a_lost_terminal_leaves_the_agent_active_and_the_next_pass_resumes_it() {
    let fx = Fx::new();
    let dir = fx.dir("docs");
    fx.transcript(&dir, CONV_A);
    fx.claude_mode("wait:0");
    let mut w = Running::spawn(&fx, wrapped(&fx, &dir, &["--resume", CONV_A]));
    assert!(w.started(), "the fake claude never started");
    // SAFETY: kill has no memory-safety preconditions.
    assert_eq!(unsafe { libc::kill(w.pid(), libc::SIGHUP) }, 0);
    w.finish(30);
    let e = fx.agent(NAME);
    assert_eq!(e["state"], "active", "{e}");
    assert!(absent(&e, "ended_at") && absent(&e, "ended_by"), "{e}");
    assert_eq!(e["conversation_id"], CONV_A);

    expect_code(&fx.supervise_once(&["--gap", "0"]), 0);
    let started = fx.new_sessions();
    assert_eq!(started.len(), 1, "{started:?}");
    let argv = started[0].pane_argv();
    assert!(
        argv.windows(2).any(|w| w == ["--supervise", NAME]),
        "{argv:?}"
    );
    assert!(
        argv.windows(2).any(|w| w == ["--resume", CONV_A]),
        "{argv:?}"
    );
    let last = fx.log_lines().pop().expect("a resume line");
    assert!(last.ends_with(" result=resumed"), "{last}");
}

#[test]
fn a_non_zero_claude_exit_leaves_the_agent_active() {
    let fx = Fx::new();
    let dir = fx.dir("docs");
    fx.claude_mode("exit:3");
    let out = fx.output(&mut wrapped(&fx, &dir, &["--resume", CONV_B]));
    expect_code(&out, 3);
    let e = fx.agent(NAME);
    assert_eq!(e["state"], "active", "{e}");
    assert!(absent(&e, "ended_by"), "{e}");
    assert_eq!(
        e["conversation_id"], CONV_B,
        "--resume is recorded at the start"
    );
}

/// B3: a forked session writes a new conversation; `--resume` names its parent.
#[test]
fn fork_session_records_no_id_from_resume() {
    let fx = Fx::new();
    let dir = fx.dir("docs");
    fx.claude_mode("exit:3");
    let out = fx.output(&mut wrapped(
        &fx,
        &dir,
        &["--resume", CONV_B, "--fork-session"],
    ));
    expect_code(&out, 3);
    assert!(absent(&fx.agent(NAME), "conversation_id"));
}

/// D3: the wrapper reactivates a stopped entry in place.
#[test]
fn a_stopped_entry_becomes_active_and_keeps_its_history() {
    let fx = Fx::new();
    let dir = fx.dir("docs");
    let mut e = fx.entry("area", &dir, Some(CONV_A));
    let restarts = json!([ts(now() - 600)]);
    let recovery = json!({ "logged_at": ts(now() - 60), "conversation_id": CONV_A,
        "lost": ts(now() - 61), "result": "stopped: restart limit" });
    e["state"] = json!("stopped");
    e["alert"] = json!("stopped: restart limit (1 restarts in the last hour)");
    e["restarts"] = restarts.clone();
    e["last_recovery"] = recovery.clone();
    let registered = e["registered_at"].clone();
    fx.write_agents(&[(NAME, e)]);
    fx.claude_mode("exit:3");
    expect_code(
        &fx.output(&mut wrapped(&fx, &dir, &["--resume", CONV_A])),
        3,
    );
    let e = fx.agent(NAME);
    assert_eq!(e["state"], "active", "{e}");
    assert!(absent(&e, "alert"), "{e}");
    assert_eq!(e["restarts"], restarts);
    assert_eq!(e["last_recovery"], recovery);
    assert_eq!(e["registered_at"], registered);
    assert_ne!(e["launch_id"], LAUNCH_ID);
    assert!(parse_ts(e["launched_at"].as_str().unwrap()) >= now() - 60);
}

/// D4: without `--resume`, the id thread finds the Claude child and its
/// sessions file while Claude runs.
#[test]
fn the_id_thread_records_the_claude_process_and_its_conversation() {
    let fx = Fx::new();
    let dir = fx.dir("docs");
    fx.claude_mode("wait:0");
    let mut w = Running::spawn(&fx, wrapped(&fx, &dir, &["--model", "opus"]));
    assert!(w.started(), "the fake claude never started");
    fx.add_proc(&Proc::new(60001, SELF_PID, &["claude", "--model", "opus"], &dir).start(4242));
    fx.sessions_file(&fx.claude_dir, 60001, CONV_C, 4242, &dir);
    let found = wait_until(15, || {
        try_agent(&fx, NAME).is_some_and(|e| {
            e["conversation_id"] == CONV_C && e["claude_pid"] == 60001 && e["proc_start"] == 4242
        })
    });
    assert!(found, "{:?}", try_agent(&fx, NAME));
    fx.claude_release();
    let out = w.finish(30);
    expect_code(&out, 0);
    assert_eq!(fx.agent(NAME)["state"], "ended");
}
