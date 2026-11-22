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
//! attempt that never starts, which must leave fd 0 whole for the plain
//! launch and say nothing about a step that never ran; a captured pipe the
//! caller never closes, with a limit and without one; a capture lost before
//! the limit, which must refuse the rerun rather than wait first and fail
//! after; and a source the copier cannot read, which reaches the attempt as
//! a clean end of file and must not pass for a prompt it was given whole.
//!
//! Issue #3036 removes two endings these tests used to pin. An
//! `--input-format` stream channel is copied and replayed like any piped
//! prompt, and a capture file the disk refuses carries on in memory, so in
//! both a limit is waited out and the rerun reads the same bytes.

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

/// Issue #2911: a limit that is still there after the wait is waited for
/// again, as often as it comes back, and every retry is given the same piped
/// prompt from the one copy the supervisor kept.
///
/// Renamed from `a_limit_still_failing_after_the_wait_ends_the_step_and_leaves_no_capture`,
/// which asserted that the third limit ended the step: the re-entry used the
/// rest of the step's `max_wait_secs` budget and the next limit was refused.
/// Both the budget and the refusal are gone. The copy is still removed when
/// the step ends; the tests above and below that end a step with an error
/// cover the removal on that path.
#[test]
fn a_limit_that_keeps_coming_back_is_waited_out_each_time_with_the_same_prompt() {
    let f = Fixture::new();
    let mut c = f.cmd(&["claude", "--subprocess-safe", "--", "-p"]);
    // The shortest length a wait may be given, so the five waits stay short
    // on the test clock whatever kind of wait each one is.
    c.env("LIMIT_CALLS", "5")
        .env("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "60")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().expect("start amplihack");
    let bytes = b"prompt\n".to_vec();
    let feeder = feed(&mut child, bytes.clone());
    let out = child.wait_with_output().unwrap();
    feeder.join().unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        f.count("0"),
        6,
        "five limits, five waits, then the attempt that works: {}",
        describe(&out)
    );
    for n in 1..=6 {
        assert_eq!(
            f.call("0", n)["STDIN"],
            sha256_hex(&bytes),
            "attempt {n} was given the whole prompt"
        );
    }
    let ev = events(&out);
    assert_eq!(
        ev.iter().filter(|l| l.starts_with("start ")).count(),
        5,
        "{ev:?}"
    );
    assert!(
        !ev.iter()
            .any(|l| l.starts_with("skipped") || l.starts_with("failed")),
        "{ev:?}"
    );
    assert_eq!(leftovers(&f), Vec::<String>::new(), "the copy is removed");
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
    let err = text(&out.stderr);
    assert!(
        err.contains("running it once"),
        "the stand-down was reported\n{}",
        describe(&out)
    );
    // Nothing was read off fd 0, so nothing may be said about a step that
    // stopped reading: no attempt ever ran. Before this the copier had
    // already started, met the dropped read end of the first attempt's
    // pipe, and said the step "ran on part of its task".
    assert!(
        !err.contains("stopped reading its prompt"),
        "no attempt ran, so nothing stopped reading\n{}",
        describe(&out)
    );
    assert_eq!(
        leftovers(&f),
        Vec::<String>::new(),
        "no capture left behind"
    );
}

/// Issue #2816: a stand-down costs the step nothing, even when its stdin
/// cannot be read at all.
///
/// The step's stdin here is the *write* end of a pipe: a pipe, so a capture
/// is set up, and unreadable, so a copier would fail on its first read. No
/// copier runs, because none is started until the first attempt is spawned
/// and this one never is. So fd 0 is exactly what the caller gave the step,
/// and the plain launch runs on it — which is what this launch did before
/// any of this existed.
#[test]
fn a_stand_down_with_an_unreadable_stdin_still_runs_the_step() {
    let f = Fixture::new();
    let mut c = f.cmd(&["claude", "--subprocess-safe", "--", "-p"]);
    let (_read, write) = std::io::pipe().expect("a pipe");
    c.env("AMPLIHACK_TEST_FAIL_ATTEMPT", "1")
        // The plain launch is the only run, so it must not meet a limit.
        .env("LIMIT_CALLS", "0")
        .stdin(Stdio::from(std::os::fd::OwnedFd::from(write)))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = c.spawn().expect("start amplihack");
    let out = finish(child, "the step hung after an unreadable stdin");

    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 1, "one plain launch\n{}", describe(&out));
    let err = text(&out.stderr);
    assert!(
        err.contains("running it once"),
        "the stand-down was reported\n{}",
        describe(&out)
    );
    assert!(
        !err.contains("stopped reading its prompt"),
        "no attempt ran, so nothing stopped reading\n{}",
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

/// Issue #2816: a captured pipe the caller never closes ends the step with
/// its limit result, and says why.
///
/// `a_held_open_captured_pipe_does_not_hang_the_step` above is the same
/// shape without a limit, and it never reaches the copier at all. This one
/// does: the attempt reports a usage limit, the wait runs to the stated
/// reset, and only then is the stored prompt needed. There is none yet -
/// the caller is still holding the pipe and has written nothing.
///
/// Issue #3036, review of PR #2992: the retry waits for it. This used to
/// end the step with the limit result and `skipped reason=piped_prompt`,
/// a limit ending a step for a prompt that was merely late. The step now
/// says `waiting_for_prompt`, keeps answering stop signals and the end of
/// its session as a limit wait does (so issue #1618's rule, never blocked
/// deaf on a pipe nobody closes, still holds), and runs the retry as soon
/// as the caller closes the pipe.
#[test]
fn issue_3036_a_held_open_captured_pipe_with_a_limit_waits_for_its_prompt() {
    let f = Fixture::new();
    let mut c = f.cmd(&["claude", "--subprocess-safe", "--", "-p"]);
    // The agent is told not to read its stdin: the caller writes nothing.
    let (read, write) = std::io::pipe().expect("a pipe");
    c.env("SKIP_STDIN", "1")
        .stdin(Stdio::from(std::os::fd::OwnedFd::from(read)));
    // Held open until the step says it is waiting for its prompt.
    let mut held = Some(write);
    let waiting = "amplihack.recipe.usage_limit_wait waiting_for_prompt";
    let out = f.run_watching(&mut c, |line| {
        if line == waiting {
            held = None;
        }
    });
    assert!(
        held.is_none(),
        "the step never said it was waiting\n{}",
        describe(&out)
    );

    assert!(
        out.status.success(),
        "the retry ran once its prompt had arrived\n{}",
        describe(&out)
    );
    assert_eq!(f.count("0"), 2, "one retry\n{}", describe(&out));
    let err = text(&out.stderr);
    assert!(
        !err.contains("skipped reason=piped_prompt") && !err.contains("cannot be run again"),
        "a late prompt does not end the step\n{}",
        describe(&out)
    );
    let ev = events(&out);
    assert_eq!(ev.len(), 4, "{ev:?}");
    assert!(ev[0].starts_with("start reason=usage_limit "), "{ev:?}");
    assert_eq!(ev[1], "end reason=reset reached", "{ev:?}");
    assert_eq!(ev[2], "waiting_for_prompt", "{ev:?}");
    assert_eq!(ev[3], "resume attempt=2", "{ev:?}");
    assert_eq!(
        leftovers(&f),
        Vec::<String>::new(),
        "no capture left behind"
    );
}

/// Issue #3036: a usage limit on a step whose input is an `--input-format`
/// stream channel is waited out, and the retry reads the same input from
/// its first byte.
///
/// Before #3036 such a channel was never copied, and a limit ended the step
/// with `skipped reason=piped_prompt`. It is copied now as any piped prompt
/// is. Under supervision every attempt's stdout is held back and printed
/// only when the step finishes, and an attempt that is run again has its
/// output discarded, so the caller has seen nothing of the attempt that met
/// the limit, and a retry given the same input is the same step from its
/// side. The caller here writes its messages and closes the channel, which
/// is the only way a supervised stream step can finish at all, since its
/// output reaches the caller only once the step has ended.
#[test]
fn a_limit_on_an_input_format_channel_is_waited_out_and_the_rerun_reads_the_same_input() {
    let f = Fixture::new();
    // Two stream-json user messages, then the awkward bytes the other tests
    // use, so the comparison is byte for byte over more than a pipe holds.
    let mut bytes = br#"{"type":"user","message":{"role":"user","content":"first"}}"#.to_vec();
    bytes.push(b'\n');
    bytes.extend_from_slice(br#"{"type":"user","message":{"role":"user","content":"second"}}"#);
    bytes.push(b'\n');
    bytes.extend(prompt());
    let mut child = start(
        &f,
        &[
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
        ],
    );
    let feeder = feed(&mut child, bytes.clone());
    let out = finish(child, "the step hung on a stream-json channel");
    feeder.join().unwrap();

    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "the step ran again\n{}", describe(&out));
    let (first, second) = (
        f.call("0", 1)["STDIN"].clone(),
        f.call("0", 2)["STDIN"].clone(),
    );
    assert_eq!(
        first,
        sha256_hex(&bytes),
        "attempt 1 read the whole channel"
    );
    assert_eq!(
        second, first,
        "the retry read the same input, byte for byte"
    );

    let ev = events(&out);
    assert_eq!(ev.len(), 3, "one wait, then the retry: {ev:?}");
    assert!(
        ev[0].starts_with(
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 "
        ),
        "{ev:?}"
    );
    assert_eq!(ev[1], "end reason=reset reached");
    assert_eq!(ev[2], "resume attempt=2");
    let err = text(&out.stderr);
    assert!(
        !err.contains("skipped reason=piped_prompt"),
        "the channel is not an ending any more\n{}",
        describe(&out)
    );
    assert!(!err.contains("cannot be run again"), "{}", describe(&out));
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

/// Issues #2816, #3036: a capture file that cannot be written says why, the
/// attempt in hand still gets its whole prompt, and so does the rerun the
/// limit's wait earns.
///
/// `AMPLIHACK_TEST_REFUSE_CAPTURE_WRITE=1` gives the copier a read-only
/// description of the capture file, so every write to it fails as a full
/// disk, a quota or a failing device would. None of those can be arranged
/// from outside a test.
///
/// Before #3036 the step then ended with `skipped reason=piped_prompt`: a
/// full disk was a reason for a usage limit to end a run. Now the copier
/// names its error the moment it meets it and carries the copy on in
/// memory, the limit is waited out, and the rerun reads the same bytes.
#[test]
fn a_capture_file_that_cannot_be_written_names_its_error_and_the_rerun_still_runs() {
    let f = Fixture::new();
    let bytes = prompt();
    let mut c = f.cmd(&["claude", "--subprocess-safe", "--", "-p"]);
    c.env("AMPLIHACK_TEST_REFUSE_CAPTURE_WRITE", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().expect("start amplihack");
    let feeder = feed(&mut child, bytes.clone());
    let out = finish(child, "the step hung on a capture it could not write");
    feeder.join().unwrap();

    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "the step ran again\n{}", describe(&out));
    assert_eq!(
        f.call("0", 1)["STDIN"],
        sha256_hex(&bytes),
        "the attempt still read the whole prompt, byte for byte\n{}",
        describe(&out)
    );
    assert_eq!(
        f.call("0", 2)["STDIN"],
        sha256_hex(&bytes),
        "and so did the rerun, from memory\n{}",
        describe(&out)
    );
    let err = text(&out.stderr);
    assert!(
        err.contains("could not be stored on disk for a rerun ("),
        "the copier names its error\n{}",
        describe(&out)
    );
    assert!(
        err.contains("kept in memory instead"),
        "and says nothing is lost by it\n{}",
        describe(&out)
    );
    assert!(
        !err.contains("could not be read to its end"),
        "the prompt itself was read fine\n{}",
        describe(&out)
    );
    let ev = events(&out);
    assert_eq!(ev.len(), 3, "one wait, then the retry: {ev:?}");
    assert!(ev[0].starts_with("start reason=usage_limit "), "{ev:?}");
    assert_eq!(ev[1], "end reason=reset reached");
    assert_eq!(ev[2], "resume attempt=2");
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
