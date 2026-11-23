//! Issue #2912: what stops a standing session that exits at once on every
//! start while nothing holds it back, and what it costs until then.
//!
//! The wrapper relaunches every exit its owner did not cause, with no count,
//! no brake and no delay. A session that cannot start therefore loops as
//! fast as it can start and exit whenever a login has room, and also on a
//! host with no rotator. The wrapper does not stop that loop.
//! The stop is the supervisor agent's: its bundled `supervise-agent-sessions`
//! skill reads the record and the log, judges a start-up crash loop from
//! them and unregisters the session.
//!
//! These tests run the loop against a stand-in that exits 1 at once on every
//! start, let it spin, and then take one pass of the skill on the evidence
//! the loop left: the alert in `amplihack agent status --json`, matched to
//! the skill's row, and the session's lines in the supervisor log, matched
//! to the evidence the skill's own table asks for before a stop. The pass
//! runs the command the skill's "Stop a crash loop" section gives. The loop
//! then ends within one more start, refuses with `reason=superseded`, and
//! the supervisor service does not bring the session back.
//!
//! Every registry and log write is counted from the kernel's inotify events
//! on `~/.amplihack/agents/`, not inferred from the code: one registry write
//! and one log line for each relaunch, before and after the pass. That is
//! the cost of the loop until a supervisor reads it.
//!
//! The judgement here is the skill's table applied by the test; on a host it
//! is an agent's. The tests show that the pass the skill sets out ends the
//! loop and what the loop costs until it does. They cannot show that a
//! supervisor agent runs on a given host: where none does, nothing in
//! amplihack stops the loop, as docs/howto/keep-sessions-running.md says
//! under "Who reads the record".

#![cfg(target_os = "linux")]

use crate::issue_1760_fixture::*;
use crate::issue_2912_standing_relaunch::{
    NAME, Running, alert, install_stand_in, relaunch_lines, runs, set_standing, swap, wrapped,
};
use crate::issue_2912_supervisor_skill::{skill_row_for, stop_command, stop_evidence};
use amplihack_utils::file_event::parse_events;
use serde_json::Value;
use std::ffi::CString;
use std::fs::{self, File};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::Stdio;
use std::time::Instant;

/// How many relaunches the loop makes before the pass. Enough that "many
/// relaunches within minutes" holds by any reading, and few enough that the
/// test ends in seconds on a loaded host.
const MANY: usize = 20;

/// Registry and log writes in one directory, read from inotify.
///
/// A registry write is an atomic rename onto `registry.json`
/// (`IN_MOVED_TO`); a log write is a `write(2)` to `supervisor.log`
/// (`IN_MODIFY`), one per line. The queue is drained while the loop runs, so
/// it never overflows, and an overflow would fail the test rather than
/// undercount.
struct Writes {
    fd: OwnedFd,
    registry: usize,
    log: usize,
}

impl Writes {
    fn watch(dir: &Path) -> Self {
        // SAFETY: no pointers; the flags are valid for inotify_init1.
        let raw = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        assert!(
            raw >= 0,
            "inotify_init1: {}",
            std::io::Error::last_os_error()
        );
        // SAFETY: `raw` is a new descriptor that nothing else owns.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let path = CString::new(dir.as_os_str().as_bytes()).unwrap();
        let mask = libc::IN_MOVED_TO | libc::IN_MODIFY;
        // SAFETY: a valid inotify descriptor and a NUL-terminated path.
        let wd = unsafe { libc::inotify_add_watch(fd.as_raw_fd(), path.as_ptr(), mask) };
        assert!(
            wd >= 0,
            "inotify_add_watch: {}",
            std::io::Error::last_os_error()
        );
        Self {
            fd,
            registry: 0,
            log: 0,
        }
    }

    /// Reads every queued event and adds it to the counts.
    fn drain(&mut self) {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            // SAFETY: `buf` is writable for its whole length.
            let n = unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                let e = std::io::Error::last_os_error();
                assert!(
                    n == 0 || e.kind() == std::io::ErrorKind::WouldBlock,
                    "reading inotify: {e}"
                );
                return;
            }
            let n = usize::try_from(n).unwrap();
            for e in parse_events(&buf[..n]).expect("well-formed inotify events") {
                assert_eq!(e.mask & libc::IN_Q_OVERFLOW, 0, "inotify dropped events");
                if e.mask & libc::IN_MOVED_TO != 0 && e.name == b"registry.json" {
                    self.registry += 1;
                }
                if e.mask & libc::IN_MODIFY != 0 && e.name == b"supervisor.log" {
                    self.log += 1;
                }
            }
        }
    }
}

/// The threads of process `pid` now, from `/proc/<pid>/status`.
fn threads(pid: u32) -> usize {
    fs::read_to_string(format!("/proc/{pid}/status"))
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("Threads:"))
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or(0)
}

/// The `N` of an alert that says `N relaunches since <ts> without a reply`.
fn chain_of(alert: &str) -> usize {
    let (_, tail) = alert.split_once("; ").expect("a relaunch alert");
    tail.split_whitespace()
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no count in {alert:?}"))
}

/// The alert `amplihack agent status --json` gives this session: step 1 of
/// the skill's pass.
fn status_alert(fx: &Fx) -> String {
    let out = fx.run(&["agent", "status", "--json"]);
    expect_code(&out, 0);
    let v: Value = serde_json::from_str(&stdout(&out)).expect("status --json is JSON");
    let agents = v["agents"].as_array().expect("an agents array");
    let me = agents
        .iter()
        .find(|a| a["name"] == NAME)
        .expect("the session");
    me["alert"].as_str().expect("an alert").to_string()
}

/// The pass the skill sets out, on what the loop has left: the alert matched
/// to its row, the log matched to the evidence the skill's table asks for
/// before a stop, and then the skill's stop command. Returns the alert.
fn skill_pass(fx: &Fx) -> String {
    let alert = status_alert(fx);
    assert_eq!(
        skill_row_for(&alert),
        Some("relaunched in its pane at"),
        "the skill's row for the alert: {alert}"
    );
    assert!(
        alert.ends_with("without a reply from the model in its conversation"),
        "{alert}"
    );
    assert!(chain_of(&alert) >= MANY, "{alert}");

    let evidence = stop_evidence();
    for part in ["within minutes", "`waited=none` on each line", "no reply"] {
        assert!(evidence.contains(part), "{part:?} is not in {evidence:?}");
    }
    let log = fx.log_lines();
    let mine: Vec<String> = log
        .into_iter()
        .filter(|l| l.contains(&format!(" session={NAME} ")))
        .collect();
    let lines = relaunch_lines(&mine);
    assert!(lines.len() >= MANY, "{mine:?}");
    assert!(
        lines.iter().all(|l| l.ends_with(" waited=none")),
        "{mine:?}"
    );
    let first = parse_ts(&lines[0][..20]);
    let last = parse_ts(&lines[lines.len() - 1][..20]);
    assert!(
        last - first <= 300,
        "the relaunches span {} s",
        last - first
    );

    let cmd = stop_command(NAME);
    assert_eq!(cmd, ["agent", "unregister", NAME], "the skill's stop");
    let args: Vec<&str> = cmd.iter().map(String::as_str).collect();
    expect_code(&fx.run(&args), 0);
    alert
}

/// Why the room gate lets every relaunch through at once.
#[derive(Debug, Clone, Copy)]
enum Gate {
    /// The rotator's swap record shows a login with room.
    Room,
    /// There is no rotator: the profile root has no swap record.
    NoRotator,
}

#[test]
fn a_session_that_exits_at_once_spins_until_the_skills_stop_ends_it() {
    for gate in [Gate::Room, Gate::NoRotator] {
        let fx = Fx::new();
        let dir = fx.dir("docs");
        install_stand_in(&fx, &dir, &["spin:1"]);
        set_standing(&fx, true);
        fx.private_agents_dir();
        let mut writes = Writes::watch(&fx.agents_dir());
        let mut c = wrapped(&fx, &dir, &[], &[]);
        if let Gate::Room = gate {
            swap(&fx, 1, 10, 0o600);
        }
        let pane = fx.records.join("pane");
        c.stderr(Stdio::from(File::create(&pane).unwrap()));
        let began = Instant::now();
        let w = Running::spawn(&fx, c);

        let spun = wait_until(|| {
            writes.drain();
            relaunch_lines(&fx.log_lines()).len() >= MANY
        });
        assert!(
            spun,
            "{gate:?}: the loop did not spin: {:?}",
            fx.log_lines()
        );
        let took = began.elapsed();
        // Each run's id thread ends with its run. Were it left to its next
        // one-second tick, the wrapper would hold one for every run of the
        // last second: 18 at the rate this loop runs on a debug build.
        let live_threads = threads(w.pid());
        assert!(live_threads < MANY / 2, "{gate:?}: {live_threads} threads");
        writes.drain();
        let (reg_before, log_before) = (writes.registry, writes.log);
        let lines_before = relaunch_lines(&fx.log_lines()).len();

        let seen = skill_pass(&fx);
        let starts_at_stop = runs(&fx).len();
        let out = w.finish();
        expect_code(&out, 1);
        writes.drain();

        let starts = runs(&fx).len();
        assert!(
            starts <= starts_at_stop + 1,
            "{gate:?}: {starts} starts, {starts_at_stop} when the stop returned"
        );
        let log = fx.log_lines();
        let relaunches = relaunch_lines(&log).len();
        assert_eq!(starts, relaunches + 1, "{gate:?}: one start per relaunch");
        let refused: Vec<_> = log
            .iter()
            .filter(|l| l.contains(" standing session refused "))
            .collect();
        assert_eq!(refused.len(), 1, "{gate:?}: {log:?}");
        assert!(refused[0].ends_with(" reason=superseded"), "{}", refused[0]);
        assert_eq!(
            writes.registry,
            relaunches + 2,
            "{gate:?}: the start, one per relaunch and the unregister"
        );
        assert_eq!(
            writes.log,
            relaunches + 1,
            "{gate:?}: one line per relaunch and the refusal"
        );
        let pane = fs::read_to_string(&pane).unwrap();
        assert_eq!(
            relaunch_lines(&pane.lines().map(String::from).collect::<Vec<_>>()).len(),
            relaunches,
            "{gate:?}: the pane has each relaunch line"
        );
        assert!(alert(&fx).is_empty(), "the record is gone");

        // The stop holds: the supervisor service has no record to resume.
        expect_code(&fx.supervise_once(&["--gap", "0"]), 0);
        assert!(fx.new_sessions().is_empty(), "{:?}", fx.new_sessions());

        eprintln!(
            "{gate:?}: {lines_before} relaunches in {} ms before the pass \
             ({reg_before} registry writes, {log_before} log writes, \
             {live_threads} threads in the wrapper); the alert read {} \
             relaunches; {relaunches} relaunches, {} registry writes and {} \
             log writes in all, {} starts after the stop returned",
            took.as_millis(),
            chain_of(&seen),
            writes.registry,
            writes.log,
            starts - starts_at_stop
        );
    }
}

/// The other way the room gate never waits is a profile root that cannot be
/// found (`Standing::resolve` warns, and the relaunch would not wait). It
/// does not spin either: the launch path refuses the same root before any
/// Claude Code starts, so the wrapper refuses once with `launch-failed`,
/// which is not relaunched, and leaves the session for the supervisor.
#[test]
fn a_profile_root_that_cannot_be_found_stops_the_launch_and_does_not_spin() {
    let fx = Fx::new();
    let dir = fx.dir("docs");
    install_stand_in(&fx, &dir, &["spin:1"]);
    set_standing(&fx, true);
    let mut c = wrapped(&fx, &dir, &[], &[]);
    c.env("AMPLIHACK_CLAUDE_PROFILE_ROOT", "not/absolute");
    let out = Running::spawn(&fx, c).finish();
    assert!(!out.status.success(), "{:?}", out.status);
    let err = stderr(&out);
    assert!(
        err.contains("the profile root could not be found"),
        "the standing warning: {err}"
    );
    assert_eq!(
        runs(&fx).len(),
        0,
        "no Claude Code started: {:?}",
        runs(&fx)
    );
    let log = fx.log_lines();
    assert_eq!(relaunch_lines(&log).len(), 0, "{log:?}");
    assert_eq!(
        log,
        [format!(
            "{} standing session refused session={NAME} reason=launch-failed",
            &log[0][..20]
        )],
        "{err}"
    );
    assert!(
        alert(&fx).starts_with("not relaunched: the launch failed"),
        "{}",
        alert(&fx)
    );
}
