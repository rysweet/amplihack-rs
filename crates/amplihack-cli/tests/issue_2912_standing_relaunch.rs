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
//!
//! A run that is meant to have done something appends a turn the model
//! answered to its conversation's transcript, as a real session does on
//! every turn. Whether a run holds such a reply goes on the session's record
//! with the relaunch, so a stand-in that writes no reply is a session that
//! started and crashed with nothing for `--continue` to carry on, however
//! much else it writes. The lines it writes are the fixtures in
//! `src/commands/agent/testdata/issue_2912/`, recorded from a real Claude
//! Code; see docs/howto/keep-sessions-running.md.
//!
//! The wrapper has no brake. Every process exit nobody chose is relaunched,
//! and a crash loop runs until the script ends with a clean exit, which is
//! the owner ending the session, or until the supervisor takes the record
//! away. What a crash loop leaves is evidence on the record for the
//! supervisor agent, whose bundled `supervise-agent-sessions` skill has a
//! row for every alert these tests see (`issue_2912_supervisor_skill.rs`).

#![cfg(target_os = "linux")]

use crate::issue_1760_fake_proc::Proc;
use crate::issue_1760_fixture::*;
use crate::issue_2912_supervisor_skill::skill_row_for;
use crate::support::exec_stub::write_executable;
use crate::support::wait::spawn_once;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

const NAME: &str = "area-docs";
const CLAUDE_PID: u32 = 60001;

/// The stand-in session. Run `n` records its arguments, then acts on line
/// `n` of `claude.script`:
///
/// - `hold:N` appends a turn the model answered to its conversation's
///   transcript, waits for `release.<n>` and exits N: a run that did
///   something.
/// - `boot:N` appends what a real start writes with no turn behind it
///   (`bridge-session` and `cost-state`), waits for `release.<n>` and exits
///   N: a start that grows the transcript and does no work.
/// - `fail:N` appends a turn whose request failed (the prompt and Claude
///   Code's own `<synthetic>` assistant line), waits and exits N.
/// - `copy:N` appends a copy of everything already in the transcript, as a
///   resume under a new id copies the history, waits and exits N.
/// - `wait:N` waits for `release.<n>` and exits N, having written nothing:
///   a run the wrapper certainly saw, since the test can wait for its
///   process to be recorded before releasing it, and which still leaves
///   nothing for `--continue` to carry on.
/// - `exit:N` exits N at once, having written nothing: a run that started
///   and crashed before anything could be recorded of it.
///
/// Anything else exits 0.
fn install_stand_in(fx: &Fx, dir: &Path, script: &[&str]) {
    let body = format!(
        r#"#!/bin/sh
case "$1" in --version) echo '2.1.319 (Claude Code)'; exit 0 ;; esac
d='{records}'
t='{transcript}'
f='{fixtures}'
# The turn's time to the millisecond, as `%N` cut to three digits. Not
# `%3N`: uutils coreutils 0.12 drops the leading zeros before it applies
# the width, so at 0.095 s it prints 952 and dates the turn up to 0.9 s
# ahead, after the next run's launch, which reads as a reply to that run.
now=$(date -u +%Y-%m-%dT%H:%M:%S.%N)
now="${{now%??????}}Z"
{{ for a in "$@"; do printf '%s ' "$a"; done; echo; }} >> "$d/claude.runs"
n=$(wc -l < "$d/claude.runs" | tr -d ' ')
verb=$(sed -n "${{n}}p" "$d/claude.script")
: > "$d/claude.started"
echo FAKE-CLAUDE-STARTED >&2
mkdir -p "$(dirname "$t")"
case "$verb" in
  hold:*) sed "s/@NOW@/$now/g" "$f/model_reply_turn.jsonl" >> "$t" ;;
  boot:*) cat "$f/start_without_a_turn.jsonl" >> "$t" ;;
  fail:*) sed "s/@NOW@/$now/g" "$f/failed_turn.jsonl" >> "$t" ;;
  copy:*) if [ -f "$t" ]; then cat "$t" > "$d/copy.$n"; cat "$d/copy.$n" >> "$t"; fi ;;
esac
case "$verb" in
  hold:*|boot:*|fail:*|copy:*)
          while [ ! -e "$d/release.$n" ]; do sleep 0.05; done
          : > "$d/exited.$n"; exit "${{verb#*:}}" ;;
  wait:*) while [ ! -e "$d/release.$n" ]; do sleep 0.05; done
          : > "$d/exited.$n"; exit "${{verb#wait:}}" ;;
  exit:*) : > "$d/exited.$n"; exit "${{verb#exit:}}" ;;
  *) : > "$d/exited.$n"; exit 0 ;;
esac
"#,
        records = fx.records.display(),
        transcript = transcript(fx, dir).display(),
        fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/commands/agent/testdata/issue_2912")
            .display()
    );
    write_executable(&fx.tools.join("claude"), body, 0o755);
    fs::write(fx.records.join("claude.script"), script.join("\n") + "\n").unwrap();
}

/// `~/.claude/projects/<launch dir>/<CONV_C>.jsonl`: the transcript of the
/// conversation the stand-in's sessions file names, which is where
/// `claude --continue` would read from and where the stand-in writes.
///
/// It is not created here. The stand-in creates it when it runs, so a run
/// that writes nothing leaves no transcript at all, and the first run of a
/// session starts from nothing as a real one does.
fn transcript(fx: &Fx, dir: &Path) -> PathBuf {
    let enc = dir.display().to_string().replace(['/', '.'], "-");
    fx.claude_dir
        .join("projects")
        .join(enc)
        .join(format!("{CONV_C}.jsonl"))
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
        Self {
            fx,
            child: Some(spawn_once("amplihack", &mut c)),
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
    install_stand_in(&fx, &dir, script);
    show_claude(&fx, &dir);
    (fx, dir)
}

/// The session record's `alert`: the record of the last relaunch, or the
/// reason one was refused. Empty when there is none.
fn alert(fx: &Fx) -> String {
    entry(fx)
        .and_then(|e| e["alert"].as_str().map(String::from))
        .unwrap_or_default()
}

/// `text` with every `YYYY-MM-DDTHH:MM:SSZ` replaced by `<ts>`, and the
/// timestamps in the order they appeared, each checked to parse.
fn untimed(text: &str) -> (String, Vec<String>) {
    let shape = |w: &[u8]| {
        w.len() == 20
            && w.iter().enumerate().all(|(i, &c)| match i {
                4 | 7 => c == b'-',
                10 => c == b'T',
                13 | 16 => c == b':',
                19 => c == b'Z',
                _ => c.is_ascii_digit(),
            })
    };
    let bytes = text.as_bytes();
    let (mut out, mut found, mut i) = (String::new(), Vec::new(), 0);
    while i < bytes.len() {
        if i + 20 <= bytes.len() && shape(&bytes[i..i + 20]) {
            let ts = &text[i..i + 20];
            parse_ts(ts);
            found.push(ts.to_string());
            out.push_str("<ts>");
            i += 20;
        } else {
            let c = text[i..].chars().next().unwrap();
            out.push(c);
            i += c.len_utf8();
        }
    }
    (out, found)
}

/// The record a relaunch leaves after an exit with `status` and `chain`
/// relaunches since the model last replied, timestamps left as `<ts>`.
fn quiet_note(status: u8, chain: u32) -> String {
    let count = match chain {
        1 => "1 relaunch".to_string(),
        n => format!("{n} relaunches"),
    };
    format!(
        "relaunched in its pane at <ts> after it exited exit-status-{status}; \
         {count} since <ts> without a reply from the model in its conversation"
    )
}

fn no_refusal(fx: &Fx) {
    let log = fx.log_lines();
    assert!(
        !log.iter().any(|l| l.contains(" standing session refused ")),
        "{log:?}"
    );
}

/// What one pass of the supervisor service does with a session a refusal
/// left `active`. The pane the service starts runs the wrapper as its
/// command, so the pane closed when the wrapper returned, and the fake tmux
/// has no such session; the run's Claude Code has gone from the process
/// table. The pass resumes the session's own conversation in a new pane,
/// which is the recovery the refusal leaves the session to.
fn one_supervisor_pass_resumes_it(fx: &Fx) {
    fx.remove_proc(CLAUDE_PID);
    let pass = fx.supervise_once(&["--gap", "0"]);
    expect_code(&pass, 0);
    let log = fx.log_lines();
    let resumed: Vec<_> = log
        .iter()
        .filter(|l| l.contains(&format!(" agent={NAME} ")) && l.ends_with(" result=resumed"))
        .collect();
    assert_eq!(resumed.len(), 1, "{log:?}");
    let news = fx.new_sessions();
    assert_eq!(news.len(), 1, "{news:?}");
    let pane = news[0].pane_argv();
    let at = pane.iter().position(|a| a == "--resume").expect("--resume");
    assert_eq!(pane[at + 1], CONV_C, "{pane:?}");
    let sup = pane
        .iter()
        .position(|a| a == "--supervise")
        .expect("--supervise");
    assert_eq!(pane[sup + 1], NAME, "{pane:?}");
}

/// The one refusal line the log holds, with the test's own message.
fn refusal(fx: &Fx, what: &str) -> String {
    let log = fx.log_lines();
    let found: Vec<_> = log
        .iter()
        .filter(|l| l.contains(" standing session refused "))
        .collect();
    assert_eq!(found.len(), 1, "{what}: {log:?}");
    found[0].clone()
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

/// Issue #2912's own case: a standing session that sat idle and was then
/// killed by the out-of-memory killer, by a host event or by a pane that
/// went away has nothing to show for its run, and a login has room, so
/// nothing waited. It is started again all the same, as every process exit
/// nobody chose is.
///
/// Run 1 is waited for until the wrapper has recorded its process, so the
/// session was certainly up; it then dies having written nothing. Run 2
/// does a turn of work and ends cleanly.
#[test]
fn an_idle_session_that_was_killed_is_started_again_while_a_login_has_room() {
    let (fx, dir) = fixture(&["wait:137", "hold:0"]);
    set_standing(&fx, true);
    let w = Running::spawn(&fx, wrapped(&fx, &dir, &[], &[]));
    run_is_up(&fx, 1);
    release(&fx, 1);
    run_is_up(&fx, 2);
    release(&fx, 2);
    expect_code(&w.finish(), 0);
    let r = runs(&fx);
    assert_eq!(r.len(), 2, "the idle death was not answered: {r:?}");
    assert!(r[1].contains("--continue"), "{}", r[1]);
    let log = fx.log_lines();
    let lines = relaunch_lines(&log);
    assert_eq!(lines.len(), 1, "{log:?}");
    assert!(
        lines[0].ends_with("reason=SIGKILL waited=none"),
        "{}",
        lines[0]
    );
    assert!(
        !log.iter().any(|l| l.contains(" standing session refused ")),
        "{log:?}"
    );
    assert_eq!(waiting_lines(&fx), 0, "a login had room: {log:?}");
}

/// A session that starts, is seen, and exits non-zero on every start is
/// relaunched on every exit. The wrapper keeps no count that could stop it.
/// What it leaves instead is the evidence the supervisor agent judges a
/// crash loop on: each relaunch on the session's record, with how many
/// relaunches there have been since the model last replied and since when.
///
/// Run 1 does a turn of work and exits 1, so the record of its relaunch
/// says the model replied. Runs 2 to 4 are waited for until the wrapper's id
/// thread has recorded their process, so the wrapper certainly saw them, and
/// each exits 1 having written nothing: the chain grows by one each time and
/// keeps the time it began. Run 5 ends cleanly, which is the owner ending
/// the session.
#[test]
fn a_session_that_keeps_crashing_is_relaunched_every_time_and_its_record_says_so() {
    let (fx, dir) = fixture(&["hold:1", "wait:1", "wait:1", "wait:1", "wait:0"]);
    set_standing(&fx, true);
    let w = Running::spawn(&fx, wrapped(&fx, &dir, &[], &[]));
    let mut notes = Vec::new();
    for n in 1..=5usize {
        run_is_up(&fx, n);
        notes.push(untimed(&alert(&fx)));
        release(&fx, n);
    }
    let out = w.finish();
    expect_code(&out, 0);
    assert_eq!(runs(&fx).len(), 5, "{:?}", runs(&fx));
    let log = fx.log_lines();
    let lines = relaunch_lines(&log);
    assert_eq!(lines.len(), 4, "{log:?}");
    for line in lines {
        assert!(
            line.ends_with(
                " standing session relaunch session=area-docs reason=exit-status-1 waited=none"
            ),
            "{line}"
        );
        assert!(stderr(&out).contains(line.as_str()), "{}", stderr(&out));
    }
    no_refusal(&fx);

    assert_eq!(notes[0].0, "", "the first run has no relaunch to record");
    assert_eq!(
        notes[1].0,
        "relaunched in its pane at <ts> after it exited exit-status-1; \
         the model replied during the run"
    );
    for (n, chain) in [(2, 1), (3, 2), (4, 3)] {
        assert_eq!(notes[n].0, quiet_note(1, chain), "run {}", n + 1);
    }
    let began = &notes[2].1[1];
    assert!(
        notes[2..].iter().all(|(_, ts)| &ts[1] == began),
        "the chain keeps the time it began: {notes:?}"
    );
    assert_eq!(
        fx.agent(NAME)["state"],
        "ended",
        "the clean exit is the owner"
    );
}

/// The bytes of the stand-in's transcript on disk, zero when there is none.
fn transcript_bytes(fx: &Fx, dir: &Path) -> u64 {
    fs::metadata(transcript(fx, dir)).map_or(0, |m| m.len())
}

/// Crusty's case on PR #3009: a start-up crash loop that writes to its
/// transcript on every start. A real Claude Code does exactly that: a
/// `--continue` start ended before any turn appends `bridge-session` and
/// `cost-state` lines, and a failed request appends the prompt and an
/// assistant line of Claude Code's own. Measured by size, every one of
/// these runs did work, and the record would say so. Measured by replies,
/// they did none, and the record says that.
///
/// Run 1 does a turn of work and exits 1. Runs 2 and 4 start, write what a
/// start writes and exit 1; run 3 has its request fail and exits 1. Each is
/// relaunched, and while run 5 is up the record counts three relaunches
/// with no reply behind them.
#[test]
fn a_crash_loop_that_grows_its_transcript_is_recorded_as_without_a_reply() {
    let (fx, dir) = fixture(&["hold:1", "boot:1", "fail:1", "boot:1", "wait:0"]);
    set_standing(&fx, true);
    let w = Running::spawn(&fx, wrapped(&fx, &dir, &[], &[]));
    let mut sizes = Vec::new();
    for n in 1..=4usize {
        run_is_up(&fx, n);
        sizes.push(transcript_bytes(&fx, &dir));
        release(&fx, n);
    }
    run_is_up(&fx, 5);
    let note = untimed(&alert(&fx)).0;
    release(&fx, 5);
    expect_code(&w.finish(), 0);
    assert!(
        sizes.windows(2).all(|w| w[1] > w[0]) && sizes[0] > 0,
        "the premise: every run grew the transcript: {sizes:?}"
    );
    assert_eq!(runs(&fx).len(), 5, "{:?}", runs(&fx));
    assert_eq!(relaunch_lines(&fx.log_lines()).len(), 4);
    no_refusal(&fx);
    assert_eq!(note, quiet_note(1, 3));
}

/// A resume that copies the conversation's history into the transcript the
/// record names, as `--continue --fork-session` does into a new one, grows
/// it by everything that went before. The copied replies keep the times
/// they were first written, before this run's launch, so they are not this
/// run's replies and the record counts the runs as without one.
#[test]
fn history_copied_into_the_transcript_is_not_a_reply() {
    let (fx, dir) = fixture(&["hold:1", "copy:1", "copy:1", "wait:0"]);
    set_standing(&fx, true);
    let w = Running::spawn(&fx, wrapped(&fx, &dir, &[], &[]));
    for n in 1..=3usize {
        run_is_up(&fx, n);
        release(&fx, n);
        assert!(wait_until(|| exited(&fx, n)), "run {n}");
    }
    run_is_up(&fx, 4);
    let note = untimed(&alert(&fx)).0;
    release(&fx, 4);
    expect_code(&w.finish(), 0);
    assert_eq!(runs(&fx).len(), 4, "{:?}", runs(&fx));
    let reply = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/commands/agent/testdata/issue_2912/model_reply_turn.jsonl"),
    )
    .unwrap();
    // `@NOW@` becomes a 24-character timestamp in each line the run wrote.
    let one_turn = reply.len() + reply.matches("@NOW@").count() * (24 - "@NOW@".len());
    assert_eq!(
        transcript_bytes(&fx, &dir),
        4 * one_turn as u64,
        "the premise: two copies doubled the transcript twice"
    );
    assert_eq!(note, quiet_note(1, 2));
    no_refusal(&fx);
}

/// A session that never gets anywhere at all, whether its Claude Code was
/// seen by the wrapper's process scan or died before it could be, is
/// relaunched on every exit, and the scan is not consulted. Three crashes
/// are three relaunches; the fourth run ends cleanly, which is the owner.
#[test]
fn a_session_that_gets_nowhere_at_all_is_relaunched_on_every_exit() {
    for seen in [true, false] {
        let fx = Fx::new();
        let dir = fx.dir("docs");
        install_stand_in(&fx, &dir, &["exit:1", "exit:1", "exit:1"]);
        if seen {
            show_claude(&fx, &dir);
        }
        set_standing(&fx, true);
        let out = fx.output(&mut wrapped(&fx, &dir, &[], &[]));
        expect_code(&out, 0);
        assert_eq!(runs(&fx).len(), 4, "seen={seen}: {:?}", runs(&fx));
        let log = fx.log_lines();
        let lines = relaunch_lines(&log);
        assert_eq!(lines.len(), 3, "seen={seen}: {log:?}");
        for line in lines {
            assert!(
                stderr(&out).contains(line.as_str()),
                "seen={seen}: {}",
                stderr(&out)
            );
        }
        no_refusal(&fx);
    }
}

/// A session whose conversation is never on its record: nothing in the
/// synthetic process table is a Claude, so the wrapper's id thread never
/// finds one with a sessions file and there is no conversation of this
/// session's to read. It is relaunched like any other, and the record says
/// no reply was found; `agent status` shows the missing conversation next
/// to it.
#[test]
fn a_session_with_no_conversation_on_record_is_relaunched_and_recorded() {
    let fx = Fx::new();
    let dir = fx.dir("docs");
    install_stand_in(&fx, &dir, &["exit:1", "exit:1", "wait:0"]);
    set_standing(&fx, true);
    let w = Running::spawn(&fx, wrapped(&fx, &dir, &[], &[]));
    assert!(wait_until(|| runs(&fx).len() == 3), "{:?}", runs(&fx));
    let e = fx.agent(NAME);
    let note = untimed(&alert(&fx)).0;
    release(&fx, 3);
    expect_code(&w.finish(), 0);
    assert!(
        e["conversation_id"].is_null(),
        "the premise is a record with no conversation: {e}"
    );
    assert_eq!(e["state"], "active", "{e}");
    assert_eq!(note, quiet_note(1, 2));
    no_refusal(&fx);
}

/// Every relaunch reaches the supervisor, not only the log: it is on the
/// session's own record, which stays `active`, and `amplihack agent status`
/// prints it as an `ALERT` line. That is where the supervisor agent reads a
/// session that keeps exiting.
///
/// Both crashing runs are waited for until the wrapper has recorded the
/// session's process and conversation, so the conversation is on the record
/// and the note names what was read of it: the model wrote no reply to it.
#[test]
fn every_relaunch_is_left_on_the_record_for_the_supervisor() {
    let (fx, dir) = fixture(&["wait:7", "wait:7", "wait:0"]);
    set_standing(&fx, true);
    let w = Running::spawn(&fx, wrapped(&fx, &dir, &[], &[]));
    for n in 1..=2usize {
        run_is_up(&fx, n);
        release(&fx, n);
    }
    run_is_up(&fx, 3);
    let e = fx.agent(NAME);
    let raw = alert(&fx);
    let status = fx.run(&["agent", "status"]);
    release(&fx, 3);
    expect_code(&w.finish(), 0);
    assert_eq!(e["state"], "active", "{e}");
    assert_eq!(e["conversation_id"], CONV_C, "{e}");
    let (note, times) = untimed(&raw);
    assert_eq!(note, quiet_note(7, 2), "{raw}");
    assert_eq!(
        skill_row_for(&raw),
        Some("relaunched in its pane at"),
        "the supervisor skill reads it: {raw}"
    );
    assert!(
        times[1] <= times[0],
        "the chain began before this relaunch: {raw}"
    );
    assert!(
        stdout(&status).contains(&format!("ALERT {NAME}: {raw}")),
        "the status shows it: {}",
        stdout(&status)
    );
}

/// The supervisor agent's way to stop a session it judges to be a crash
/// loop: take the record away. The next exit is then not this wrapper's to
/// relaunch, and it says so once rather than starting Claude Code again.
#[test]
fn a_crash_loop_the_supervisor_unregisters_is_not_relaunched_again() {
    let (fx, dir) = fixture(&["wait:1", "wait:1", "wait:1", "wait:1"]);
    set_standing(&fx, true);
    let w = Running::spawn(&fx, wrapped(&fx, &dir, &[], &[]));
    run_is_up(&fx, 1);
    release(&fx, 1);
    run_is_up(&fx, 2);
    let unregistered = fx.run(&["agent", "unregister", NAME]);
    expect_code(&unregistered, 0);
    release(&fx, 2);
    expect_code(&w.finish(), 1);
    assert_eq!(runs(&fx).len(), 2, "{:?}", runs(&fx));
    assert_eq!(relaunch_lines(&fx.log_lines()).len(), 1);
    let refusal = refusal(&fx, "unregistered");
    assert!(
        refusal.ends_with(" standing session refused session=area-docs reason=superseded"),
        "{refusal}"
    );
}

/// A launch that returns an error with no exit status from Claude Code
/// started no process, so there is no exit to relaunch, and a relaunch would
/// fail again within moments with no Claude Code start to pace the loop.
/// Here the stand-in is removed while run 1 is up, so the relaunch's launch
/// cannot find `claude`. The wrapper refuses once, leaves the reason on the
/// record, which stays `active` for the supervisor's recovery, and returns
/// the error. The supervisor skill has a row for the alert, and the next
/// pass of the supervisor service resumes the session.
#[test]
fn a_launch_that_fails_without_an_exit_status_is_left_for_the_supervisor() {
    let (fx, dir) = fixture(&["hold:1", "hold:0"]);
    set_standing(&fx, true);
    let w = Running::spawn(&fx, wrapped(&fx, &dir, &[], &[]));
    run_is_up(&fx, 1);
    fs::remove_file(fx.tools.join("claude")).unwrap();
    release(&fx, 1);
    let out = w.finish();
    assert!(!out.status.success(), "{:?}", out.status);
    assert_eq!(runs(&fx).len(), 1, "{:?}", runs(&fx));
    assert_eq!(relaunch_lines(&fx.log_lines()).len(), 1);
    let refusal = refusal(&fx, "launch failed");
    assert!(
        refusal.ends_with(" standing session refused session=area-docs reason=launch-failed"),
        "{refusal}"
    );
    let e = fx.agent(NAME);
    assert_eq!(e["state"], "active", "{e}");
    assert_eq!(
        e["alert"].as_str().unwrap_or_default(),
        "not relaunched: the launch failed without an exit status from Claude \
         Code, so no process exited to be relaunched; the error is in the pane, \
         and the session was left for the supervisor",
        "{e}"
    );
    assert_eq!(
        skill_row_for(e["alert"].as_str().unwrap_or_default()),
        Some("not relaunched: the launch failed without an exit status"),
    );
    one_supervisor_pass_resumes_it(&fx);
}

/// Work keeps a session going with no count limit: four crashes after four
/// turns of work are four relaunches, with a login with room throughout.
#[test]
fn a_session_that_keeps_working_is_relaunched_as_often_as_it_exits() {
    let (fx, dir) = fixture(&["hold:1", "hold:1", "hold:1", "hold:1", "hold:0"]);
    set_standing(&fx, true);
    let w = Running::spawn(&fx, wrapped(&fx, &dir, &[], &[]));
    for n in 1..=5usize {
        run_is_up(&fx, n);
        release(&fx, n);
    }
    expect_code(&w.finish(), 0);
    assert_eq!(runs(&fx).len(), 5, "{:?}", runs(&fx));
    let log = fx.log_lines();
    assert_eq!(relaunch_lines(&log).len(), 4, "{log:?}");
    assert!(
        !log.iter().any(|l| l.contains(" standing session refused ")),
        "{log:?}"
    );
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

/// A wait for room that ends without one and without a signal leaves a
/// session stopped that its owner wants running. Here the rotator's
/// directory is moved away during the wait, so the watch on the swap record
/// is lost. The refusal is logged, and its cause is on the session's record
/// too: the record stays `active` for the supervisor and `amplihack agent
/// status` shows why. The supervisor skill has a row for the alert, and the
/// next pass of the supervisor service resumes the session.
#[test]
fn a_wait_for_room_that_loses_its_watch_is_left_on_the_record() {
    let (fx, dir) = fixture(&["hold:1", "hold:0"]);
    set_standing(&fx, true);
    let swap_file = swap(&fx, 1, 100, 0o600);
    let w = Running::spawn(&fx, wrapped(&fx, &dir, &[], &[]));
    run_is_up(&fx, 1);
    release(&fx, 1);
    waiting_for_room(&fx, 1);
    let rotate = swap_file.parent().unwrap().to_path_buf();
    fs::rename(&rotate, rotate.with_extension("moved")).unwrap();
    expect_code(&w.finish(), 1);
    assert_eq!(runs(&fx).len(), 1, "{:?}", runs(&fx));
    let refusal = refusal(&fx, "watch lost");
    assert!(
        refusal.ends_with(" standing session refused session=area-docs reason=gone"),
        "{refusal}"
    );
    let e = fx.agent(NAME);
    assert_eq!(
        e["state"], "active",
        "the supervisor may still recover it: {e}"
    );
    let alert = e["alert"].as_str().unwrap_or_default();
    assert_eq!(
        alert,
        "not relaunched: it exited exit-status-1 while no login had room, and the \
         watch on the rotator's swap record was lost, so the session was left for \
         the supervisor",
        "{e}"
    );
    let status = fx.run(&["agent", "status"]);
    assert!(
        stdout(&status).contains(&format!("ALERT {NAME}: {alert}")),
        "the status shows it: {}",
        stdout(&status)
    );
    assert_eq!(
        skill_row_for(alert),
        Some("not relaunched: it exited"),
        "the supervisor skill reads it"
    );
    one_supervisor_pass_resumes_it(&fx);
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
