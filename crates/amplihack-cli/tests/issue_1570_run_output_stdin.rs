//! Issue #1570: `amplihack_cli::util::run_output_with_timeout_stdin`.
//!
//! Contract (docs/reference/large-payload-delivery.md, section
//! "Rust API: amplihack_cli::util::run_output_with_timeout_stdin"):
//!
//! | Behaviour     | Detail                                                        |
//! | ------------- | ------------------------------------------------------------- |
//! | Payload Some  | stdin is piped; the writer thread starts after spawn          |
//! | Payload None  | stdin is `Stdio::null()` (this entry point only)              |
//! | Reader closed | `BrokenPipe`/`ConnectionReset`; the child's exit status decides |
//! | Writer error  | Any other error: `Err` naming only the program and error kind |
//! | Timeout       | The child is killed; the writer thread is not joined          |

#![cfg(target_os = "linux")]

use amplihack_cli::util::run_output_with_timeout_stdin;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// More than 128 KiB (one Linux `MAX_ARG_STRLEN`) of text that mixes 1-, 2-,
/// 3- and 4-byte UTF-8 sequences with shell metacharacters and newlines.
fn multibyte_payload(min_bytes: usize) -> Vec<u8> {
    let pattern = "issue 1570: 'quotes' \"dq\" $(not run) `tick` é € 𝄞 中文\n";
    let mut text = String::with_capacity(min_bytes + pattern.len());
    while text.len() < min_bytes {
        text.push_str(pattern);
    }
    text.into_bytes()
}

#[test]
fn some_payload_reaches_cat_byte_for_byte() {
    let payload = multibyte_payload(200 * 1024);
    assert!(payload.len() > 131_072);

    let output = run_output_with_timeout_stdin(
        Command::new("cat"),
        Duration::from_secs(30),
        Some(payload.clone()),
    )
    .expect("cat with a stdin payload must run to completion");

    assert!(output.status.success(), "cat failed: {:?}", output.status);
    assert_eq!(
        output.stdout.len(),
        payload.len(),
        "cat must echo every payload byte"
    );
    assert!(
        output.stdout == payload,
        "stdin payload must arrive byte-for-byte (multi-byte UTF-8 intact)"
    );
}

#[test]
fn one_mebibyte_echoed_through_a_pipe_does_not_deadlock() {
    // The child writes everything it reads back to stdout, so a writer that
    // ran on the calling thread before the stdout readers started would
    // deadlock once both pipe buffers filled.
    let payload = multibyte_payload(1024 * 1024);
    let started = Instant::now();

    let output = run_output_with_timeout_stdin(
        Command::new("cat"),
        Duration::from_secs(30),
        Some(payload.clone()),
    )
    .expect("1 MiB through cat must not hit the timeout");

    assert!(output.status.success());
    assert!(output.stdout == payload, "1 MiB payload must round-trip");
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "stdin writer and stdout readers must run concurrently"
    );
}

#[test]
fn none_payload_gives_cat_immediate_eof() {
    let started = Instant::now();
    let output = run_output_with_timeout_stdin(Command::new("cat"), Duration::from_secs(10), None)
        .expect("cat with no payload must exit at once on EOF");

    assert!(output.status.success());
    assert!(
        output.stdout.is_empty(),
        "no payload means empty stdin; cat printed {} bytes",
        output.stdout.len()
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn none_payload_stdin_is_dev_null_not_inherited() {
    let mut cmd = Command::new("readlink");
    cmd.arg("/proc/self/fd/0");
    let output = run_output_with_timeout_stdin(cmd, Duration::from_secs(10), None).unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "/dev/null",
        "with no payload the child's stdin must be Stdio::null(), never inherited"
    );
}

#[test]
fn none_payload_overrides_a_stdin_the_caller_left_open() {
    // A pipe whose write end the test keeps open: a child that inherited it
    // would block in `cat` until the timeout. `Stdio::null()` gives EOF.
    let (reader, _writer_kept_open) = std::io::pipe().expect("os pipe");
    let mut cmd = Command::new("cat");
    cmd.stdin(Stdio::from(reader));

    let result = run_output_with_timeout_stdin(cmd, Duration::from_secs(3), None);

    let output = result.expect(
        "with no payload stdin must be replaced by Stdio::null(), so cat sees EOF and exits",
    );
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn child_that_never_reads_stdin_still_reports_its_own_status() {
    let payload = multibyte_payload(1024 * 1024);
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", "echo ran; exit 3"]);

    let output = run_output_with_timeout_stdin(cmd, Duration::from_secs(30), Some(payload))
        .expect("the child closing stdin early (EPIPE) must not replace its result");

    assert_eq!(
        output.status.code(),
        Some(3),
        "the child's exit status must be returned unchanged"
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "ran");
}

#[test]
fn child_that_reads_part_of_stdin_still_reports_success() {
    let payload = multibyte_payload(1024 * 1024);
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", "head -c 16 >/dev/null; echo partial"]);

    let output = run_output_with_timeout_stdin(cmd, Duration::from_secs(30), Some(payload))
        .expect("partial read must not turn into an error");

    assert!(output.status.success(), "status: {:?}", output.status);
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "partial");
}

#[test]
fn timeout_kills_child_and_returns_while_writer_is_blocked() {
    // `sleep` never reads stdin, so the writer blocks once the pipe buffer
    // (64 KiB) is full. The call must still return near the timeout.
    let payload = multibyte_payload(4 * 1024 * 1024);
    let temp = tempfile::tempdir().unwrap();
    let pid_file = temp.path().join("pid");
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", "printf '%s' \"$$\" > \"$1\"; exec sleep 30", "sh"])
        .arg(&pid_file);

    let started = Instant::now();
    let result = run_output_with_timeout_stdin(cmd, Duration::from_millis(500), Some(payload));
    let elapsed = started.elapsed();

    let error = result.expect_err("a child that sleeps past the timeout must time out");
    assert!(
        format!("{error:#}").contains("timed out after"),
        "timeout error must stay explicit; got {error:#}"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "a blocked stdin writer must not be joined after a timeout; took {elapsed:?}"
    );

    let pid: i32 = std::fs::read_to_string(&pid_file)
        .expect("child must have recorded its pid")
        .trim()
        .parse()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut alive = true;
    while Instant::now() < deadline {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        // Gone, or a zombie awaiting reaping, both mean it was killed.
        if stat.is_empty() || stat.contains(") Z ") {
            alive = false;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if alive {
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
    }
    assert!(!alive, "the timed-out child (pid {pid}) must be killed");
}

#[test]
fn a_descendant_holding_stdin_open_does_not_outlast_the_timeout() {
    // The child exits at once, but its background `sleep` inherits stdin and
    // never reads it, so the 1 MiB write cannot finish. The writer join is
    // bounded, so the call returns with the child's status well within the
    // timeout. The descendant's output goes to /dev/null: readers blocking on
    // an inherited stdout are a separate, older limitation.
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("sleep.pid");
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(r#"sleep 30 >/dev/null 2>&1 & printf '%s' "$!" > "$1"; exit 0"#)
        .arg("sh")
        .arg(&pid_file);
    let started = Instant::now();
    let result = run_output_with_timeout_stdin(
        cmd,
        Duration::from_secs(20),
        Some(multibyte_payload(1024 * 1024)),
    );
    let elapsed = started.elapsed();
    if let Ok(pid) = std::fs::read_to_string(&pid_file) {
        let _ = Command::new("kill")
            .arg("-KILL")
            .arg(pid.trim())
            .stderr(Stdio::null())
            .status();
    }
    let output = result.expect("the child's status must be returned");
    assert!(output.status.success(), "status: {:?}", output.status);
    assert!(
        elapsed < Duration::from_secs(10),
        "the writer join must be bounded; took {elapsed:?}"
    );
}
