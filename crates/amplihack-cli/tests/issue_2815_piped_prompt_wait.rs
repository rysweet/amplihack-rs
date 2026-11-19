//! Issue #2815: a usage limit in a step whose prompt arrives on stdin is
//! waited out, and the step runs again with the same prompt.
//!
//! The stub `claude` of `issue_1618_fixture` hashes all of its stdin and
//! reports a limit on its first call. The prompt is piped (`-p` with no
//! prompt word), so it is not in argv. The wait runs on the test clock the
//! runner already has (`AMPLIHACK_USAGE_LIMIT_WAIT_TEST_NOW` and
//! `..._TEST_SCALE`), so the run takes seconds. `SKIP_STDIN` is never set,
//! except in the held-open case, which has no prompt to read.
//!
//! Issue #2816 adds the cases the capture path reaches on its own: a first
//! attempt that never starts, after the copier has taken the prompt off
//! stdin, with the copy usable and with it lost; a captured pipe the caller
//! never closes; a capture lost before the limit, which must refuse the
//! rerun rather than wait first and fail after; and a source the copier
//! cannot read, which reaches the attempt as a clean end of file and must
//! not pass for a prompt it was given whole.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, describe, events, sha256_hex, text};
use std::io::Write;
use std::process::{Child, Output, Stdio};
use std::time::Duration;

/// More than a pipe buffer, with a NUL and bytes that are not UTF-8.
fn prompt() -> Vec<u8> {
    let mut v = b"the piped prompt\0\xff\xfe\n".to_vec();
    v.extend((0..200_000u32).map(|i| (i % 251) as u8));
    v
}

fn start(f: &Fixture, claude_args: &[&str]) -> Child {
    let mut args = vec!["claude", "--subprocess-safe", "--"];
    args.extend_from_slice(claude_args);
    let mut c = f.cmd(&args);
    assert!(
        c.get_envs().all(|(k, _)| k != "SKIP_STDIN"),
        "SKIP_STDIN must not be set"
    );
    c.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    c.spawn().expect("start amplihack")
}

/// Writes the prompt on a thread and closes the pipe, as the recipe runner
/// does.
fn feed(child: &mut Child, bytes: Vec<u8>) -> std::thread::JoinHandle<()> {
    let mut stdin = child.stdin.take().unwrap();
    std::thread::spawn(move || {
        // The step may finish without reading all of it.
        let _ = stdin.write_all(&bytes);
    })
}

fn leftovers(f: &Fixture) -> Vec<String> {
    std::fs::read_dir(f.rerun_dir())
        .map(|d| {
            d.filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.ends_with(".in"))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_limit_on_a_piped_prompt_is_waited_out_and_the_rerun_gets_the_same_prompt() {
    let f = Fixture::new();
    let bytes = prompt();
    let mut child = start(&f, &["-p"]);
    let feeder = feed(&mut child, bytes.clone());
    let out = child.wait_with_output().unwrap();
    feeder.join().unwrap();

    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "the step ran again\n{}", describe(&out));
    let (first, second) = (
        f.call("0", 1)["STDIN"].clone(),
        f.call("0", 2)["STDIN"].clone(),
    );
    assert_ne!(first, "skipped");
    assert_ne!(second, "skipped");
    assert_eq!(first, second, "the rerun received the same prompt");
    assert_eq!(first, sha256_hex(&bytes), "byte for byte, as written");

    // One wait, reported once, naming the reset.
    let ev = events(&out);
    assert_eq!(ev.len(), 3, "{ev:?}");
    assert!(
        ev[0].starts_with(
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 "
        ),
        "{ev:?}"
    );
    assert_eq!(ev[1], "end reason=reset reached");
    assert_eq!(ev[2], "resume attempt=2");
    assert!(
        !text(&out.stderr).contains("skipped reason=piped_prompt"),
        "{}",
        describe(&out)
    );
    f.assert_isolated("0");
    assert_eq!(
        leftovers(&f),
        Vec::<String>::new(),
        "no capture left behind"
    );
}

#[test]
fn the_other_prompt_shapes_on_stdin_are_captured_too() {
    // The value of a hidden option, or of an option this version does not
    // know, is not the prompt: it is still on the pipe.
    for claude_args in [
        &["-p", "--max-thinking-tokens", "8000"][..],
        &["-p", "--some-new-opt", "value"],
    ] {
        let f = Fixture::new();
        let bytes = b"a short prompt\n".to_vec();
        let mut child = start(&f, claude_args);
        let feeder = feed(&mut child, bytes.clone());
        let out = child.wait_with_output().unwrap();
        feeder.join().unwrap();
        assert!(out.status.success(), "{claude_args:?}: {}", describe(&out));
        assert_eq!(f.count("0"), 2, "{claude_args:?}");
        assert_eq!(
            f.call("0", 1)["STDIN"],
            sha256_hex(&bytes),
            "{claude_args:?}"
        );
        assert_eq!(
            f.call("0", 2)["STDIN"],
            sha256_hex(&bytes),
            "{claude_args:?}"
        );
        assert_eq!(events(&out).len(), 3, "{claude_args:?}");
    }
}

#[test]
fn a_prompt_in_argv_is_not_captured() {
    // `-p step` carries the prompt: stdin is not copied, so no capture file
    // is made and the existing argv path is unchanged.
    let f = Fixture::new();
    let mut child = start(&f, &["-p", "step"]);
    let feeder = feed(&mut child, b"not the prompt\n".to_vec());
    let out = child.wait_with_output().unwrap();
    feeder.join().unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2);
    assert_eq!(leftovers(&f), Vec::<String>::new());
}

#[test]
fn a_limit_still_failing_after_the_wait_ends_the_step_and_leaves_no_capture() {
    let f = Fixture::new();
    let mut c = f.cmd(&["claude", "--subprocess-safe", "--", "-p"]);
    c.env("LIMIT_CALLS", "5")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().expect("start amplihack");
    let feeder = feed(&mut child, b"prompt\n".to_vec());
    let out = child.wait_with_output().unwrap();
    feeder.join().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "one wait, then terminal");
    assert_eq!(leftovers(&f), Vec::<String>::new(), "removed on error too");
}

/// Runs `child` with a ceiling, so a step that hangs fails the test instead
/// of the whole suite. The returned value is the finished `Output`.
///
/// Nothing in these tests is timed. No assertion depends on how long a run
/// takes, and none of them needs more than about a second: the wait itself
/// runs on the test clock the runner already has. The ceiling is only a
/// hang detector, and is deliberately far above anything load can cause,
/// which is what
/// `docs/bugfixes/bugfix-937-recipe-runner-timeout-test-flake.md` settled
/// on for the same problem. A tighter one would flake on a busy runner; no
/// ceiling at all would turn one stuck step into a stuck suite with
/// nothing naming the step that stuck.
fn finish(child: Child, what: &str) -> Output {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || tx.send(child.wait_with_output()));
    rx.recv_timeout(Duration::from_secs(120))
        .unwrap_or_else(|_| panic!("{what}"))
        .unwrap()
}

/// Issue #2816: a step that stops reading its prompt is reported.
///
/// The recipe runner warns when it writes a task straight to an agent that
/// exits before taking all of it. A supervised step puts the copier
/// between the two, and the copier always drains the runner's write, so
/// that warning can no longer fire for such a step. The supervisor makes
/// the report instead, and the step itself is unaffected: it still waits
/// out its limit and still runs again on the whole stored prompt.
///
/// `SKIP_STDIN=1` is an agent that never reads its stdin. The prompt is
/// several times what a pipe holds, so the copier must meet the closed end
/// whatever the scheduling. The rerun joins the copier before it starts,
/// so the line is on stderr before the run can end.
#[test]
fn a_step_that_stops_reading_its_prompt_is_reported() {
    let f = Fixture::new();
    let bytes = prompt();
    let mut c = f.cmd(&["claude", "--subprocess-safe", "--", "-p"]);
    c.env("SKIP_STDIN", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().expect("start amplihack");
    let feeder = feed(&mut child, bytes);
    let out = finish(child, "the step hung after its reader closed");
    feeder.join().unwrap();

    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        f.count("0"),
        2,
        "the step still waited and ran again\n{}",
        describe(&out)
    );
    assert!(
        text(&out.stderr).contains("stopped reading its prompt"),
        "the early close was reported\n{}",
        describe(&out)
    );
    assert_eq!(
        leftovers(&f),
        Vec::<String>::new(),
        "no capture left behind"
    );
}

/// Issue #2816: the supervisor stands down after the copier has started.
///
/// `AMPLIHACK_TEST_FAIL_ATTEMPT=1` makes the first attempt fail to start,
/// as a replaced binary, `EMFILE` or a refused output buffer would. The
/// supervisor's answer is to let a plain in-process launch run the step
/// once — on the very fd 0 the copier has been draining. The stored prompt
/// must therefore be handed back, and the single launch must read it whole.
/// Before #2816 that launch read an empty or truncated prompt, silently.
#[test]
fn a_first_attempt_that_cannot_start_hands_the_plain_launch_the_whole_prompt() {
    let f = Fixture::new();
    let bytes = prompt();
    let mut c = f.cmd(&["claude", "--subprocess-safe", "--", "-p"]);
    c.env("AMPLIHACK_TEST_FAIL_ATTEMPT", "1")
        // The plain launch is the only run, so it must not meet a limit.
        .env("LIMIT_CALLS", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().expect("start amplihack");
    let feeder = feed(&mut child, bytes.clone());
    let out = finish(child, "the step hung after the supervisor stood down");
    feeder.join().unwrap();

    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 1, "one plain launch\n{}", describe(&out));
    assert_eq!(
        f.call("0", 1)["STDIN"],
        sha256_hex(&bytes),
        "the plain launch read the whole prompt, byte for byte\n{}",
        describe(&out)
    );
    assert!(
        text(&out.stderr).contains("running it once"),
        "the stand-down was reported\n{}",
        describe(&out)
    );
    assert_eq!(
        leftovers(&f),
        Vec::<String>::new(),
        "no capture left behind"
    );
}

/// Issue #2816: the same stand-down with a capture that cannot be replayed.
///
/// The step's stdin here is the *write* end of a pipe. It is a pipe, so a
/// copy is started, and every read of it fails with `EBADF`, so no prompt
/// is ever stored. The step is then not run at all: a plain launch would
/// read a stdin the copier has already touched, and a prompt that is short
/// is a task that is wrong. The failure names the cause.
#[test]
fn a_stand_down_with_an_unusable_capture_refuses_to_run_the_step() {
    let f = Fixture::new();
    let mut c = f.cmd(&["claude", "--subprocess-safe", "--", "-p"]);
    let (_read, write) = std::io::pipe().expect("a pipe");
    c.env("AMPLIHACK_TEST_FAIL_ATTEMPT", "1")
        .env("LIMIT_CALLS", "0")
        .stdin(Stdio::from(std::os::fd::OwnedFd::from(write)))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = c.spawn().expect("start amplihack");
    let out = finish(child, "the step hung after an unusable capture");

    assert!(!out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 0, "the step never ran\n{}", describe(&out));
    let err = text(&out.stderr);
    assert!(
        err.contains("cannot be given to a plain launch"),
        "the refusal names its cause\n{}",
        describe(&out)
    );
    assert_eq!(
        leftovers(&f),
        Vec::<String>::new(),
        "no capture left behind"
    );
}

/// Issue #2816: the regression guard of issue #1618, on the capture path.
///
/// `a_held_open_stdin_pipe_does_not_hang_the_step` in
/// `issue_1618_wait_stdin.rs` passes the prompt in argv, so nothing is
/// copied and the copier is never started. This is the same shape with a
/// bare `-p`, where a copier does run: the caller holds the pipe open and
/// never writes a byte to it, and the step must still finish. The
/// supervisor's own thread never reads stdin, and the copier is joined only
/// when a rerun needs the prompt, which a step that meets no limit never
/// does.
#[test]
fn a_held_open_captured_pipe_does_not_hang_the_step() {
    let f = Fixture::new();
    let mut c = f.cmd(&["claude", "--subprocess-safe", "--", "-p"]);
    // The agent is told not to read its stdin: the caller never writes one.
    c.env("SKIP_STDIN", "1")
        .env("LIMIT_CALLS", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().expect("start amplihack");
    // Held open for the whole of the step, never written and never closed.
    let held = child.stdin.take();
    let out = finish(child, "the step hung on a held-open captured pipe");
    drop(held);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 1, "one attempt, no limit\n{}", describe(&out));
    assert_eq!(
        leftovers(&f),
        Vec::<String>::new(),
        "no capture left behind"
    );
}

/// Issue #2816: an attempt that succeeds on a prompt the copier could not
/// read to its end does not make the step a success.
///
/// The step's stdin is the write end of a pipe: a pipe, so a copy starts,
/// and unreadable, so the copier's first read fails. The attempt then meets
/// a clean end of file, which it cannot tell from a whole prompt, and the
/// stub exits 0 on it.
///
/// Before this change that 0 was the step's 0: the prompt had been silently
/// shortened to nothing and no line anywhere said so, because the copier's
/// error was observed only by a join that a successful step never makes.
/// Now the copier names the cause as it happens, the supervisor names the
/// consequence, and the step fails — with the attempt's own output still
/// printed, since nothing it produced is hidden.
#[test]
fn an_attempt_that_succeeds_on_a_cut_short_prompt_does_not_make_the_step_succeed() {
    let f = Fixture::new();
    let mut c = f.cmd(&["claude", "--subprocess-safe", "--", "-p"]);
    let (_read, write) = std::io::pipe().expect("a pipe");
    c.env("LIMIT_CALLS", "0")
        .stdin(Stdio::from(std::os::fd::OwnedFd::from(write)))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = c.spawn().expect("start amplihack");
    let out = finish(child, "the step hung on a cut-short prompt");

    assert!(
        !out.status.success(),
        "a success on part of a prompt is not the step's success\n{}",
        describe(&out)
    );
    assert_eq!(f.count("0"), 1, "the attempt ran once\n{}", describe(&out));
    assert_eq!(
        f.call("0", 1)["STDIN"],
        sha256_hex(b""),
        "the attempt was given nothing but the end of file\n{}",
        describe(&out)
    );
    let err = text(&out.stderr);
    assert!(
        err.contains("could not be read to its end"),
        "the copier names the cause\n{}",
        describe(&out)
    );
    assert!(
        err.contains("not this step's result"),
        "the supervisor names the consequence\n{}",
        describe(&out)
    );
    assert!(
        text(&out.stdout).contains("attempt 1 output"),
        "the attempt's own output is still printed\n{}",
        describe(&out)
    );
    assert_eq!(
        leftovers(&f),
        Vec::<String>::new(),
        "no capture left behind"
    );
}

/// Issue #2816: a capture already known to be lost refuses the rerun when
/// the limit is seen, not after the wait.
///
/// The step's stdin is the write end of a pipe: a pipe, so a copy starts,
/// and unreadable, so the copier fails on its first read and publishes the
/// loss at once. The first attempt still runs — it gets the copier's own
/// pipe, which reaches end of file immediately — and reports a usage limit.
/// That end of file is a prompt cut short, which issue #2816 also makes the
/// copier say on stderr; the limit result is non-zero already, so what the
/// step returns is unchanged by it.
///
/// The supervisor must then do what it does for a capture that was never
/// made: write `skipped reason=piped_prompt` and pass the limit result up,
/// with the first attempt's held-back stdout. Before this change the flag
/// was read only by `for_attempt(2)`, so the step announced a wait, waited
/// up to `max_wait_secs`, and then ended in an error with that output
/// discarded.
#[test]
fn a_capture_lost_before_the_limit_refuses_the_rerun_instead_of_waiting() {
    let f = Fixture::new();
    let mut c = f.cmd(&["claude", "--subprocess-safe", "--", "-p"]);
    let (_read, write) = std::io::pipe().expect("a pipe");
    c.stdin(Stdio::from(std::os::fd::OwnedFd::from(write)))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = c.spawn().expect("start amplihack");
    let out = finish(child, "the step hung on a lost capture");

    assert_eq!(
        out.status.code(),
        Some(1),
        "the limit result is passed up\n{}",
        describe(&out)
    );
    assert_eq!(f.count("0"), 1, "no rerun\n{}", describe(&out));
    let err = text(&out.stderr);
    assert!(
        err.contains("skipped reason=piped_prompt"),
        "the refusal is logged\n{}",
        describe(&out)
    );
    assert!(
        err.contains("could not be read to its end"),
        "the prompt the attempt was handed is named as cut short\n{}",
        describe(&out)
    );
    assert_eq!(
        events(&out),
        vec!["skipped reason=piped_prompt".to_string()],
        "no wait was ever started\n{}",
        describe(&out)
    );
    assert!(
        text(&out.stdout).contains("attempt 1 output"),
        "the first attempt's held-back output is still printed\n{}",
        describe(&out)
    );
    assert_eq!(
        leftovers(&f),
        Vec::<String>::new(),
        "no capture left behind"
    );
}
