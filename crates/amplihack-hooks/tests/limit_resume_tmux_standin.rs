//! Issue #2695: with a rotator record showing room and `$TMUX_PANE` set, the
//! `limit-resume` hook runs `tmux send-keys` for its own pane, and for no
//! other target; with no room, or with no usable pane, it runs `tmux` not at
//! all.
//!
//! Contract (what the deputy's review of PR #2892 asked for):
//! - A stand-in `tmux` stands first on `PATH`. The hook starts its one
//!   program through `keys::tmux_send`, which is `Command::new("tmux")`, so
//!   the stand-in is reached by the same lookup a real session uses.
//! - With a swap record on disk onto a login with room, `$TMUX` and
//!   `$TMUX_PANE=%5`, the stand-in is run exactly four times, in order, each
//!   `send-keys -t %5` with `Escape`, `C-u`, `-l continue` and `Enter`. The
//!   outcome is `resumed-keys` with `keys_sent=4`; the exit code is 0 and
//!   nothing is written to stderr.
//! - With a swap record onto a login with no room (a window at 100%), the
//!   stand-in is never run. The hook goes on waiting, ends on its deadline
//!   (`no-capacity-timeout`) and exits 0.
//! - With no pane, the stand-in is never run, the hook exits 2 and writes
//!   exactly `RESUME_MSG` to stderr. "No pane" is `$TMUX_PANE` unset, an
//!   inherited `$TMUX_PANE` with no `$TMUX`, and an id that is not `%` and
//!   digits (here one that would add a second `-t` if it reached the argv).
//!
//! How the stand-in comes first on `PATH`. Changing the environment of a
//! running test process is `std::env::set_var`, which is unsafe and races
//! with the other threads of the test binary, so no test does it. Each
//! scenario instead re-runs this same test binary, once, for the one test,
//! with `env_clear()` and a `PATH` that is the stand-in's directory and
//! nothing else. The child takes the fixture branch of the test (see
//! [`run`]); the parent reads what the stand-in recorded.
//!
//! What is injected, and why. The ownership proof (`/proc` ancestry, tty and
//! foreground group) cannot run under `cargo test`, which has no terminal
//! ancestor, so the child's `Deps::pane_check` returns the verdict the
//! hook's own environment would give if the proof had passed: `Ready` for a
//! pane that parses, `None` for one that does not. The proof is covered
//! by `limit_resume_pane_ownership.rs`. Everything after the verdict is the
//! production path: the swap record is read from disk, the judgement is
//! `judge_at`, the commit is the real `FsGuard`, and the keys go through
//! `keys::tmux_send`.
//!
//! No test here talks to a tmux server, opens a real home or profile root,
//! sleeps, retries or polls. A child is run to completion with `output()`,
//! so no process is left to stop.

#![cfg(unix)]

use amplihack_hooks::limit_resume::guard::{FsGuard, GuardStore};
use amplihack_hooks::limit_resume::keys::{self, Key, Pane, PaneCheck};
use amplihack_hooks::limit_resume::log::{LogEntry, append, format_line};
use amplihack_hooks::limit_resume::{Deps, Event, RESUME_MSG, run_with_io};
use amplihack_utils::rotator_status::CycleStatus;
use amplihack_utils::rotator_status::swap_record::{
    SwapReading, SwapRecord, read_swap, rotate_dir,
};
use amplihack_utils::secure_files::write_private_atomic;
use serde_json::json;
use std::fs;
use std::io::{self, Cursor, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

/// Names the scenario the child runs. Unset in the parent.
const SCENARIO_ENV: &str = "AMPLIHACK_LIMIT_RESUME_STANDIN";
/// The scenario's private directory, for the child's sandbox proof.
const ROOT_ENV: &str = "AMPLIHACK_LIMIT_RESUME_STANDIN_ROOT";
/// First line after the shebang of the stand-in: how the child knows the
/// `tmux` it would run is the stand-in and not a real one.
const MARKER: &str = "# amplihack-limit-resume-standin-tmux";
/// The pane of every scenario that names one.
const PANE: &str = "%5";
/// A made-up session id and tmux socket, shaped like the real ones.
const SESSION: &str = "s2695";
const SOCKET_AND_SESSION: &str = "tmux.sock,4242,0";
/// What the stand-in records for each of the four keys, one line per run,
/// every argument in brackets so spaces cannot blur two arguments into one.
const EXPECTED_RECORD: [&str; 4] = [
    "[send-keys][-t][%5][Escape]",
    "[send-keys][-t][%5][C-u]",
    "[send-keys][-t][%5][-l][continue]",
    "[send-keys][-t][%5][Enter]",
];

const NOW: i64 = 1_792_592_000;

/// Scenarios take turns: a stand-in is written, made executable and run by
/// a child within one scenario, and no other scenario of this binary may
/// fork while it is open for writing (`ETXTBSY`).
static SERIAL: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scenario {
    /// Room, `$TMUX` and `$TMUX_PANE=%5`.
    Ready,
    /// A swap onto a login whose session window is full.
    NoRoom,
    /// Room, `$TMUX` set, `$TMUX_PANE` unset.
    NoPane,
    /// Room, `$TMUX_PANE=%5`, `$TMUX` unset: an inherited pane is no pane.
    PaneWithoutTmux,
    /// Room, `$TMUX` set, `$TMUX_PANE` not an id.
    MalformedPane,
}

impl Scenario {
    fn name(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::NoRoom => "no-room",
            Self::NoPane => "no-pane",
            Self::PaneWithoutTmux => "pane-without-tmux",
            Self::MalformedPane => "malformed-pane",
        }
    }

    /// The test function that runs this scenario, for `--exact`.
    fn test_fn(self) -> &'static str {
        match self {
            Self::Ready => "a_session_with_room_and_its_own_pane_gets_four_send_keys_for_that_pane",
            Self::NoRoom => "a_swap_onto_a_full_login_runs_tmux_not_at_all",
            Self::NoPane => "a_session_with_no_pane_runs_tmux_not_at_all",
            Self::PaneWithoutTmux => "an_inherited_pane_without_a_tmux_server_runs_tmux_not_at_all",
            Self::MalformedPane => "a_malformed_pane_id_runs_tmux_not_at_all",
        }
    }

    /// The `$TMUX` the child is given.
    fn tmux(self, root: &Path) -> Option<String> {
        match self {
            Self::PaneWithoutTmux => None,
            _ => Some(format!("{}/{SOCKET_AND_SESSION}", root.display())),
        }
    }

    /// The `$TMUX_PANE` the child is given.
    fn tmux_pane(self) -> Option<&'static str> {
        match self {
            Self::NoPane => None,
            // If this reached `send-keys -t` unparsed, the record would hold
            // a second target, `%9`.
            Self::MalformedPane => Some("%5 -t %9"),
            _ => Some(PANE),
        }
    }

    /// The session window of the login the rotator swaps to.
    fn swap_session_percent(self) -> u8 {
        if self == Self::NoRoom { 100 } else { 12 }
    }
}

// ── The parent: a sandbox, a stand-in, a child run ──────────────────────────

/// What a scenario left behind.
struct Ran {
    /// The hook's exit code, as the child printed it.
    exit: i32,
    /// What the hook wrote to its stderr, as the child printed it.
    stderr: String,
    /// How many times the hook asked for a key to be sent.
    attempts: u32,
    /// One entry per run of the stand-in, from its own record.
    record: Vec<String>,
    /// The hook's `resume.log`.
    log: String,
}

fn private_dir(p: &Path) {
    fs::create_dir(p).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
}

/// A stand-in `tmux` in `<root>/bin`: a shell script that appends its
/// arguments, in brackets, to `<root>/record` and exits 0. It reads neither
/// the environment nor its arguments as anything but text.
fn write_standin(root: &Path) -> PathBuf {
    let bin = root.join("bin");
    private_dir(&bin);
    let record = root.join("record");
    let record = record.to_str().unwrap();
    assert!(
        !record.contains('\'') && !record.contains('\n'),
        "the sandbox path must be quotable: {record}"
    );
    let script = format!(
        "#!/bin/sh\n{MARKER}\n{{\n  for a in \"$@\"; do printf '[%s]' \"$a\"; done\n  \
         printf '\\n'\n}} >> '{record}'\n"
    );
    let path = bin.join("tmux");
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&path)
        .unwrap();
    f.write_all(script.as_bytes()).unwrap();
    // Closed before any child can run it.
    drop(f);
    bin
}

fn parent(scenario: Scenario) -> Ran {
    let _turn = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let sandbox = tempfile::tempdir().unwrap();
    fs::set_permissions(sandbox.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let root = sandbox.path();
    let bin = write_standin(root);
    let home = root.join("home");
    private_dir(&home);

    let mut child = Command::new(std::env::current_exe().unwrap());
    child
        .args(["--exact", scenario.test_fn(), "--nocapture"])
        .env_clear()
        // The stand-in's directory and nothing else: the host's `tmux`, if
        // it has one, cannot be found whatever goes wrong.
        .env("PATH", &bin)
        .env("HOME", &home)
        .env(SCENARIO_ENV, scenario.name())
        .env(ROOT_ENV, root)
        .stdin(Stdio::null());
    if let Some(tmux) = scenario.tmux(root) {
        child.env("TMUX", tmux);
    }
    if let Some(pane) = scenario.tmux_pane() {
        child.env("TMUX_PANE", pane);
    }
    let out = child.output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success(),
        "the {} child failed: {:?}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        scenario.name(),
        out.status
    );
    // The child must have run its one test to its end. A filter that names
    // no test also exits 0, and would pass every "nothing was sent" check.
    assert!(
        stdout.contains("running 1 test") && stdout.contains("1 passed"),
        "the {} child did not run exactly its own test:\n{stdout}",
        scenario.name()
    );
    assert!(
        stdout.contains(&format!("@@standin complete {}", scenario.name())),
        "the {} child did not reach the end of its fixture:\n{stdout}",
        scenario.name()
    );

    let field = |key: &str| {
        let prefix = format!("@@standin {key}=");
        let mut found = stdout.lines().filter_map(|l| l.strip_prefix(&prefix));
        let value = found
            .next()
            .unwrap_or_else(|| panic!("the child printed no {key}:\n{stdout}"));
        assert!(found.next().is_none(), "the child printed {key} twice");
        value.to_string()
    };
    let record = match fs::read_to_string(root.join("record")) {
        Ok(text) => text.lines().map(str::to_string).collect(),
        // Never run: the stand-in creates the file on its first run.
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => panic!("the record cannot be read: {e}"),
    };
    let log = fs::read_to_string(resume_log(&root.join("profile"))).unwrap_or_default();
    Ran {
        exit: field("exit").parse().unwrap(),
        // The child prints stderr with `{:?}`; so is the expectation.
        stderr: field("stderr"),
        attempts: field("attempts").parse().unwrap(),
        record,
        log,
    }
}

fn debug_str(s: &str) -> String {
    format!("{s:?}")
}

// ── The child: the hook, run for real up to the pane verdict ────────────────

fn resume_log(profile: &Path) -> PathBuf {
    resume_dir(profile).join("resume.log")
}

fn resume_dir(profile: &Path) -> PathBuf {
    profile.join(".auto-switch.d").join("resume")
}

/// What the rotator writes after a swap at `at`, through the same private
/// atomic write it uses.
fn write_swap(profile: &Path, seq: u64, at: i64, session_percent: u8) {
    let dir = rotate_dir(profile);
    if !dir.exists() {
        private_dir(&dir);
    }
    let record = SwapRecord {
        from_org: None,
        to_org: None,
        affinity: None,
        seq,
        live_login: "spare".into(),
        from_login: "work".into(),
        swapped_at: at,
        reading: SwapReading {
            session: session_percent,
            week: 40,
            measured_at: at - 30,
        },
    };
    write_private_atomic(
        &dir,
        "swap.json",
        serde_json::to_string(&record).unwrap().as_bytes(),
    )
    .unwrap();
}

/// The hook's environment with the rotator, the clock and the pane verdict
/// scripted, and everything else the production one: the swap record is read
/// from disk, the guard and log are the real files under the sandbox, and a
/// key is sent by `keys::tmux_send`.
struct StandInDeps {
    profile: PathBuf,
    guard: FsGuard,
    clock: i64,
    start: i64,
    session_percent: u8,
    armed: bool,
    swaps: u64,
    /// Keys the hook asked to send.
    attempts: u32,
}

impl Deps for StandInDeps {
    fn auto_switch_on(&mut self) -> bool {
        true
    }
    fn now(&mut self) -> i64 {
        self.clock
    }
    fn read_cycle(&mut self, _now: i64) -> Option<CycleStatus> {
        Some(CycleStatus {
            at: Some(self.start - 120),
            measured_at: Some(self.start - 150),
            outcome: Some("stayed".into()),
            alert: false,
            live: Some("work".into()),
            overflow: None,
            platform_live: false,
            live_room: None,
        })
    }
    fn guard(&mut self) -> &mut dyn GuardStore {
        &mut self.guard
    }
    fn log(&mut self, now: i64, entry: &LogEntry) {
        append(&resume_dir(&self.profile), &format_line(now, entry));
    }
    fn rotator_down(&mut self, _now: i64) -> bool {
        false
    }
    fn arm(&mut self) -> io::Result<()> {
        self.armed = true;
        Ok(())
    }
    /// The first wake is the rotator's swap, 61 s into the wait; the second
    /// is the hook's own deadline. Nothing sleeps: the clock is a number.
    fn wait_event(&mut self, deadline: i64) -> Event {
        assert!(self.armed, "wait before arm");
        if self.swaps > 0 {
            self.clock = deadline;
            return Event::Deadline;
        }
        self.swaps += 1;
        self.clock = self.start + 61;
        write_swap(&self.profile, self.swaps, self.clock, self.session_percent);
        Event::Swap
    }
    fn read_swap(&mut self, now: i64) -> Option<SwapRecord> {
        read_swap(&self.profile, now)
    }
    fn stop_time(&mut self, _transcript: Option<&Path>, _start: i64) -> Option<i64> {
        None
    }
    /// The verdict the hook's own environment gives when the proof passes:
    /// the pane the session was started with, if it is one.
    fn pane_check(&mut self) -> PaneCheck {
        let var = |k: &str| std::env::var(k).ok();
        match Pane::parse(var("TMUX").as_deref(), var("TMUX_PANE").as_deref()) {
            Some(pane) => PaneCheck::Ready(pane),
            None => PaneCheck::None,
        }
    }
    fn send_keys(&mut self, pane: &Pane, key: Key) -> io::Result<()> {
        self.attempts += 1;
        keys::tmux_send(pane, key)
    }
    fn session_alive_now(&mut self) -> bool {
        true
    }
}

/// Stops the child before it can reach anything real: it must be the child
/// of [`parent`], under a private directory, with the stand-in as the only
/// `tmux` it can find.
fn prove_sandbox(scenario: Scenario) -> PathBuf {
    let root =
        PathBuf::from(std::env::var(ROOT_ENV).unwrap_or_else(|_| {
            panic!("{SCENARIO_ENV} is set but {ROOT_ENV} is not: not a child")
        }));
    assert!(root.is_absolute(), "the sandbox must be an absolute path");
    let meta = fs::symlink_metadata(&root).unwrap();
    assert!(meta.is_dir(), "the sandbox must be a directory");
    assert_eq!(
        meta.permissions().mode() & 0o777,
        0o700,
        "the sandbox must be private"
    );
    let bin = root.join("bin");
    assert_eq!(
        std::env::var_os("PATH").as_deref(),
        Some(bin.as_os_str()),
        "PATH must be the stand-in's directory alone"
    );
    let standin = fs::read_to_string(bin.join("tmux")).unwrap();
    assert!(
        standin.lines().nth(1) == Some(MARKER),
        "the tmux first on PATH is not the stand-in"
    );
    assert!(
        std::env::var_os("HOME").is_some_and(|h| Path::new(&h).starts_with(&root)),
        "HOME must be inside the sandbox"
    );
    // A `$TMUX` that is not the sandbox's would name a real server.
    match (scenario.tmux(&root), std::env::var("TMUX").ok()) {
        (Some(want), Some(got)) => assert_eq!(got, want, "TMUX must be the sandbox's"),
        (None, None) => {}
        (want, got) => panic!("TMUX is {got:?}, expected {want:?}"),
    }
    assert_eq!(
        std::env::var("TMUX_PANE").ok().as_deref(),
        scenario.tmux_pane(),
        "TMUX_PANE must be the scenario's"
    );
    root
}

/// The hook, from a `StopFailure` payload on stdin to an exit code, with the
/// rotator and the pane verdict scripted as described above. Prints what the
/// parent asserts on, one `@@standin` line each.
fn child(scenario: Scenario) {
    let root = prove_sandbox(scenario);
    let profile = root.join("profile");
    private_dir(&profile);
    private_dir(&profile.join(".auto-switch.d"));

    let mut deps = StandInDeps {
        profile: profile.clone(),
        guard: FsGuard::new(&profile),
        clock: NOW,
        start: NOW,
        session_percent: scenario.swap_session_percent(),
        armed: false,
        swaps: 0,
        attempts: 0,
    };
    let payload = json!({
        "hook_event_name": "StopFailure",
        "session_id": SESSION,
        "error": "rate_limit",
        "last_assistant_message": "You've hit your session limit"
    })
    .to_string();
    let mut stderr = Vec::new();
    let exit = run_with_io(Cursor::new(payload.into_bytes()), &mut stderr, &mut deps);

    // A fresh line first: libtest has already printed `test <name> ... `.
    println!();
    println!("@@standin exit={exit}");
    println!(
        "@@standin stderr={}",
        debug_str(&String::from_utf8_lossy(&stderr))
    );
    println!("@@standin attempts={}", deps.attempts);
    println!("@@standin complete {}", scenario.name());
}

/// In the child, runs the fixture and gives `None`; in the parent, runs the
/// child and gives what it left. A child named for another scenario is a
/// fault, not a skip.
fn run(scenario: Scenario) -> Option<Ran> {
    match std::env::var(SCENARIO_ENV) {
        Ok(named) => {
            assert_eq!(named, scenario.name(), "a child ran the wrong test");
            child(scenario);
            None
        }
        Err(_) => Some(parent(scenario)),
    }
}

// ── What a session with room and its own pane gets ──────────────────────────

#[test]
fn a_session_with_room_and_its_own_pane_gets_four_send_keys_for_that_pane() {
    let Some(ran) = run(Scenario::Ready) else {
        return;
    };
    assert_eq!(
        ran.record, EXPECTED_RECORD,
        "tmux must run exactly four times, each send-keys for the session's own pane"
    );
    assert!(
        ran.record
            .iter()
            .all(|l| l.starts_with("[send-keys][-t][%5]")),
        "every run targets %5 and nothing else: {:?}",
        ran.record
    );
    assert_eq!(ran.attempts, 4);
    assert_eq!(ran.exit, 0, "keys were sent, so there is no exit-2 wake");
    assert_eq!(
        ran.stderr,
        debug_str(""),
        "the keys wake the session; stderr stays empty"
    );

    let tag = Pane::parse(Some("x"), Some(PANE)).unwrap().log_tag();
    let line = ran
        .log
        .lines()
        .find(|l| l.contains("outcome=resumed-keys"))
        .unwrap_or_else(|| panic!("no resumed-keys line in the log:\n{}", ran.log));
    assert!(line.contains("keys_sent=4"), "{line}");
    assert!(line.contains("swap_live=spare"), "{line}");
    assert!(line.contains(&format!("pane={tag}")), "{line}");
    assert!(
        !ran.log.contains(PANE),
        "the pane id never reaches the log, only its tag:\n{}",
        ran.log
    );
}

// ── What a session with nothing to send keys to gets ────────────────────────

/// The hook records the resume and wakes the session with exit 2 and exactly
/// `RESUME_MSG`; `tmux` is not run.
fn assert_wakes_without_tmux(ran: &Ran) {
    assert_eq!(
        ran.record,
        Vec::<String>::new(),
        "no pane, so tmux must not run at all"
    );
    assert_eq!(ran.attempts, 0, "no key may even be attempted");
    assert_eq!(ran.exit, 2, "the exit-2 wake is what is left");
    assert_eq!(ran.stderr, debug_str(&format!("{RESUME_MSG}\n")));
    let line = ran
        .log
        .lines()
        .find(|l| l.contains("outcome=resumed "))
        .unwrap_or_else(|| panic!("no resumed line in the log:\n{}", ran.log));
    assert!(
        !line.contains("keys_sent=") && !line.contains("pane="),
        "a wake without keys names neither: {line}"
    );
    assert!(
        line.split_whitespace().any(|f| f == "detail=no-tmux-pane"),
        "a wake without a pane says so: {line}"
    );
}

#[test]
fn a_session_with_no_pane_runs_tmux_not_at_all() {
    let Some(ran) = run(Scenario::NoPane) else {
        return;
    };
    assert_wakes_without_tmux(&ran);
}

#[test]
fn an_inherited_pane_without_a_tmux_server_runs_tmux_not_at_all() {
    let Some(ran) = run(Scenario::PaneWithoutTmux) else {
        return;
    };
    assert_wakes_without_tmux(&ran);
}

#[test]
fn a_malformed_pane_id_runs_tmux_not_at_all() {
    let Some(ran) = run(Scenario::MalformedPane) else {
        return;
    };
    assert_wakes_without_tmux(&ran);
}

// ── What a swap onto a full login gets ──────────────────────────────────────

#[test]
fn a_swap_onto_a_full_login_runs_tmux_not_at_all() {
    let Some(ran) = run(Scenario::NoRoom) else {
        return;
    };
    assert_eq!(
        ran.record,
        Vec::<String>::new(),
        "no room, so tmux must not run at all"
    );
    assert_eq!(ran.attempts, 0);
    // The hook does not resume: it logs the swap, waits again and ends on
    // its own deadline, which wakes nobody.
    assert_eq!(ran.exit, 0);
    assert_eq!(ran.stderr, debug_str(""));
    let outcomes: Vec<&str> = ran
        .log
        .lines()
        .filter_map(|l| {
            l.split_whitespace()
                .find_map(|f| f.strip_prefix("outcome="))
        })
        .collect();
    assert_eq!(
        outcomes,
        ["swap-no-room", "no-capacity-timeout"],
        "the full login is logged, then the wait ends:\n{}",
        ran.log
    );
}
