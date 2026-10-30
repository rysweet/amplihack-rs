//! `amplihack wait` (issue #1794): block in-process for a number of seconds,
//! or until a PID exits within a deadline.
//!
//! Agents used `tail -f /dev/null` or `sleep` loops to wait, which left
//! orphaned processes behind. This command starts no child process, reads no
//! environment variable and writes nothing to stdout. The only file it opens
//! is `/proc/<pid>/stat` on Linux, to see whether the target is a zombie.
//!
//! Exit codes: 0 when the wait is over, 2 for invalid arguments (reported by
//! clap through [`parse_condition`]), 124 when `--deadline` passes first, the
//! same code `timeout` uses.

use anyhow::Result;
use std::time::Duration;

/// The one supported condition form, named in every parse error.
const SUPPORTED_FORM: &str = "pid-exited:<PID>";

/// How often `--until` checks the condition.
#[cfg(unix)]
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Exit code when the deadline passes before the condition holds.
#[cfg(unix)]
const DEADLINE_EXIT_CODE: i32 = 124;

/// A condition `--until` waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Condition {
    /// The process with this PID (always 1 or more) has exited.
    PidExited(i32),
}

/// Parse `--until`. Used as the clap value parser, so every rejection here
/// exits 2 with the message.
///
/// Only `pid-exited:<PID>` is accepted, with a PID from 1 to `i32::MAX`
/// written as plain digits. The command's own PID is rejected, because
/// waiting on it could only end at the deadline.
pub fn parse_condition(value: &str) -> Result<Condition, String> {
    let Some((kind, pid)) = value.split_once(':') else {
        return Err(format!("expected {SUPPORTED_FORM}, got {value:?}"));
    };
    if kind != "pid-exited" {
        return Err(format!(
            "unknown condition kind {kind:?}; the supported form is {SUPPORTED_FORM}"
        ));
    }
    if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!(
            "PID must be a positive integer in {SUPPORTED_FORM}, got {pid:?}"
        ));
    }
    let pid: i32 = pid
        .parse()
        .map_err(|_| format!("PID {pid} is out of range (1 to {})", i32::MAX))?;
    if pid == 0 {
        return Err(format!("PID must be 1 or more in {SUPPORTED_FORM}"));
    }
    if u32::try_from(pid).is_ok_and(|pid| pid == std::process::id()) {
        return Err("amplihack wait cannot wait for its own PID".to_string());
    }
    Ok(Condition::PidExited(pid))
}

/// Run `amplihack wait`. Clap guarantees exactly one of `seconds` and
/// `until`, and that `deadline` is given with `until` only.
///
/// Returns `Ok(())` when the wait is over. When the deadline passes first it
/// reports this on stderr and ends the process with exit code 124.
#[cfg(unix)]
pub fn run(seconds: Option<u64>, until: Option<Condition>, deadline: Option<u64>) -> Result<()> {
    match (seconds, until, deadline) {
        (Some(seconds), None, None) => {
            std::thread::sleep(Duration::from_secs(seconds));
            Ok(())
        }
        (None, Some(Condition::PidExited(pid)), Some(deadline)) => wait_for_exit(pid, deadline),
        _ => anyhow::bail!(
            "amplihack wait needs either --seconds N, or --until {SUPPORTED_FORM} with --deadline SECS"
        ),
    }
}

/// Run `amplihack wait` on a platform without `kill(2)`: report that it is
/// unsupported and exit 2.
#[cfg(not(unix))]
pub fn run(_seconds: Option<u64>, _until: Option<Condition>, _deadline: Option<u64>) -> Result<()> {
    eprintln!("amplihack wait: unsupported on this platform (Unix only)");
    std::process::exit(2);
}

#[cfg(unix)]
fn wait_for_exit(pid: i32, deadline: u64) -> Result<()> {
    let limit = Duration::from_secs(deadline);
    let start = std::time::Instant::now();
    loop {
        if exited(pid) {
            return Ok(());
        }
        let remaining = limit.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            eprintln!("amplihack wait: deadline of {deadline} s passed; pid {pid} still running");
            std::process::exit(DEADLINE_EXIT_CODE);
        }
        std::thread::sleep(POLL_INTERVAL.min(remaining));
    }
}

/// Whether `pid` has exited. `kill(pid, 0)` failing with `ESRCH` means there
/// is no such process. A zombie (exited, not yet reaped by its parent) still
/// accepts the signal, so on Linux its state in `/proc/<pid>/stat` is also
/// checked. Any other failure counts as running, so the wait errs towards
/// waiting until the deadline.
#[cfg(unix)]
fn exited(pid: i32) -> bool {
    // SAFETY: `kill` with signal 0 delivers nothing; it only reports whether
    // the process exists and may be signalled. `pid` is 1 or more (checked by
    // `parse_condition`), so it never names a process group or every process.
    let rc = unsafe { libc::kill(pid, 0) };
    if rc != 0 {
        return std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
    }
    is_zombie(pid)
}

#[cfg(target_os = "linux")]
fn is_zombie(pid: i32) -> bool {
    std::fs::read(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| parse_stat_state(&stat))
        .is_some_and(|state| state == b'Z' || state == b'X')
}

#[cfg(all(unix, not(target_os = "linux")))]
fn is_zombie(_pid: i32) -> bool {
    // No /proc here: only `kill(pid, 0)` is used, so an unreaped zombie
    // counts as running until its parent reaps it or the deadline passes.
    false
}

/// The state byte of a `/proc/<pid>/stat` line. The command name in
/// parentheses may itself contain `)` and spaces, so the state is read after
/// the last `)`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_stat_state(stat: &[u8]) -> Option<u8> {
    let close = stat.iter().rposition(|&b| b == b')')?;
    stat.get(close.checked_add(2)?)
        .copied()
        .filter(u8::is_ascii_alphabetic)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn parse_condition_accepts_positive_pids() {
        assert_eq!(parse_condition("pid-exited:1"), Ok(Condition::PidExited(1)));
        assert_eq!(
            parse_condition("pid-exited:4242"),
            Ok(Condition::PidExited(4242))
        );
        assert_eq!(
            parse_condition("pid-exited:2147483647"),
            Ok(Condition::PidExited(i32::MAX))
        );
    }

    #[test]
    fn parse_condition_rejects_bad_pids_and_forms() {
        for bad in [
            "pid-exited:0",
            "pid-exited:-1",
            "pid-exited:-3",
            "pid-exited:2147483648",
            "pid-exited:99999999999999999999",
            "pid-exited:abc",
            "pid-exited:12a",
            "pid-exited: 12",
            "pid-exited:+12",
            "pid-exited:",
            "pid-exited",
            "1234",
            "",
            ":1",
            "file-exists:/tmp/x",
            "PID-EXITED:1",
            "Pid-Exited:1",
        ] {
            assert!(parse_condition(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn unknown_kind_error_lists_the_supported_form() {
        let err = parse_condition("file-exists:/tmp/x").unwrap_err();
        assert!(err.contains("pid-exited:<PID>"), "{err}");
    }

    #[test]
    fn own_pid_is_rejected() {
        // Rejected by the value parser, so clap reports it with exit 2.
        // Never call `run` with our own PID here: on the deadline it exits
        // the process with 124, which would end the test harness.
        let own = format!("pid-exited:{}", std::process::id());
        assert!(
            parse_condition(&own).is_err(),
            "waiting on our own PID must be rejected"
        );
    }

    #[test]
    fn stat_state_normal() {
        assert_eq!(
            parse_stat_state(b"1234 (sleep) S 1 1234 1234 0 -1"),
            Some(b'S')
        );
        assert_eq!(parse_stat_state(b"1234 (sleep) Z 1 1234"), Some(b'Z'));
    }

    #[test]
    fn stat_state_hostile_comm_uses_last_paren() {
        // A comm of `x) Z (` must not read as a zombie.
        assert_eq!(parse_stat_state(b"1234 (x) Z () R 1 1234"), Some(b'R'));
        assert_eq!(parse_stat_state(b"1234 (a b c) S 1"), Some(b'S'));
    }

    #[test]
    fn stat_state_non_utf8_comm() {
        assert_eq!(parse_stat_state(b"1234 (\xff\xfe) X 1 1234"), Some(b'X'));
    }

    #[test]
    fn stat_state_truncated_or_empty_is_none() {
        for input in [
            &b""[..],
            b"1234",
            b"1234 (sleep",
            b"1234 (sleep)",
            b"1234 (sleep) ",
            b")",
        ] {
            assert_eq!(parse_stat_state(input), None, "{input:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn our_own_process_is_not_exited() {
        let own = i32::try_from(std::process::id()).unwrap();
        assert!(!exited(own));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn reaped_child_is_exited_and_zombie_child_is_exited() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("0")
            .env_clear()
            .spawn()
            .unwrap();
        let pid = i32::try_from(child.id()).unwrap();
        // Unreaped: a zombie once it finishes. Reap before any assertion.
        let start = std::time::Instant::now();
        let mut seen = exited(pid);
        while !seen && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
            seen = exited(pid);
        }
        child.wait().unwrap();
        assert!(seen, "zombie child must read as exited");
        assert!(exited(pid), "reaped child must read as exited");
    }

    /// `run` returns `Ok(())` on success and calls `std::process::exit` only
    /// for exit 124, so the success path is safe to call in-process.
    #[cfg(unix)]
    #[test]
    fn seconds_zero_returns_ok() {
        assert!(run(Some(0), None, None).is_ok());
    }
}
