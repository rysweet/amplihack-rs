//! Issue #1618: a recipe step's stdin and the usage-limit wait.
//!
//! - A prompt piped on stdin (`-p` with no prompt word) is never waited for:
//!   the pipe is gone after the first attempt, so a rerun would have no
//!   prompt. The value of a hidden option or of an option this Claude
//!   version does not know is not taken as the prompt either.
//! - A stdin pipe held open by the caller does not hang the step.
//!
//! See `issue_1618_fixture`.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, describe, events};
use std::process::Stdio;
use std::time::Duration;

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
