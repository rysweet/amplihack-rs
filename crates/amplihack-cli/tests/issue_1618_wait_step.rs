//! Issue #1618: a recipe agent step (`amplihack claude -p` with
//! `AMPLIHACK_NONINTERACTIVE=1`) that hits a usage limit waits for the reset
//! and runs once more with the same inputs, instead of ending the run.
//!
//! Covers AC1 (one retry, identical argv, cwd and stdin, only the retry's
//! stdout), AC7 (a second limit is passed up), AC10 (exact log lines), the
//! epoch and past-reset forms, the limits that are never waited out (spend,
//! a far or unreadable reset, model prose), a held-open stdin pipe, and ST7
//! (a marker on exit 0 or a failure that is not a usage limit never waits).
//! See `issue_1618_fixture`.

#![cfg(unix)]

use crate::issue_1618_fixture::{
    Fixture, LIMIT_TEXT, SPEND_SESSION_TEXT, SPEND_TEXT, SPEND_WEEKLY_TEXT, WEEKLY_TEXT,
    assert_progress_shape, claude_stdout, describe, events, sha256_hex, text, wait_lines,
};
use std::fs;
use std::process::Stdio;
use std::time::Duration;

/// Runs a step whose stdin is `input`.
fn step_with_stdin(f: &Fixture, input: &[u8], extra: &[(&str, &str)]) -> std::process::Output {
    let path = f.root.join("stdin.bin");
    fs::write(&path, input).unwrap();
    let mut c = f.step();
    c.stdin(Stdio::from(fs::File::open(&path).unwrap()));
    for (k, v) in extra {
        c.env(k, v);
    }
    f.run(&mut c)
}

#[test]
fn ac1_a_step_limited_once_waits_then_retries_with_identical_inputs() {
    let f = Fixture::new();
    let input: Vec<u8> = b"the step's prompt\n".repeat(5000);
    let out = step_with_stdin(&f, &input, &[]);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "exactly one retry");
    let (first, second) = (f.call("0", 1), f.call("0", 2));
    for key in [
        "ARGV",
        "PWD",
        "HOME",
        "CLAUDE_CONFIG_DIR",
        "PROFILE",
        "STDIN",
    ] {
        assert_eq!(first[key], second[key], "{key} differs between attempts");
    }
    assert_eq!(
        first["STDIN"],
        sha256_hex(&input),
        "the full stdin, byte-exact"
    );
    assert_eq!(first["PROFILE"], "<unset>", "no profile override");
    assert_eq!(
        claude_stdout(&out),
        "attempt 2 output\n",
        "only the retry's stdout is printed"
    );
    assert_eq!(
        events(&out),
        vec![
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 \
             until=2026-10-18 08:32:00 -07:00 bound=reset+margin",
            "end reason=reset reached",
            "resume attempt=2",
        ]
    );
    f.assert_isolated("0");
}

#[test]
fn ac10_lines_have_the_documented_fields_and_order() {
    let f = Fixture::new();
    let out = f.run(&mut f.step());
    assert!(out.status.success(), "{}", describe(&out));
    let lines = wait_lines(&out);
    let pos = |prefix: &str| {
        lines
            .iter()
            .position(|l| l.starts_with(prefix))
            .unwrap_or_else(|| panic!("no {prefix:?} line in {lines:?}"))
    };
    let (start, end, resume) = (pos("start "), pos("end "), pos("resume "));
    assert!(start < end && end < resume, "{lines:?}");
    for (i, l) in lines.iter().enumerate() {
        if i > start && i < end {
            assert!(
                l.starts_with("progress "),
                "only progress between start and end: {l}"
            );
        }
    }
    assert_progress_shape(&out);
    let stderr = text(&out.stderr);
    for l in stderr.lines().filter(|l| l.contains("usage_limit_wait")) {
        assert!(l.starts_with("amplihack.recipe.usage_limit_wait "), "{l:?}");
        assert!(l.bytes().all(|b| (0x20..0x7f).contains(&b)), "{l:?}");
    }
}

#[test]
fn the_epoch_form_gives_its_reset() {
    let f = Fixture::new();
    // 2026-10-18 08:10:00 -07:00, built so the literal stays in one place.
    let msg = format!("Claude AI usage limit {}|1792336200", "reached");
    let mut c = f.step();
    c.env("LIMIT_MESSAGE", &msg);
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        events(&out)[0],
        "start reason=usage_limit harness=claude reset=2026-10-18 08:10:00 -07:00 \
         until=2026-10-18 08:12:00 -07:00 bound=reset+margin"
    );
}

#[test]
fn limits_a_wait_cannot_outlast_are_passed_up_at_once() {
    let prose = "Done. Note: the usage limit resets 8:30am (America/Los_Angeles).";
    for (message, stdout, reason) in [
        (SPEND_TEXT, "", "spend_limit"),
        (SPEND_WEEKLY_TEXT, "", "reset_too_far"),
        (WEEKLY_TEXT, "", "reset_too_far"),
        (
            "You've hit your session limit · try again later",
            "",
            "no_reset",
        ),
        // Neutral stderr: work markers such as "failed" outrank a limit.
        ("step stopped", prose, "no_limit_message"),
    ] {
        let f = Fixture::new();
        let mut c = f.step();
        c.env("LIMIT_MESSAGE", message).env("STDOUT_TEXT", stdout);
        let out = f.run(&mut c);
        assert_eq!(out.status.code(), Some(1), "{reason}: {}", describe(&out));
        assert_eq!(f.count("0"), 1, "{reason}: no rerun");
        assert_eq!(
            events(&out),
            [format!("skipped reason={reason}")],
            "{reason}"
        );
    }
}

#[test]
fn a_spend_limit_that_states_a_session_reset_waits_for_it() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("LIMIT_MESSAGE", SPEND_SESSION_TEXT);
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2);
    assert_eq!(
        events(&out),
        vec![
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 \
             until=2026-10-18 08:32:00 -07:00 bound=reset+margin",
            "end reason=reset reached",
            "resume attempt=2",
        ]
    );
}

#[test]
fn a_prompt_piped_on_stdin_is_not_waited_for() {
    // `-p` with no prompt text: the prompt is on the pipe. The value of a
    // hidden option (`--max-thinking-tokens`) or of an option this version
    // does not know is not a prompt either, so these skip the wait too.
    for claude_args in [
        &["-p"][..],
        &["-p", "--max-thinking-tokens", "8000"],
        &["-p", "--some-new-opt", "value"],
    ] {
        let f = Fixture::new();
        let mut args = vec!["claude", "--subprocess-safe", "--"];
        args.extend_from_slice(claude_args);
        let mut c = f.cmd(&args);
        c.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().expect("start amplihack");
        {
            use std::io::Write;
            let mut stdin = child.stdin.take().unwrap();
            stdin.write_all(b"the piped prompt\n").unwrap();
        }
        let out = child.wait_with_output().unwrap();
        assert_eq!(
            out.status.code(),
            Some(1),
            "{claude_args:?}: {}",
            describe(&out)
        );
        assert_eq!(
            f.count("0"),
            1,
            "{claude_args:?}: no rerun with an empty prompt"
        );
        assert_eq!(
            events(&out),
            ["skipped reason=piped_prompt"],
            "{claude_args:?}"
        );
    }
}

#[test]
fn a_held_open_stdin_pipe_does_not_hang_the_step() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("SKIP_STDIN", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().expect("start amplihack");
    // Held open, never written or closed, until the step has finished.
    let held = child.stdin.take();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || tx.send(child.wait_with_output()));
    let out = rx
        .recv_timeout(Duration::from_secs(60))
        .expect("the step hung on a held-open stdin pipe")
        .unwrap();
    drop(held);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "the retry ran");
}

#[test]
fn a_reset_inside_the_ten_minute_grace_retries_at_once() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env(
        "LIMIT_MESSAGE",
        "You've hit your session limit · resets 7:55am (America/Los_Angeles)",
    );
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        events(&out),
        vec![
            "start reason=usage_limit harness=claude reset=2026-10-18 07:55:00 -07:00 \
             until=2026-10-18 08:00:00 -07:00 bound=reset+margin",
            "end reason=reset reached",
            "resume attempt=2",
        ]
    );
    assert_eq!(f.count("0"), 2);
}

#[test]
fn ac7_a_second_limit_is_passed_up_after_one_wait() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("LIMIT_CALLS", "5");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "one wait, one retry, then give up");
    let ev = events(&out);
    assert_eq!(
        ev.iter().filter(|l| l.starts_with("start ")).count(),
        1,
        "{ev:?}"
    );
    assert_eq!(
        ev.last().map(String::as_str),
        Some("skipped reason=already_waited")
    );
    assert_eq!(claude_stdout(&out), "attempt 2 output\n");
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains(LIMIT_TEXT),
        "the runner's classifier must still see the limit: {stderr}"
    );
}

#[test]
fn st7_a_limit_message_on_exit_zero_never_waits() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("LIMIT_EXIT", "0");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 1);
    assert!(wait_lines(&out).is_empty(), "{:?}", wait_lines(&out));
}

#[test]
fn a_failure_that_is_not_a_usage_limit_never_waits() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("LIMIT_MESSAGE", "fatal: something unrelated broke");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 1);
    assert!(wait_lines(&out).is_empty(), "{:?}", wait_lines(&out));
}

#[test]
fn a_launch_that_is_not_a_print_step_is_left_alone() {
    let f = Fixture::new();
    let mut c = f.cmd(&["claude", "--subprocess-safe", "--", "step"]);
    c.stdin(Stdio::null());
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 1);
    assert!(wait_lines(&out).is_empty(), "{:?}", wait_lines(&out));
}

#[test]
fn a_launch_that_is_already_supervised_never_waits() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("AMPLIHACK_AUTO_SWITCH_SUPERVISED", "1");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(
        f.count("0"),
        1,
        "a nested launch cannot start a second wait"
    );
    assert!(wait_lines(&out).is_empty(), "{:?}", wait_lines(&out));
}
