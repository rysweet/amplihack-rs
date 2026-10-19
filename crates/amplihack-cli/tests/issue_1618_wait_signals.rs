//! Issue #1618, AC8: SIGINT, SIGTERM or SIGHUP during the usage-limit wait
//! ends it. The failed attempt's held-back stdout is printed, the stdin copy
//! is deleted, there is no retry, and the step exits 128 plus the signal.
//!
//! The wait runs in real time here (scale 1, a 32-minute deadline), so the
//! step is still waiting when the signal arrives. Each test runs in its own
//! process (`in_quiet_process`) so no sibling test shares its timing. See
//! `issue_1618_fixture`.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, WAIT_PREFIX, claude_stdout, describe, text};
use crate::support::quiet_process::in_quiet_process;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::process::{Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Starts a limited step, waits for its `start` line, sends `sig`, and
/// returns its output with the whole stderr.
fn signal_during_wait(f: &Fixture, sig: i32) -> Output {
    let input = f.root.join("stdin.bin");
    fs::write(&input, b"prompt for the step\n").unwrap();
    let mut c = f.step();
    c.env("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_SCALE", "1")
        .stdin(Stdio::from(fs::File::open(&input).unwrap()))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().expect("spawn amplihack");
    let pid = child.id() as libc::pid_t;

    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        let mut all = String::new();
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            let _ = tx.send(line.clone());
            all.push_str(&line);
            all.push('\n');
        }
        all
    });
    let mut stdout = child.stdout.take().unwrap();
    let out_reader = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = stdout.read_to_end(&mut v);
        v
    });

    let started = loop {
        match rx.recv_timeout(Duration::from_secs(60)) {
            Ok(l) if l.starts_with(&format!("{WAIT_PREFIX}start ")) => break true,
            Ok(_) => continue,
            Err(_) => break false,
        }
    };
    if !started {
        let _ = child.kill();
        let _ = child.wait();
        panic!(
            "the step never started waiting:\n{}",
            reader.join().unwrap_or_default()
        );
    }
    // SAFETY: plain kill(2) on our own child.
    assert_eq!(unsafe { libc::kill(pid, sig) }, 0);

    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the step did not exit within 30 s of signal {sig}");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    Output {
        status,
        stdout: out_reader.join().unwrap(),
        stderr: reader.join().unwrap().into_bytes(),
    }
}

fn assert_signal_ends_the_step(sig: i32) {
    let f = Fixture::new();
    let out = signal_during_wait(&f, sig);
    assert_eq!(out.status.code(), Some(128 + sig), "{}", describe(&out));
    assert_eq!(f.count("0"), 1, "no retry after a signal");
    let stderr = text(&out.stderr);
    let events: Vec<&str> = stderr
        .lines()
        .filter_map(|l| l.strip_prefix(WAIT_PREFIX))
        .filter(|l| !l.starts_with("progress "))
        .collect();
    assert_eq!(events.len(), 2, "{events:?}");
    assert!(events[0].starts_with("start "), "{events:?}");
    assert_eq!(events[1], "end reason=signal");
    assert_eq!(
        claude_stdout(&out),
        "attempt 1 output\n",
        "the held-back stdout is printed"
    );
    assert!(
        f.stdin_copies().is_empty(),
        "stdin copy left behind: {:?}",
        f.stdin_copies()
    );
}

#[test]
fn ac8_sigint_during_the_wait_exits_130_without_a_retry() {
    in_quiet_process(
        module_path!(),
        "ac8_sigint_during_the_wait_exits_130_without_a_retry",
        || assert_signal_ends_the_step(libc::SIGINT),
    );
}

#[test]
fn ac8_sigterm_during_the_wait_exits_143_without_a_retry() {
    in_quiet_process(
        module_path!(),
        "ac8_sigterm_during_the_wait_exits_143_without_a_retry",
        || assert_signal_ends_the_step(libc::SIGTERM),
    );
}

#[test]
fn ac8_sighup_during_the_wait_exits_129_without_a_retry() {
    in_quiet_process(
        module_path!(),
        "ac8_sighup_during_the_wait_exits_129_without_a_retry",
        || assert_signal_ends_the_step(libc::SIGHUP),
    );
}
