//! Issue #1618: a recipe step's stdin and the usage-limit wait.
//!
//! - A prompt piped on stdin (`-p` with no prompt word) is waited for since
//!   issue #2815: the supervisor keeps a private copy of it, so the step
//!   runs again with the same input. The value of a hidden option or of an
//!   option this Claude version does not know is not taken as the prompt
//!   either.
//! - A stdin pipe held open by the caller does not hang the step. The case
//!   here is the one where no copy is made, because the prompt is in argv:
//!   nothing in this file reaches the copier. The matching case on the
//!   capture path, where a copier does run on a pipe the caller never
//!   closes, is `a_held_open_captured_pipe_does_not_hang_the_step` in
//!   `issue_2815_piped_prompt_wait.rs` (issue #2816).
//!
//! See `issue_1618_fixture`.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, describe, events};
use std::process::Stdio;
use std::time::Duration;

#[test]
fn a_prompt_piped_on_stdin_is_waited_for_and_replayed() {
    // `-p` with no prompt text: the prompt is on the pipe. The value of a
    // hidden option (`--max-thinking-tokens`) or of an option this version
    // does not know is not a prompt either, so these are captured too.
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
        assert!(out.status.success(), "{claude_args:?}: {}", describe(&out));
        assert_eq!(f.count("0"), 2, "{claude_args:?}: the rerun ran");
        let want = crate::issue_1618_fixture::sha256_hex(b"the piped prompt\n");
        assert_eq!(f.call("0", 1)["STDIN"], want, "{claude_args:?}");
        assert_eq!(f.call("0", 2)["STDIN"], want, "{claude_args:?}");
        let ev = events(&out);
        assert_eq!(ev.len(), 3, "{claude_args:?}: {ev:?}");
        assert!(ev[0].starts_with("start reason=usage_limit"), "{ev:?}");
        assert_eq!(ev[2], "resume attempt=2", "{claude_args:?}");
    }
}

/// The prompt is in argv (`f.step()` is `-p step`), so `prompt_in_args` is
/// true and stdin is never copied. This is the guard for the path that
/// leaves stdin alone; the capture path has its own, named in the module
/// comment above.
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
