//! Issue #2019: contracts of the event-wait helpers in `test_support`.
//!
//! - `test_gate::Gate` (`new() -> Arc<Gate>`, `arrive_and_wait`,
//!   `wait_arrived(what)`, `open`): holds one side of a test at a known
//!   point. Used out of order it panics with a message containing `gate`.
//! - `wait::CEILING`: lib tests reach the one hang guard of
//!   `tests/support/wait.rs` through `#[path]`.
//! - `pidfd_wait::{parse_pid, PidFd, wait_pid_gone, KillOnDrop}` (Linux):
//!   wait on and kill a process the test did not spawn without trusting a
//!   pid that may have been reused.
//! - `exec_stub::write_executable` refuses setuid, setgid and sticky modes.
//!
//! None of these tests sleeps or measures real time. Each blocking step is
//! bounded only by `CEILING` and fails with "hung" if it is reached.

use super::test_gate::Gate;
use super::wait::CEILING;
use std::sync::mpsc;
use std::time::Duration;

fn panic_text(e: Box<dyn std::any::Any + Send>) -> String {
    e.downcast_ref::<String>()
        .cloned()
        .or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_string()))
        .unwrap_or_default()
}

// ── wait::CEILING ───────────────────────────────────────────────────────────

#[test]
fn lib_tests_share_the_600_second_ceiling() {
    assert_eq!(CEILING, Duration::from_secs(600));
}

#[test]
fn output_within_has_no_etxtbsy_retry() {
    // A stub that fails to start is a bug fixed by write_executable, not
    // something to retry.
    let src = include_str!("../../tests/support/wait.rs");
    assert!(
        !src.contains("BUSY_RETRIES"),
        "wait.rs must not retry ETXTBSY"
    );
    assert!(
        !src.contains("BUSY_PAUSE"),
        "wait.rs must not pause between spawns"
    );
    assert!(
        !src.contains("fn spawn_retrying"),
        "wait.rs must spawn once"
    );
}

// ── Gate ────────────────────────────────────────────────────────────────────

#[test]
fn a_handler_at_the_gate_stays_there_until_it_is_opened() {
    let gate = Gate::new();
    let (tx, rx) = mpsc::channel();
    let handler = {
        let gate = gate.clone();
        std::thread::spawn(move || {
            gate.arrive_and_wait();
            tx.send("passed").unwrap();
        })
    };
    gate.wait_arrived("the handler to reach the gate");
    // The handler is known to be inside `arrive_and_wait` with the gate
    // shut, so nothing can have been sent: no sleep is involved.
    assert!(
        matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)),
        "a shut gate let the handler through"
    );
    gate.open();
    assert_eq!(
        rx.recv_timeout(CEILING)
            .expect("hung: the gate was opened but the handler stayed"),
        "passed"
    );
    handler.join().unwrap();
}

#[test]
fn wait_arrived_returns_at_once_when_the_handler_arrived_first() {
    let gate = Gate::new();
    let (tx, rx) = mpsc::channel();
    let handler = {
        let gate = gate.clone();
        std::thread::spawn(move || {
            tx.send(()).unwrap();
            gate.arrive_and_wait();
        })
    };
    rx.recv_timeout(CEILING)
        .expect("hung: handler did not start");
    gate.wait_arrived("the handler to reach the gate");
    gate.open();
    handler.join().unwrap();
}

#[test]
fn a_handler_arriving_after_the_gate_opened_passes_straight_through() {
    let gate = Gate::new();
    gate.open();
    gate.arrive_and_wait();
}

#[test]
fn opening_a_gate_twice_panics_naming_the_gate() {
    let gate = Gate::new();
    gate.open();
    let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gate.open()))
        .expect_err("a second open must panic");
    assert!(panic_text(err).contains("gate"));
}

#[test]
fn waiting_for_arrival_after_opening_panics_naming_the_gate() {
    let gate = Gate::new();
    gate.open();
    let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        gate.wait_arrived("a handler")
    }))
    .expect_err("wait_arrived after open must panic");
    assert!(panic_text(err).contains("gate"));
}

#[test]
fn every_clone_is_the_same_gate() {
    let gate = Gate::new();
    let other = gate.clone();
    let handler = std::thread::spawn(move || other.arrive_and_wait());
    gate.wait_arrived("the clone to arrive");
    gate.open();
    handler.join().unwrap();
}

// ── exec_stub::write_executable ─────────────────────────────────────────────

#[cfg(unix)]
#[test]
fn write_executable_refuses_special_mode_bits_before_writing() {
    for mode in [0o4755, 0o2755, 0o1755, 0o7777] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stub");
        let err = std::panic::catch_unwind(|| {
            super::exec_stub::write_executable(&path, "#!/bin/sh\nexit 0\n", mode)
        })
        .expect_err("a setuid, setgid or sticky stub must be refused");
        assert!(
            panic_text(err).contains("0o7000"),
            "the refusal names the forbidden bits for mode {mode:o}"
        );
        assert!(!path.exists(), "nothing is written for mode {mode:o}");
    }
}

#[cfg(unix)]
#[test]
fn write_executable_gives_the_plain_mode_and_runs() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stub");
    super::write_executable(&path, "#!/bin/sh\nexit 7\n");
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o7777;
    assert_eq!(mode, 0o755);
    let status = std::process::Command::new(&path).status().unwrap();
    assert_eq!(status.code(), Some(7));
}

#[cfg(unix)]
#[test]
fn rewriting_a_running_stub_replaces_it_instead_of_failing() {
    use std::io::{BufRead, Write};
    use std::process::{Command, Stdio};

    // The original interpreter must execute its body before we replace it.
    // A shell builtin waits on stdin, so no FIFO open or grandchild can hang.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stub");
    super::write_executable(
        &path,
        "#!/bin/sh\nprintf 'ready\\n'\nread -r release\nexit 1\n",
    );
    struct Reap(std::process::Child);
    impl Drop for Reap {
        fn drop(&mut self) {
            super::wait::kill_and_reap(&mut self.0);
        }
    }
    let mut running = Reap(
        Command::new(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let stdout = running.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut line = String::new();
        let result = std::io::BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send(result.map(|_| line));
    });
    assert_eq!(
        rx.recv_timeout(CEILING)
            .expect("hung: original stub never announced readiness")
            .unwrap(),
        "ready\n"
    );
    reader.join().unwrap();
    super::write_executable(&path, "#!/bin/sh\nexit 0\n");
    assert!(
        super::wait::output_within("replacement stub", &mut Command::new(&path))
            .status
            .success()
    );
    running
        .0
        .stdin
        .take()
        .unwrap()
        .write_all(b"release\n")
        .unwrap();
    assert_eq!(
        super::wait::wait_child("original stub exit", &mut running.0).code(),
        Some(1)
    );
}

// ── pidfd_wait (Linux) ──────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
mod pidfd {
    use super::super::pidfd_wait::{KillOnDrop, PidFd, parse_pid, wait_pid_gone};
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Command, Stdio};

    #[test]
    fn parse_pid_accepts_plain_digits() {
        assert_eq!(parse_pid("4242").unwrap(), 4242);
    }

    #[test]
    fn parse_pid_accepts_one_trailing_newline_as_echo_writes_it() {
        assert_eq!(parse_pid("4242\n").unwrap(), 4242);
    }

    #[test]
    fn parse_pid_rejects_anything_but_ascii_digits() {
        for bad in [
            "", "\n", " 42", "42 ", "+42", "-42", "4_2", "0x2a", "42\n\n", "4\n2", "٤٢", "42.0",
        ] {
            assert!(parse_pid(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn parse_pid_rejects_0_1_and_our_own_pid() {
        assert!(parse_pid("0").is_err(), "0 is the process group");
        assert!(parse_pid("1").is_err(), "1 is init");
        let own = std::process::id().to_string();
        assert!(parse_pid(&own).is_err(), "the test's own pid");
    }

    #[test]
    fn parse_pid_rejects_values_beyond_pid_t() {
        assert!(parse_pid("2147483648").is_err());
        assert!(parse_pid("99999999999999999999").is_err());
    }

    /// `cat` with a piped stdin blocks until the stdin is closed: an exit the
    /// test controls without any sleep.
    fn blocked_cat() -> std::process::Child {
        Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("spawn cat")
    }

    #[test]
    fn wait_pid_gone_returns_once_the_process_exits() {
        let mut child = blocked_cat();
        let fd = PidFd::open(child.id() as i32).expect("pidfd of a live child");
        // Closing stdin lets cat exit.
        drop(child.stdin.take());
        wait_pid_gone(&fd, "cat to exit after its stdin closed");
        assert!(child.wait().unwrap().success());
    }

    #[test]
    fn kill_on_drop_kills_through_the_pidfd() {
        let mut child = blocked_cat();
        let stdin = child.stdin.take().unwrap();
        let fd = PidFd::open(child.id() as i32).expect("pidfd of a live child");
        drop(KillOnDrop(fd));
        let status = child.wait().unwrap();
        assert_eq!(status.signal(), Some(libc::SIGKILL), "{status:?}");
        drop(stdin);
    }

    #[test]
    fn opening_a_reaped_pid_reports_esrch() {
        let mut child = Command::new("/bin/true").spawn().unwrap();
        let pid = child.id() as i32;
        child.wait().unwrap();
        let err = PidFd::open(pid).expect_err("a reaped pid has no process");
        assert_eq!(err.raw_os_error(), Some(libc::ESRCH));
    }

    #[test]
    fn a_pidfd_outlives_the_reap_without_pointing_elsewhere() {
        // Opened while alive: after the reap the fd still refers to the dead
        // process, so waiting on it returns and killing it is a no-op.
        let mut child = blocked_cat();
        let fd = PidFd::open(child.id() as i32).unwrap();
        drop(child.stdin.take());
        child.wait().unwrap();
        wait_pid_gone(&fd, "an already reaped cat");
        drop(KillOnDrop(fd));
    }
}
