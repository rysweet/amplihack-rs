//! Issue #2815: a usage limit in a step whose prompt arrives on stdin is
//! waited out, and the step runs again with the same prompt.
//!
//! The stub `claude` of `issue_1618_fixture` hashes all of its stdin and
//! reports a limit on its first call. The prompt is piped (`-p` with no
//! prompt word), so it is not in argv. The wait runs on the test clock the
//! runner already has (`AMPLIHACK_USAGE_LIMIT_WAIT_TEST_NOW` and
//! `..._TEST_SCALE`), so the run takes seconds. `SKIP_STDIN` is never set.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, describe, events, sha256_hex, text};
use std::io::Write;
use std::process::{Child, Stdio};

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
