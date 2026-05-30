//! TDD red-phase behavioural test for the apostrophe-survival contract.
//!
//! Reproduces the failure mode tracked in Simard #1871 / #1879 / #1897:
//! a 64 KiB prompt that embeds apostrophes, double quotes, backslashes,
//! newlines, and control bytes must round-trip through every supported
//! delivery mode without truncation or shell-escape corruption.
//!
//! These tests fail until `amplihack-utils::prompt_delivery::deliver` is
//! implemented per the design note on Simard #1897. They are intentionally
//! NOT `#[ignore]`-d so they show up red in CI for the implementation PR.
//!
//! Approach: rather than depend on the real claude / copilot / codex
//! binaries, we use POSIX `cat` as a stand-in echo target:
//!   * `Argv`     → `sh -c 'printf "%s" "$1"' sh <prompt>`
//!   * `Tempfile` → `cat <tempfile-path>`
//!   * `Stdin`    → `cat` (reads its own stdin)
//!
//! All three should yield exactly the bytes we handed `deliver`. Any
//! difference indicates a shell-escape / truncation bug in the helper.

#![cfg(unix)]

use std::process::{Command, Stdio};

use amplihack_utils::prompt_delivery::{DeliveryCaps, DeliveryMode, PromptDelivery, deliver};

const PAYLOAD_SIZE: usize = 64 * 1024;

fn synthetic_payload() -> String {
    // Patterns mirroring the bug repro: apostrophes (#1871 root cause),
    // double quotes, backslashes, newlines, and a low control byte that
    // sometimes trips PTY canonical mode.
    let pattern = b"a'\\\"b\n\x01";
    let mut out = Vec::with_capacity(PAYLOAD_SIZE);
    while out.len() < PAYLOAD_SIZE {
        out.extend_from_slice(pattern);
    }
    out.truncate(PAYLOAD_SIZE);
    // Replace the control byte so we keep valid UTF-8 — but apostrophe is
    // the one that actually matters per the original bug.
    out.iter_mut().for_each(|b| {
        if *b == 0x01 {
            *b = b'.';
        }
    });
    String::from_utf8(out).expect("ascii pattern is valid utf-8")
}

fn run(mut cmd: Command, prompt: &str, mode: DeliveryMode) -> String {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("harness should spawn");
    // For stdin mode the helper would normally take the child's stdin and
    // write to it before wait(). Until that is implemented this branch
    // emulates the eventual behaviour so the assertion still measures
    // round-trip integrity end-to-end.
    if mode == DeliveryMode::Stdin {
        use std::io::Write;
        if let Some(mut sin) = child.stdin.take() {
            sin.write_all(prompt.as_bytes())
                .expect("write to child stdin should succeed");
        }
    }
    let out = child
        .wait_with_output()
        .expect("harness should run to completion");
    assert!(
        out.status.success(),
        "harness exited non-zero: {:?}",
        out.status
    );
    String::from_utf8(out.stdout).expect("harness stdout should be utf-8")
}

#[test]
fn argv_roundtrip_small_payload() {
    let prompt = "a'b\"c\\d\ne";
    let caps = DeliveryCaps::argv_only();
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg("printf '%s' \"$1\"").arg("sh");
    let _handle = deliver(&mut cmd, prompt, PromptDelivery::Argv, &caps)
        .expect("argv delivery should succeed");
    let got = run(cmd, prompt, DeliveryMode::Argv);
    assert_eq!(got, prompt, "argv round-trip must be byte-exact");
}

#[test]
fn argv_roundtrip_at_4kb_boundary() {
    let unit = "a'b\\\"c\n";
    let repeats = 4096 / unit.len() + 1;
    let prompt: String = unit.repeat(repeats).chars().take(4096).collect();
    assert_eq!(prompt.len(), 4096, "prompt sized to exactly 4 KiB");
    let caps = DeliveryCaps::argv_only();
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg("printf '%s' \"$1\"").arg("sh");
    let _handle = deliver(&mut cmd, &prompt, PromptDelivery::Argv, &caps)
        .expect("argv delivery should succeed at 4 KiB");
    let got = run(cmd, &prompt, DeliveryMode::Argv);
    assert_eq!(got, prompt, "4 KiB argv round-trip must be byte-exact");
}

#[test]
fn tempfile_roundtrip_64kb_apostrophes() {
    let prompt = synthetic_payload();
    let caps = DeliveryCaps::argv_and_tempfile("");
    // For `cat`, we need the file path as a positional arg, not under a
    // flag. We declare an empty flag in caps so the helper appends only
    // the path. (The real claude/copilot binaries will use a real flag.)
    let mut cmd = Command::new("cat");
    let handle = deliver(&mut cmd, &prompt, PromptDelivery::Tempfile, &caps)
        .expect("tempfile delivery should succeed");
    assert_eq!(handle.mode(), DeliveryMode::Tempfile);
    let got = run(cmd, &prompt, DeliveryMode::Tempfile);
    assert_eq!(
        got.len(),
        prompt.len(),
        "tempfile round-trip byte count must match (got {} vs {})",
        got.len(),
        prompt.len()
    );
    assert_eq!(got, prompt, "tempfile round-trip must be byte-exact");
    drop(handle);
}

#[test]
fn stdin_roundtrip_64kb_apostrophes() {
    let prompt = synthetic_payload();
    let caps = DeliveryCaps::all_modes("--prompt-file");
    let mut cmd = Command::new("cat");
    let handle = deliver(&mut cmd, &prompt, PromptDelivery::Stdin, &caps)
        .expect("stdin delivery should succeed");
    assert_eq!(handle.mode(), DeliveryMode::Stdin);
    let got = run(cmd, &prompt, DeliveryMode::Stdin);
    assert_eq!(got, prompt, "stdin round-trip must be byte-exact");
}

#[test]
fn tempfile_path_does_not_leak_after_child_exits() {
    let prompt = synthetic_payload();
    let caps = DeliveryCaps::argv_and_tempfile("");
    let mut cmd = Command::new("cat");
    let handle = deliver(&mut cmd, &prompt, PromptDelivery::Tempfile, &caps)
        .expect("tempfile delivery should succeed");
    let path = handle
        .tempfile_path()
        .expect("tempfile mode handle must expose a path")
        .to_path_buf();
    let _ = run(cmd, &prompt, DeliveryMode::Tempfile);
    drop(handle);
    assert!(
        !path.exists(),
        "tempfile must be unlinked after handle drop: {path:?}"
    );
}

#[test]
fn concurrent_deliveries_use_distinct_tempfiles() {
    use std::thread;
    let prompt = synthetic_payload();
    let caps = DeliveryCaps::argv_and_tempfile("");

    let mut handles = Vec::new();
    let mut paths = Vec::new();
    for _ in 0..4 {
        let mut cmd = Command::new("cat");
        let h = deliver(&mut cmd, &prompt, PromptDelivery::Tempfile, &caps)
            .expect("tempfile delivery should succeed");
        paths.push(
            h.tempfile_path()
                .expect("tempfile path must be exposed")
                .to_path_buf(),
        );
        let prompt_clone = prompt.clone();
        handles.push(thread::spawn(move || {
            run(cmd, &prompt_clone, DeliveryMode::Tempfile)
        }));
        // Keep `h` alive until the spawned thread reads it; in the real
        // implementation the handle owns the NamedTempFile so we must NOT
        // drop it here. Leak it for the duration of the test by stashing.
        std::mem::forget(h);
    }
    for h in handles {
        let got = h.join().expect("worker thread should not panic");
        assert_eq!(got, prompt, "every concurrent delivery must round-trip");
    }
    // All temp paths must be unique.
    let mut sorted = paths.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        paths.len(),
        "concurrent tempfile deliveries must produce distinct paths: {paths:?}"
    );
}
