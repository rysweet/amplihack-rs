//! Issue #2911: the one ending left to a step that has
//! met a limit is a lost prompt, and it is reached only when the prompt
//! itself could not be read to its end.
//!
//! A piped prompt used to have three ways to end a step on a limit with
//! `skipped reason=piped_prompt`: the prompt cut short, the copy failing
//! (a panic, a flush, a part on disk that could not be read back), and the
//! copy never set up because the process was out of file descriptors. The
//! last two left the attempt with its whole task and no copy for a retry,
//! so the limit ended the step. Both are gone:
//!
//! - the copy is kept in memory, so every byte the copier reads is a byte a
//!   retry is given, and nothing about storing it can fail;
//! - a step whose copy cannot be set up is not run at all, before a byte of
//!   its prompt is read, so no attempt runs that a retry could not repeat.
//!
//! What is left is the prompt cut short: the attempt was given part of its
//! task, so the step fails whether or not it meets a limit. It is a
//! prompt-loss failure, logged `failed reason=prompt_cut_short`. A capacity
//! wait could not help it, because a retry would be given the same part of
//! the task. When a limit also stands the run learns of it as data: the
//! supervisor reports it to the recipe runner over the recovery channel,
//! which records it as the step's `failure_cause`, and a relaunch resumes
//! the step with its whole prompt. The step also writes a sentence and the
//! reset for the person reading the log; `limit_wait::tests_ending_evidence`
//! pins that the run's classification does not depend on that sentence.
//!
//! The stub `claude` of `issue_1618_fixture` hashes its stdin and reports a
//! limit on its first `LIMIT_CALLS` calls. The wait runs on the fixture's
//! test clock, so these runs take seconds.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, WEEKLY_TEXT, describe, events, sha256_hex, text};
use std::io::Write;
use std::process::{Command, Output, Stdio};

/// More than a pipe buffer, with a NUL and bytes that are not UTF-8.
fn prompt() -> Vec<u8> {
    let mut v = b"the piped prompt\0\xff\xfe\n".to_vec();
    v.extend((0..200_000u32).map(|i| (i % 251) as u8));
    v
}

/// A recipe agent step whose prompt is piped: a bare `-p`.
fn piped_step(f: &Fixture) -> Command {
    let mut c = f.cmd(&["claude", "--subprocess-safe", "--", "-p"]);
    c.stdout(Stdio::piped()).stderr(Stdio::piped());
    c
}

/// Runs `c` with `bytes` written to its stdin on a thread, as the recipe
/// runner writes a step's prompt, and the pipe closed after them.
fn run_fed(c: &mut Command, bytes: Vec<u8>) -> Output {
    c.stdin(Stdio::piped());
    let mut child = c.spawn().expect("start amplihack");
    let mut stdin = child.stdin.take().expect("piped stdin");
    let feeder = std::thread::spawn(move || {
        // A step that is not run never reads it.
        let _ = stdin.write_all(&bytes);
    });
    let out = child.wait_with_output().expect("wait for amplihack");
    feeder.join().unwrap();
    out
}

/// What a step that is not run, or that met no lost prompt, must never
/// write: the prompt-loss ending, its sentence, and the reason it replaced.
fn assert_no_prompt_loss_ending(out: &Output) {
    let err = text(&out.stderr);
    for gone in [
        "prompt_cut_short",
        "piped_prompt",
        "its prompt was lost",
        "cannot be run again",
    ] {
        assert!(!err.contains(gone), "{gone:?}\n{}", describe(out));
    }
}

/// A copy that cannot be set up does not let a step run without one. The
/// step is not run at all, the run is told why in words that carry no limit
/// marker (no limit was met, and the recipe runner must not class the
/// failure as one), and nothing about a limit or a lost prompt is written.
///
/// `AMPLIHACK_TEST_FAIL_PROMPT_COPY=1` makes the setup fail as it does in a
/// process out of file descriptors (`EMFILE`), which cannot be arranged
/// from outside without starving the test of descriptors too. Before this
/// change such a step ran on the caller's pipe directly, and a limit on it
/// ended the step with `skipped reason=piped_prompt`.
#[test]
fn a_step_whose_copy_cannot_be_set_up_is_not_run() {
    let f = Fixture::new();
    let mut c = piped_step(&f);
    c.env("AMPLIHACK_TEST_FAIL_PROMPT_COPY", "1");
    let out = run_fed(&mut c, prompt());

    assert!(!out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 0, "no attempt ran\n{}", describe(&out));
    let err = text(&out.stderr);
    assert!(
        err.contains(
            "this step was not run: its prompt comes on a pipe, and the copy of it that a \
             retry would read could not be set up ("
        ),
        "the run is told why, in one sentence\n{}",
        describe(&out)
    );
    assert!(
        err.contains("os error 24"),
        "and the error is named\n{}",
        describe(&out)
    );
    assert!(
        !err.to_ascii_lowercase().contains("limit"),
        "no limit was met, so none is named\n{}",
        describe(&out)
    );
    assert!(events(&out).is_empty(), "no wait line\n{}", describe(&out));
    assert_no_prompt_loss_ending(&out);
}

/// The one ending left, and what it says. The step's stdin is the write end
/// of a pipe: a pipe, so a copy is set up, and unreadable, so the copier's
/// first read fails and the attempt is given an empty prompt. The attempt
/// reports a limit.
///
/// The step ends because its prompt was lost, not because of the limit, and
/// both the event and the sentence say so. The sentence also names the
/// reset, because the limit still stands and a person reading the log needs
/// to know when a relaunch will find room. The run itself is told of the
/// limit through the recovery channel, not by this sentence.
#[test]
fn a_prompt_cut_short_is_a_lost_prompt_and_hands_the_run_the_reset() {
    let f = Fixture::new();
    let mut c = piped_step(&f);
    let (_read, write) = std::io::pipe().expect("a pipe");
    c.stdin(Stdio::from(std::os::fd::OwnedFd::from(write)));
    let out = c.output().expect("run amplihack");

    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 1, "no retry\n{}", describe(&out));
    assert_eq!(
        f.call("0", 1)["STDIN"],
        sha256_hex(b""),
        "the attempt was given part of its task: here, none of it\n{}",
        describe(&out)
    );
    assert_eq!(
        events(&out),
        vec!["failed reason=prompt_cut_short".to_string()],
        "a failure of the step's input, and no wait\n{}",
        describe(&out)
    );
    let err = text(&out.stderr);
    for said in [
        "this step failed because its prompt was lost, not because of its usage limit",
        "It fails whether or not it meets a limit",
        "The limit is reported to the recipe runner as this step's recorded cause",
        "amplihack: the limit resets 2026-10-18 08:30:00 -07:00",
    ] {
        assert!(err.contains(said), "{said:?}\n{}", describe(&out));
    }
    assert!(
        !err.contains("skipped reason=piped_prompt"),
        "not a skipped wait\n{}",
        describe(&out)
    );
}

/// The prompt-loss ending is reached only when the prompt could not be
/// read to its end. A piped prompt read whole is kept whole, so a step that
/// meets a limit again and again waits each time, every retry reads the
/// same bytes, and the ending is never written.
///
/// The limit is a weekly one whose stated reset is next year's, so each wait
/// is held to the seven-day window and ends at that deadline, which the
/// test clock reaches at once: every wait here ends on its own, with no
/// event from the test.
#[test]
fn a_prompt_read_whole_never_reaches_the_prompt_loss_ending() {
    let f = Fixture::new();
    let bytes = prompt();
    let mut c = piped_step(&f);
    c.env("LIMIT_CALLS", "3").env("LIMIT_MESSAGE", WEEKLY_TEXT);
    let out = run_fed(&mut c, bytes.clone());

    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 4, "three waits, then a success");
    for n in 1..=4 {
        assert_eq!(
            f.call("0", n)["STDIN"],
            sha256_hex(&bytes),
            "attempt {n} read the whole prompt\n{}",
            describe(&out)
        );
    }
    let ev = events(&out);
    assert_eq!(
        ev.iter().filter(|l| l.starts_with("start ")).count(),
        3,
        "{ev:?}"
    );
    assert!(!ev.iter().any(|l| l.starts_with("failed")), "{ev:?}");
    assert_no_prompt_loss_ending(&out);
}
