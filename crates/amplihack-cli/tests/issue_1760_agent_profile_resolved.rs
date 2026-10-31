//! Issue #1760, crusty round 2: the profile a launch actually ran under is
//! the one recorded, not only an explicit `--profile`. A launch without
//! `--profile` may still run in a profile directory, chosen by the default
//! pointer or by an inherited `AMPLIHACK_CLAUDE_PROFILE`. The wrapper's id
//! thread and `agent register` take the profile from the config directory
//! whose `sessions/` holds the process's file, so the pre-check and the pane
//! command both use the directory that holds the conversation, and the pane
//! passes `--profile NAME`.
//!
//! Crusty round 3: the same holds with an explicit `--profile`. A
//! `--profile NAME` naming the rotator's live login runs against
//! `~/.claude`, so no profile is recorded, and the resume still finds the
//! conversation there after the rotator has moved on.
//!
//! The pre-check resolves a missing profile the way the pane's launch does:
//! the pane removes `AMPLIHACK_CLAUDE_PROFILE` and passes no `--profile`, so
//! the launch reads the default pointer, and so does the check.

#![cfg(target_os = "linux")]

use crate::issue_1760_fake_proc::Proc;
use crate::issue_1760_fixture::*;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

const NAME: &str = "area-docs";
const CLAUDE_PID: u32 = 60001;
const START: u64 = 4242;

/// A profile directory with its own login, so the launch accepts it and it
/// cannot be the rotator's live profile.
fn logged_in_profile(fx: &Fx, name: &str) -> PathBuf {
    let d = fx.profile_dir(name);
    fs::write(d.join(".credentials.json"), "{}").unwrap();
    chmod(&d.join(".credentials.json"), 0o600);
    d
}

/// `<root>/.default`, the default-profile pointer.
fn set_pointer(fx: &Fx, name: &str) {
    let p = fx.profiles.join(".default");
    fs::write(&p, format!("{name}\n")).unwrap();
    chmod(&p, 0o600);
}

/// `<root>/.rotator/ledger.json` naming `live` (or nobody, for `None`) as
/// the rotator's live login, and a login in `~/.claude`.
fn set_live(fx: &Fx, live: Option<&str>) {
    let creds = fx.claude_dir.join(".credentials.json");
    fs::write(&creds, "{}").unwrap();
    chmod(&creds, 0o600);
    let d = fx.profiles.join(".rotator");
    fs::create_dir_all(&d).unwrap();
    chmod(&d, 0o700);
    let f = d.join("ledger.json");
    fs::write(&f, json!({ "live": live, "since": now() }).to_string()).unwrap();
    chmod(&f, 0o600);
}

/// `amplihack claude --supervise NAME` with no `--profile`, in pane `%7`.
fn wrapped(fx: &Fx, dir: &Path) -> Command {
    wrapped_with(fx, dir, &[])
}

/// `amplihack claude PRE --supervise NAME`, in pane `%7`.
fn wrapped_with(fx: &Fx, dir: &Path, pre: &[&str]) -> Command {
    fx.tmux_pane("%7", "area", "main", 4321);
    let mut args = vec!["claude"];
    args.extend(pre);
    args.extend(["--supervise", NAME, "--", "--model", "opus"]);
    let mut c = fx.cmd(&args);
    c.current_dir(dir)
        .env("TMUX", fx.tmux_env())
        .env("TMUX_PANE", "%7")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    c
}

fn try_agent(fx: &Fx) -> Option<Value> {
    let raw = fs::read(fx.registry_path()).ok()?;
    let v: Value = serde_json::from_slice(&raw).ok()?;
    v["agents"].get(NAME).cloned()
}

/// Kills the wrapper with SIGHUP (a lost terminal) and waits for it.
fn hang_up(fx: &Fx, mut child: Child) {
    // SAFETY: kill has no memory-safety preconditions.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGHUP) }, 0);
    fx.claude_release();
    if !wait_until(30, || child.try_wait().unwrap().is_some()) {
        let _ = child.kill();
        panic!("amplihack did not exit after SIGHUP");
    }
}

/// Runs the wrapper until the id thread has recorded the conversation found
/// in `config`, then loses its terminal. Returns the recorded entry.
fn launch_and_lose(fx: &Fx, dir: &Path, config: &Path, c: Command) -> Value {
    fx.claude_mode("wait:0");
    let mut c = c;
    let mut child = c.spawn().expect("spawn amplihack");
    let started = wait_until(30, || {
        fx.claude_started() || child.try_wait().unwrap().is_some()
    });
    if !(started && fx.claude_started()) {
        let _ = child.kill();
        let out = child.wait_with_output().unwrap();
        panic!("the fake claude never started: {}", stderr(&out));
    }
    let argv = ["claude", "--model", "opus"];
    fx.add_proc(&Proc::new(CLAUDE_PID, SELF_PID, &argv, dir).start(START));
    fx.sessions_file(config, CLAUDE_PID, CONV_C, START, dir);
    let found = wait_until(15, || {
        try_agent(fx).is_some_and(|e| e["conversation_id"] == CONV_C)
    });
    if !found {
        let _ = child.kill();
        panic!("the id thread recorded nothing: {:?}", try_agent(fx));
    }
    hang_up(fx, child);
    // The terminal was lost: the Claude process is gone with it.
    fx.remove_proc(CLAUDE_PID);
    let e = fx.agent(NAME);
    assert_eq!(e["state"], "active", "{e}");
    e
}

/// One pass resumes the agent once, with `--profile work` in the pane.
fn expect_one_resume_with_profile(fx: &Fx, profile: &str) {
    expect_code(&fx.supervise_once(&["--gap", "0"]), 0);
    let last = fx.log_lines().pop().expect("a recovery line");
    assert!(last.ends_with(" result=resumed"), "{last}");
    let started = fx.new_sessions();
    assert_eq!(started.len(), 1, "{started:?}");
    let argv = started[0].pane_argv();
    assert!(
        argv.windows(2).any(|w| w == ["--profile", profile]),
        "{argv:?}"
    );
    assert!(
        argv.windows(2).any(|w| w == ["--resume", CONV_C]),
        "{argv:?}"
    );
}

#[test]
fn a_launch_under_the_default_pointer_records_and_resumes_its_profile() {
    let fx = Fx::new();
    let dir = fx.dir("docs");
    let work = logged_in_profile(&fx, "work");
    set_pointer(&fx, "work");
    fx.transcript_in(&work, &dir, CONV_C);
    let e = launch_and_lose(&fx, &dir, &work, wrapped(&fx, &dir));
    assert_eq!(e["amplihack_flags"]["profile"], "work", "{e}");
    expect_one_resume_with_profile(&fx, "work");
}

#[test]
fn a_launch_under_an_inherited_profile_records_and_resumes_it() {
    let fx = Fx::new();
    let dir = fx.dir("docs");
    let rotated = logged_in_profile(&fx, "rotated");
    fx.transcript_in(&rotated, &dir, CONV_C);
    let mut c = wrapped(&fx, &dir);
    c.env("AMPLIHACK_CLAUDE_PROFILE", "rotated");
    let e = launch_and_lose(&fx, &dir, &rotated, c);
    assert_eq!(e["amplihack_flags"]["profile"], "rotated", "{e}");
    expect_one_resume_with_profile(&fx, "rotated");
}

/// `agent register` with no `--profile` anywhere: the profile comes from the
/// directory that holds the process's sessions file.
#[test]
fn register_records_the_profile_whose_directory_holds_the_sessions_file() {
    let fx = Fx::new();
    let dir = fx.dir("mistt");
    let work = logged_in_profile(&fx, "work");
    set_pointer(&fx, "work");
    let claude = Proc::new(5000, 1, &["claude"], &dir)
        .start(START)
        .env("TMUX", &fx.tmux_env())
        .env("TMUX_PANE", "%3");
    fx.add_proc(&claude);
    fx.set_self_parent(5000);
    fx.tmux_pane("%3", "coord", "main", 5000);
    fx.sessions_file(&work, 5000, CONV_C, START, &dir);
    expect_code(&fx.run(&["agent", "register", "coordinator"]), 0);
    let e = fx.agent("coordinator");
    assert_eq!(e["conversation_id"], CONV_C);
    assert_eq!(
        e["amplihack_flags"],
        json!({ "subprocess_safe": false, "no_reflection": false, "profile": "work" })
    );
}

/// With no recorded profile the pane's launch reads the default pointer,
/// so the pre-check looks in the pointer's profile, not in `~/.claude`.
#[test]
fn with_no_recorded_profile_the_precheck_follows_the_pointer() {
    let fx = Fx::new();
    let dir = fx.dir("docs");
    let work = logged_in_profile(&fx, "work");
    fx.transcript_in(&work, &dir, CONV_A);
    fx.write_agents(&[("coord", fx.entry("coord", &dir, Some(CONV_A)))]);
    expect_code(&fx.supervise_once(&["--gap", "0"]), 0);
    let last = fx.log_lines().pop().expect("a recovery line");
    assert!(last.ends_with(" result=failed: no transcript"), "{last}");
    assert!(fx.new_sessions().is_empty());

    set_pointer(&fx, "work");
    expect_code(&fx.supervise_once(&["--gap", "0"]), 0);
    let last = fx.log_lines().pop().expect("a recovery line");
    assert!(last.ends_with(" result=resumed"), "{last}");
    assert_eq!(fx.new_sessions().len(), 1);
}

/// Crusty round 3: `--profile x` while x is the rotator's live login runs
/// against `~/.claude`, so the conversation is there and no profile is
/// recorded. After the rotator moves on and x has its own login back, the
/// resume and its pre-check still use `~/.claude`, not x's directory.
#[test]
fn an_explicit_live_profile_records_none_and_resumes_after_rotation() {
    let fx = Fx::new();
    let dir = fx.dir("docs");
    fx.profile_dir("x");
    set_live(&fx, Some("x"));
    fx.transcript(&dir, CONV_C);
    let claude_dir = fx.claude_dir.clone();
    let c = wrapped_with(&fx, &dir, &["--profile", "x"]);
    let e = launch_and_lose(&fx, &dir, &claude_dir, c);
    assert!(e["amplihack_flags"]["profile"].is_null(), "{e}");

    // The rotator moves on: nobody is live and x has its own login again.
    set_live(&fx, None);
    let creds = fx.profiles.join("x").join(".credentials.json");
    fs::write(&creds, "{}").unwrap();
    chmod(&creds, 0o600);

    expect_code(&fx.supervise_once(&["--gap", "0"]), 0);
    let last = fx.log_lines().pop().expect("a recovery line");
    assert!(last.ends_with(" result=resumed"), "{last}");
    let started = fx.new_sessions();
    assert_eq!(started.len(), 1, "{started:?}");
    let argv = started[0].pane_argv();
    assert!(!argv.iter().any(|a| a == "--profile"), "{argv:?}");
    assert!(
        argv.windows(2).any(|w| w == ["--resume", CONV_C]),
        "{argv:?}"
    );
}

/// `agent register --profile x` whose process's sessions file is in
/// `~/.claude` records no profile and says why: the conversation is there.
#[test]
fn register_with_a_profile_records_where_the_sessions_file_is() {
    let fx = Fx::new();
    let dir = fx.dir("mistt");
    logged_in_profile(&fx, "x");
    let claude = Proc::new(5000, 1, &["claude"], &dir)
        .start(START)
        .env("TMUX", &fx.tmux_env())
        .env("TMUX_PANE", "%3");
    fx.add_proc(&claude);
    fx.set_self_parent(5000);
    fx.tmux_pane("%3", "coord", "main", 5000);
    let claude_dir = fx.claude_dir.clone();
    fx.sessions_file(&claude_dir, 5000, CONV_C, START, &dir);
    let o = fx.run(&["agent", "register", "coordinator", "--profile", "x"]);
    expect_code(&o, 0);
    assert!(
        stderr(&o).contains("note: --profile x not recorded; the conversation is in ~/.claude"),
        "{}",
        stderr(&o)
    );
    let e = fx.agent("coordinator");
    assert_eq!(e["conversation_id"], CONV_C);
    assert!(e["amplihack_flags"]["profile"].is_null(), "{e}");
}
