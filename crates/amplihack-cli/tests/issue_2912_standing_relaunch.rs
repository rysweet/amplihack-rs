//! Issue #2912: a standing session keeps running. The in-pane wrapper
//! (`amplihack claude --supervise NAME`) relaunches the session in its own
//! pane with `claude --continue` when its process exits other than by its
//! owner, after waiting for a login with room if none has room at the exit.
//!
//! A stand-in `claude` replaces the real one and records one line of argv
//! per run. A fake `tmux` and a synthetic `/proc` come from the issue #1760
//! fixture; no real tmux server, login, rotator or `~/.claude` is touched.
//! Standing status is set through the `agent.standing` user key (or the
//! `--standing` flag) in a temporary 0700 home in every test.
//!
//! Each run of the stand-in is held until the test releases it with an exit
//! status, so the test decides when the process exits and what the rotator's
//! swap record says at that moment. A run counts as started by the wrapper
//! once the wrapper's id thread has recorded its process.

#![cfg(target_os = "linux")]

use crate::issue_1760_fake_proc::Proc;
use crate::issue_1760_fixture::*;
use crate::support::exec_stub::write_executable;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

const NAME: &str = "area-docs";
const CLAUDE_PID: u32 = 60001;

/// The stand-in session. Run `n` records its arguments, then acts on line
/// `n` of `claude.script`: `hold:N` waits for `release.<n>` and exits N,
/// `exit:N` exits N at once. Anything else exits 0.
fn install_stand_in(fx: &Fx, script: &[&str]) {
    let body = format!(
        r#"#!/bin/sh
case "$1" in --version) echo '2.1.319 (Claude Code)'; exit 0 ;; esac
d='{records}'
{{ for a in "$@"; do printf '%s ' "$a"; done; echo; }} >> "$d/claude.runs"
n=$(wc -l < "$d/claude.runs" | tr -d ' ')
verb=$(sed -n "${{n}}p" "$d/claude.script")
: > "$d/claude.started"
echo FAKE-CLAUDE-STARTED >&2
case "$verb" in
  hold:*) while [ ! -e "$d/release.$n" ]; do sleep 0.05; done
          : > "$d/exited.$n"; exit "${{verb#hold:}}" ;;
  exit:*) : > "$d/exited.$n"; exit "${{verb#exit:}}" ;;
  *) : > "$d/exited.$n"; exit 0 ;;
esac
"#,
        records = fx.records.display()
    );
    write_executable(&fx.tools.join("claude"), body, 0o755);
    fs::write(fx.records.join("claude.script"), script.join("\n") + "\n").unwrap();
}

fn runs(fx: &Fx) -> Vec<String> {
    fs::read_to_string(fx.records.join("claude.runs"))
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

fn release(fx: &Fx, n: usize) {
    fs::write(fx.records.join(format!("release.{n}")), "").unwrap();
}

fn exited(fx: &Fx, n: usize) -> bool {
    fx.records.join(format!("exited.{n}")).exists()
}

/// A SessionEnd marker for `conv` written at `at`, with `agents/` left
/// private.
///
/// The fixture's own helper creates `agents/` and `agents/ended/` with
/// `create_dir_all`, which takes the umask, and the wrapper refuses an
/// `agents/` any group or other can reach. A test that writes a marker
/// before the first launch therefore has to set the mode itself.
fn ended_marker(fx: &Fx, conv: &str, at: i64) {
    fx.marker(conv, "prompt_input_exit", at);
    chmod(&fx.agents_dir(), 0o700);
}

/// How many times `--dangerously-skip-permissions` appears in one run's
/// recorded arguments.
fn skips(run: &str) -> usize {
    run.split_whitespace()
        .filter(|a| *a == "--dangerously-skip-permissions")
        .count()
}

/// `agent.standing = <on>` in the user file of the fixture's home.
fn set_standing(fx: &Fx, on: bool) {
    let dir = fx.home.join(".amplihack");
    fs::create_dir_all(&dir).unwrap();
    chmod(&dir, 0o700);
    let f = dir.join("config.toml");
    fs::write(&f, format!("[agent]\nstanding = {on}\n")).unwrap();
    chmod(&f, 0o600);
}

/// The wrapper command in pane `%7`, with `flags` before the `--`.
fn wrapped(fx: &Fx, dir: &Path, flags: &[&str], claude_args: &[&str]) -> Command {
    fx.tmux_pane("%7", "area", "main", 4321);
    let mut args = vec!["claude", "--subprocess-safe", "--no-reflection"];
    args.extend(["--supervise", NAME]);
    args.extend(flags);
    args.push("--");
    args.extend(claude_args);
    let mut c = fx.cmd(&args);
    c.current_dir(dir)
        .env("TMUX", fx.tmux_env())
        .env("TMUX_PANE", "%7")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    c
}

/// A running wrapper; dropping it releases every run and kills it, so a
/// failed test leaves no process behind.
struct Running<'a> {
    fx: &'a Fx,
    child: Option<Child>,
}

impl<'a> Running<'a> {
    fn spawn(fx: &'a Fx, mut c: Command) -> Self {
        let child = loop {
            match c.spawn() {
                Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {}
                r => break r.expect("spawn amplihack"),
            }
        };
        Self {
            fx,
            child: Some(child),
        }
    }
    fn alive(&mut self) -> bool {
        self.child.as_mut().unwrap().try_wait().unwrap().is_none()
    }
    fn finish(mut self) -> Output {
        let mut child = self.child.take().unwrap();
        if !wait_until(|| child.try_wait().unwrap().is_some()) {
            let _ = child.kill();
            panic!("amplihack did not exit within {} s", CEILING.as_secs());
        }
        child.wait_with_output().unwrap()
    }
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        for n in 1..=16 {
            let _ = fs::write(self.fx.records.join(format!("release.{n}")), "");
        }
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn entry(fx: &Fx) -> Option<Value> {
    let raw = fs::read(fx.registry_path()).ok()?;
    let v: Value = serde_json::from_slice(&raw).ok()?;
    v["agents"].get(NAME).cloned()
}

/// Waits until run `n` is the live run and the wrapper's id thread has
/// recorded its process, which is when Claude counts as started.
fn run_is_up(fx: &Fx, n: usize) {
    let up = wait_until(|| {
        runs(fx).len() == n
            && entry(fx)
                .is_some_and(|e| e["claude_pid"] == CLAUDE_PID && e["launch_id"].is_string())
    });
    assert!(up, "run {n} never came up: {:?} {:?}", runs(fx), entry(fx));
}

/// Makes the synthetic process table show a Claude below the wrapper.
fn show_claude(fx: &Fx, dir: &Path) {
    fx.add_proc(&Proc::new(CLAUDE_PID, SELF_PID, &["claude"], dir).start(4242));
    fx.sessions_file(&fx.claude_dir, CLAUDE_PID, CONV_C, 4242, dir);
}

/// The rotator's swap record under the fixture's profile root, written the
/// way the rotator writes it: a new file renamed over the old one.
fn swap(fx: &Fx, seq: u64, session: u8, mode: u32) -> PathBuf {
    let rotate = fx.profiles.join(".auto-switch.d").join("rotate");
    fs::create_dir_all(&rotate).unwrap();
    chmod(&fx.profiles.join(".auto-switch.d"), 0o700);
    chmod(&rotate, 0o700);
    let at = now() - 5;
    let body = json!({
        "seq": seq, "live_login": "spare", "from_login": "main", "swapped_at": at,
        "reading": { "session": session, "week": 10, "measured_at": at - 5 }
    });
    let tmp = rotate.join(format!("swap.json.new.{seq}"));
    fs::write(&tmp, body.to_string()).unwrap();
    chmod(&tmp, mode);
    let dest = rotate.join("swap.json");
    fs::rename(&tmp, &dest).unwrap();
    dest
}

fn relaunch_lines(lines: &[String]) -> Vec<&String> {
    lines
        .iter()
        .filter(|l| l.contains(" standing session relaunch "))
        .collect()
}

fn waiting_lines(fx: &Fx) -> usize {
    fx.log_lines()
        .iter()
        .filter(|l| l.contains(" standing session waiting "))
        .count()
}

/// Waits until the wrapper has written its `n`th waiting line, which it
/// does only once its watch on the swap record is armed. Writing the
/// record before that point would leave the wrapper reading a record that
/// already has room, and the test would prove nothing about the wait.
fn waiting_for_room(fx: &Fx, n: usize) {
    assert!(
        wait_until(|| waiting_lines(fx) == n),
        "the wrapper never said it was waiting for room: {:?}",
        fx.log_lines()
    );
}

fn fixture(script: &[&str]) -> (Fx, PathBuf) {
    let fx = Fx::new();
    let dir = fx.dir("docs");
    install_stand_in(&fx, script);
    show_claude(&fx, &dir);
    (fx, dir)
}

#[test]
fn a_standing_session_that_exits_is_relaunched_in_its_pane_with_continue() {
    let (fx, dir) = fixture(&["hold:1", "hold:0"]);
    set_standing(&fx, true);
    let args = ["--model", "opus", "--permission-mode", "acceptEdits"];
    let w = Running::spawn(&fx, wrapped(&fx, &dir, &[], &args));
    run_is_up(&fx, 1);
    release(&fx, 1);
    run_is_up(&fx, 2);
    release(&fx, 2);
    let out = w.finish();
    expect_code(&out, 0);

    let r = runs(&fx);
    assert_eq!(r.len(), 2, "{r:?}");
    assert!(!r[0].contains("--continue"), "{}", r[0]);
    for want in [
        "--continue",
        "--model=opus",
        "--permission-mode=acceptEdits",
    ] {
        assert!(r[1].contains(want), "{want} missing from: {}", r[1]);
    }
    assert!(!r[1].contains("--resume"), "{}", r[1]);
    assert_eq!(
        skips(&r[1]),
        skips(&r[0]),
        "the relaunch has the first run's privilege, no more and no less: {r:?}"
    );
    assert!(
        fx.new_sessions().is_empty(),
        "same pane: no new tmux session: {:?}",
        fx.tmux_records()
    );
}

/// Loud: the same line goes to the pane and to the supervisor log, and no
/// argument, prompt text or environment is in it.
#[test]
fn every_relaunch_is_logged_to_the_pane_and_the_supervisor_log() {
    let (fx, dir) = fixture(&["hold:1", "hold:0"]);
    set_standing(&fx, true);
    let w = Running::spawn(&fx, wrapped(&fx, &dir, &[], &["--model", "opus"]));
    run_is_up(&fx, 1);
    release(&fx, 1);
    run_is_up(&fx, 2);
    release(&fx, 2);
    let out = w.finish();

    let log = fx.log_lines();
    let logged = relaunch_lines(&log);
    assert_eq!(logged.len(), 1, "{log:?}");
    let line = logged[0];
    assert!(
        line.ends_with(
            " standing session relaunch session=area-docs reason=exit-status-1 waited=none"
        ),
        "{line}"
    );
    parse_ts(&line[..20]);
    assert!(
        stderr(&out).contains(line.as_str()),
        "the pane shows the line: {}",
        stderr(&out)
    );
    assert!(!line.contains("opus"), "no arguments in the line: {line}");
}

#[test]
fn a_session_ended_by_its_owner_is_not_relaunched() {
    let (fx, dir) = fixture(&["hold:0", "hold:0"]);
    set_standing(&fx, true);
    let w = Running::spawn(&fx, wrapped(&fx, &dir, &[], &[]));
    run_is_up(&fx, 1);
    release(&fx, 1);
    let out = w.finish();
    expect_code(&out, 0);
    assert_eq!(runs(&fx).len(), 1, "{:?}", runs(&fx));
    assert_eq!(fx.agent(NAME)["state"], "ended");
    assert!(relaunch_lines(&fx.log_lines()).is_empty());
}

/// The ended marker is the owner's signal even when the exit status is not.
/// The SessionEnd hook writes it as the session ends, so it is written here
/// while the run is up.
#[test]
fn a_session_with_an_ended_marker_is_not_relaunched() {
    for conv in [CONV_B, CONV_C] {
        let (fx, dir) = fixture(&["hold:1", "hold:0"]);
        set_standing(&fx, true);
        let w = Running::spawn(&fx, wrapped(&fx, &dir, &[], &["--resume", CONV_B]));
        run_is_up(&fx, 1);
        ended_marker(&fx, conv, now());
        release(&fx, 1);
        let _ = w.finish();
        assert_eq!(runs(&fx).len(), 1, "{conv}: {:?}", runs(&fx));
        assert!(relaunch_lines(&fx.log_lines()).is_empty(), "{conv}");
    }
}

/// A marker older than the run is an earlier exit of the same conversation
/// that the supervisor has not deleted yet, not the owner ending this run.
#[test]
fn a_marker_from_before_the_launch_does_not_stop_the_relaunch() {
    let (fx, dir) = fixture(&["hold:1", "hold:0"]);
    set_standing(&fx, true);
    ended_marker(&fx, CONV_B, now() - 3600);
    let w = Running::spawn(&fx, wrapped(&fx, &dir, &[], &["--resume", CONV_B]));
    run_is_up(&fx, 1);
    release(&fx, 1);
    run_is_up(&fx, 2);
    release(&fx, 2);
    expect_code(&w.finish(), 0);
    assert_eq!(runs(&fx).len(), 2, "{:?}", runs(&fx));
    assert_eq!(relaunch_lines(&fx.log_lines()).len(), 1);
}

#[test]
fn a_session_that_is_not_standing_is_not_relaunched() {
    let (fx, dir) = fixture(&["hold:3", "hold:0"]);
    let w = Running::spawn(&fx, wrapped(&fx, &dir, &[], &[]));
    run_is_up(&fx, 1);
    release(&fx, 1);
    let out = w.finish();
    expect_code(&out, 3);
    assert_eq!(runs(&fx).len(), 1, "{:?}", runs(&fx));
}

#[test]
fn the_standing_flag_overrides_the_key_both_ways() {
    // Flag on, key off: relaunched.
    let (fx, dir) = fixture(&["hold:1", "hold:0"]);
    set_standing(&fx, false);
    let w = Running::spawn(&fx, wrapped(&fx, &dir, &["--standing"], &[]));
    run_is_up(&fx, 1);
    release(&fx, 1);
    run_is_up(&fx, 2);
    release(&fx, 2);
    expect_code(&w.finish(), 0);
    assert_eq!(runs(&fx).len(), 2);

    // Flag off, key on: not relaunched.
    let (fx, dir) = fixture(&["hold:1", "hold:0"]);
    set_standing(&fx, true);
    let w = Running::spawn(&fx, wrapped(&fx, &dir, &["--no-standing"], &[]));
    run_is_up(&fx, 1);
    release(&fx, 1);
    expect_code(&w.finish(), 1);
    assert_eq!(runs(&fx).len(), 1);
}

/// A session whose Claude never started has nothing to continue, and
/// relaunching it would spin.
#[test]
fn a_session_whose_claude_never_started_is_refused_loudly() {
    let fx = Fx::new();
    let dir = fx.dir("docs");
    install_stand_in(&fx, &["exit:1", "exit:0"]);
    set_standing(&fx, true);
    let out = fx.output(&mut wrapped(&fx, &dir, &[], &[]));
    expect_code(&out, 1);
    assert_eq!(runs(&fx).len(), 1, "{:?}", runs(&fx));
    let log = fx.log_lines();
    let refusal = log
        .iter()
        .find(|l| l.contains(" standing session refused "))
        .unwrap_or_else(|| panic!("no refusal line in {log:?}"));
    assert!(
        refusal.ends_with(" standing session refused session=area-docs reason=never-started"),
        "{refusal}"
    );
    assert!(stderr(&out).contains(refusal.as_str()), "{}", stderr(&out));
}

/// No login has room at the exit: nothing runs until a room event arrives,
/// and one relaunch follows that event.
#[test]
fn no_relaunch_until_a_login_has_room_then_one_on_the_event() {
    let (fx, dir) = fixture(&["hold:1", "hold:0"]);
    set_standing(&fx, true);
    swap(&fx, 1, 100, 0o600);
    let mut w = Running::spawn(&fx, wrapped(&fx, &dir, &[], &[]));
    run_is_up(&fx, 1);
    release(&fx, 1);
    assert!(wait_until(|| exited(&fx, 1)));
    waiting_for_room(&fx, 1);
    assert_eq!(runs(&fx).len(), 1, "relaunched with no room");
    assert!(w.alive(), "the wrapper waits for room");

    swap(&fx, 2, 20, 0o600);
    run_is_up(&fx, 2);
    release(&fx, 2);
    expect_code(&w.finish(), 0);
    assert_eq!(runs(&fx).len(), 2);
    let log = fx.log_lines();
    let lines = relaunch_lines(&log);
    assert_eq!(lines.len(), 1, "{log:?}");
    assert!(
        lines[0].ends_with("reason=exit-status-1 waited=room"),
        "{}",
        lines[0]
    );
}

/// A wrapper held for room has no Claude Code to pass a signal to, so the
/// signal is its own: closing the pane or pressing Ctrl-C ends it, and
/// nothing is relaunched.
#[test]
fn a_wrapper_waiting_for_room_ends_on_a_closed_pane_or_ctrl_c() {
    for sig in [libc::SIGHUP, libc::SIGTERM, libc::SIGINT] {
        let (fx, dir) = fixture(&["hold:1", "hold:0"]);
        set_standing(&fx, true);
        swap(&fx, 1, 100, 0o600);
        let w = Running::spawn(&fx, wrapped(&fx, &dir, &[], &[]));
        run_is_up(&fx, 1);
        release(&fx, 1);
        waiting_for_room(&fx, 1);
        let pid = w.child.as_ref().unwrap().id() as libc::pid_t;
        // SAFETY: `pid` is the wrapper this test spawned and has not reaped.
        assert_eq!(unsafe { libc::kill(pid, sig) }, 0, "signal {sig}");
        let out = w.finish();
        assert!(!out.status.success(), "signal {sig}: {:?}", out.status);
        assert_eq!(runs(&fx).len(), 1, "signal {sig}: {:?}", runs(&fx));
        assert!(
            relaunch_lines(&fx.log_lines()).is_empty(),
            "signal {sig}: {:?}",
            fx.log_lines()
        );
    }
}

/// Issue #2912: bb:0 was refused at its fourth limit of a day. Four limits,
/// each followed by a login with room, each lead to a relaunch.
#[test]
fn four_limits_in_one_day_each_lead_to_a_relaunch_when_room_appears() {
    let (fx, dir) = fixture(&["hold:1", "hold:1", "hold:1", "hold:1", "hold:0"]);
    set_standing(&fx, true);
    let mut w = Running::spawn(&fx, wrapped(&fx, &dir, &[], &[]));
    for n in 1..=4usize {
        run_is_up(&fx, n);
        swap(&fx, 2 * n as u64 - 1, 100, 0o600); // the limit: no room
        release(&fx, n);
        assert!(wait_until(|| exited(&fx, n)));
        waiting_for_room(&fx, n);
        assert_eq!(runs(&fx).len(), n, "limit {n}: relaunched with no room");
        assert!(w.alive());
        swap(&fx, 2 * n as u64, 15, 0o600); // room appears
    }
    run_is_up(&fx, 5);
    release(&fx, 5);
    expect_code(&w.finish(), 0);
    assert_eq!(runs(&fx).len(), 5);
    let log = fx.log_lines();
    assert_eq!(relaunch_lines(&log).len(), 4, "{log:?}");
    assert!(
        relaunch_lines(&log)
            .iter()
            .all(|l| l.ends_with("waited=room")),
        "{log:?}"
    );
}

/// With no rotator, or a record the reader refuses, no one is delayed.
#[test]
fn a_record_the_reader_refuses_does_not_hold_the_session() {
    let (fx, dir) = fixture(&["hold:1", "hold:0"]);
    set_standing(&fx, true);
    swap(&fx, 1, 100, 0o666); // group and world writable: refused
    let w = Running::spawn(&fx, wrapped(&fx, &dir, &[], &[]));
    run_is_up(&fx, 1);
    release(&fx, 1);
    run_is_up(&fx, 2);
    release(&fx, 2);
    expect_code(&w.finish(), 0);
    let log = fx.log_lines();
    let lines = relaunch_lines(&log);
    assert_eq!(lines.len(), 1, "{log:?}");
    assert!(lines[0].ends_with("waited=none"), "{}", lines[0]);
    assert_eq!(waiting_lines(&fx), 0, "nothing waited: {log:?}");
}

/// A relaunch goes through the same launch path as the run it continues,
/// so it has that run's privilege, no more and no less: an unattended
/// session that skipped permission prompts before its exit still does, and
/// one started with a permission mode keeps that mode.
#[test]
fn the_relaunch_has_the_permission_flags_of_the_run_it_continues() {
    for args in [
        &[][..],
        &["--dangerously-skip-permissions"][..],
        &["--permission-mode", "plan"][..],
    ] {
        let (fx, dir) = fixture(&["hold:1", "hold:0"]);
        set_standing(&fx, true);
        let w = Running::spawn(&fx, wrapped(&fx, &dir, &[], args));
        run_is_up(&fx, 1);
        release(&fx, 1);
        run_is_up(&fx, 2);
        release(&fx, 2);
        expect_code(&w.finish(), 0);
        let r = runs(&fx);
        assert_eq!(r.len(), 2, "{args:?}: {r:?}");
        assert!(r[1].contains("--continue"), "{}", r[1]);
        assert_eq!(skips(&r[1]), skips(&r[0]), "{args:?}: {r:?}");
        assert_eq!(
            r[1].contains("--permission-mode=plan"),
            r[0].contains("--permission-mode plan"),
            "{args:?}: {r:?}"
        );
    }
}

/// Changes this session's registry entry the way the supervisor would.
fn edit_entry(fx: &Fx, edit: impl FnOnce(&mut Value)) {
    let mut reg = fx.registry();
    edit(&mut reg["agents"][NAME]);
    fx.write_registry(&reg);
}

/// The supervisor's pass reads the SessionEnd marker, ends the entry and
/// deletes the marker. A wrapper that arrives after that deletion still
/// sees an owner-ended session and does not start it again.
#[test]
fn a_session_the_supervisor_has_already_ended_is_not_relaunched() {
    let (fx, dir) = fixture(&["hold:1", "hold:0"]);
    set_standing(&fx, true);
    let w = Running::spawn(&fx, wrapped(&fx, &dir, &[], &[]));
    run_is_up(&fx, 1);
    edit_entry(&fx, |e| {
        e["state"] = json!("ended");
        e["ended_by"] = json!("session-end");
        e["ended_at"] = json!(ts(now()));
    });
    release(&fx, 1);
    expect_code(&w.finish(), 1);
    assert_eq!(runs(&fx).len(), 1, "{:?}", runs(&fx));
    let log = fx.log_lines();
    assert!(
        !log.iter().any(|l| l.contains(" standing session ")),
        "an owner-ended session is neither relaunched nor refused: {log:?}"
    );
    assert_eq!(fx.agent(NAME)["state"], "ended");
}

/// One change the supervisor may make to the entry, named for the message.
type Edit = fn(&mut Value);

/// While the wrapper waits for room, the supervisor may rebuild the session
/// in another window (a new launch id) or stop it. Either way the record is
/// no longer this wrapper's to take, and one refusal is logged instead of a
/// second Claude Code on the same conversation.
#[test]
fn a_session_changed_during_the_wait_is_refused_as_superseded() {
    let edits: [(&str, Edit); 2] = [
        ("rebuilt", |e| e["launch_id"] = json!(LAUNCH_ID)),
        ("stopped", |e| {
            e["state"] = json!("stopped");
            e["alert"] = json!("stopped by the test");
        }),
    ];
    for (what, edit) in edits {
        let (fx, dir) = fixture(&["hold:1", "hold:0"]);
        set_standing(&fx, true);
        swap(&fx, 1, 100, 0o600);
        let w = Running::spawn(&fx, wrapped(&fx, &dir, &[], &[]));
        run_is_up(&fx, 1);
        release(&fx, 1);
        waiting_for_room(&fx, 1);
        edit_entry(&fx, edit);
        swap(&fx, 2, 20, 0o600);
        expect_code(&w.finish(), 1);
        assert_eq!(runs(&fx).len(), 1, "{what}: {:?}", runs(&fx));
        let log = fx.log_lines();
        assert!(relaunch_lines(&log).is_empty(), "{what}: {log:?}");
        let refused: Vec<_> = log
            .iter()
            .filter(|l| l.contains(" standing session refused "))
            .collect();
        assert_eq!(refused.len(), 1, "{what}: {log:?}");
        assert!(
            refused[0].ends_with(" session=area-docs reason=superseded"),
            "{what}: {}",
            refused[0]
        );
    }
}
