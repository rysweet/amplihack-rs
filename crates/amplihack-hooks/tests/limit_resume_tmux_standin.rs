//! Issues #2695 and #3104: the `limit-resume` hook, run from a `StopFailure`
//! payload to an exit code against a stand-in `tmux`, types only into its
//! own session's pane, on a recognised limit screen, and records a resume
//! only when the keys reached the session.
//!
//! Contract (what the deputy's review of PR #2892 asked for, and #3104):
//! - A stand-in `tmux` stands first on `PATH`. The hook starts every tmux
//!   run through `keys.rs`, which is `Command::new("tmux")`, so the stand-in
//!   is reached by the same lookup a real session uses. It records each run
//!   before it prints anything, and prints fixture files for `list-panes`,
//!   `display-message` and `capture-pane`.
//! - The process tree is injected: `StandInDeps::pane_check` calls the
//!   production seam `keys::check_pane_owned_in` with the scenario's tree
//!   ([`session_tree`], or the host layout of #3156), in every scenario.
//!   Nothing else about the pane check is scripted.
//! - With a swap record on disk onto a login with room, `$TMUX` and
//!   `$TMUX_PANE=%5`, and a pane showing the usage-limit menu, the stand-in
//!   is run exactly six times for `%5` and no other target: one
//!   `display-message` for the ownership proof, one `capture-pane -p` to
//!   read the screen, then `send-keys` with `Escape`, `C-u`, `-l continue`
//!   and `Enter`, in that order. The outcome is `resumed-keys` with
//!   `keys_sent=4`; the exit code is 0, nothing is written to stderr, and
//!   the guard holds one event marker and one session line.
//! - With the same record and pane but a screen the hook does not
//!   recognise, or one `capture-pane` prints nothing for, it proves the
//!   pane, reads the screen once and types nothing: the outcome is
//!   `screen-unrecognised` with `detail=screen-unknown` or
//!   `detail=screen-unreadable`, the exit-2 wake carries `UNRECOGNISED_MSG`
//!   before `RESUME_MSG`, and nothing is recorded.
//! - With a swap record onto a login with no room (a window at 100%), the
//!   stand-in is never run. The hook goes on waiting, ends on its deadline
//!   (`no-capacity-timeout`) and exits 0.
//! - With no pane in the environment (`$TMUX_PANE` unset, an inherited
//!   `$TMUX_PANE` with no `$TMUX`, or an id that is not `%` and digits), the
//!   hook lists the panes once and looks for the one on the session's
//!   terminal. With none there, it runs nothing else, exits 2 with exactly
//!   `RESUME_MSG`, logs `not-resumed` with `detail=no-tmux-pane` and records
//!   nothing.
//! - #3104 (a): a session held on the menu whose hook environment lacks
//!   `$TMUX_PANE`, with `$TMUX` set and with it unset, is found by its
//!   terminal and answered in its own pane. (b): a session whose pane cannot
//!   be found is `not-resumed`, and the next room event for the same event
//!   key wakes it again instead of ending as `duplicate`. (c): an
//!   unrecognised screen leaves the session armed, and the next room event,
//!   with the menu showing, resumes it with keys and records it once.
//!
//! How the stand-in comes first on `PATH`. Changing the environment of a
//! running test process is `std::env::set_var`, which is unsafe and races
//! with the other threads of the test binary, so no test does it. Each
//! hook run instead re-runs this same test binary, once, for the one test,
//! with `env_clear()` and a `PATH` that is the stand-in's directory and
//! nothing else. The child takes the fixture branch of the test (see
//! [`run`]); the parent reads what the stand-in recorded. A scenario with a
//! second room event runs a second child against the same sandbox, as a
//! second `StopFailure` delivery would run a second hook.
//!
//! What is injected, and why. The ownership proof reads `/proc` ancestry,
//! tty and foreground group, which `cargo test` cannot supply, so the tree
//! is [`session_tree`]: the hook, the process it runs through with a pipe
//! on stdin, and a launcher holding [`SESSION_TTY`] in the foreground.
//! tmux's answers are the stand-in's fixtures. Everything else is the
//! production path: the swap record is read from disk, the judgement is
//! `judge_at`, the guard is the real `FsGuard`, and the tmux runs go through
//! `keys.rs`.
//!
//! No test here talks to a tmux server, opens a real home or profile root,
//! sleeps, retries or polls. A child is run to completion with `output()`,
//! so no process is left to stop.

#![cfg(unix)]

use amplihack_hooks::limit_resume::guard::{FsGuard, GuardStore};
use amplihack_hooks::limit_resume::keys::{
    self, Key, Pane, PaneCheck, ProcEntry, ProcView, ScreenCheck,
};
use amplihack_hooks::limit_resume::log::{LogEntry, append, format_line};
use amplihack_hooks::limit_resume::{Deps, Event, RESUME_MSG, UNRECOGNISED_MSG, run_with_io};
use amplihack_utils::rotator_status::CycleStatus;
use amplihack_utils::rotator_status::swap_record::{
    SwapReading, SwapRecord, read_swap, rotate_dir,
};
use amplihack_utils::secure_files::write_private_atomic;
use serde_json::json;
use std::collections::HashMap;
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
/// Which hook run of the scenario the child is: 1, or 2 for the next room
/// event.
const RUN_ENV: &str = "AMPLIHACK_LIMIT_RESUME_STANDIN_RUN";
/// First line after the shebang of the stand-in: how the child knows the
/// `tmux` it would run is the stand-in and not a real one.
const MARKER: &str = "# amplihack-limit-resume-standin-tmux";
/// The pane the environment names in the scenarios that name one.
const PANE: &str = "%5";
/// The pane on the session's terminal in the scenarios that list one.
const FOUND_PANE: &str = "%7";
/// A made-up session id and tmux socket, shaped like the real ones.
const SESSION: &str = "s2695";
const SOCKET_AND_SESSION: &str = "tmux.sock,4242,0";

/// The injected process tree: the hook, the process it runs through (its
/// parent, stdin a pipe; not Claude Code) and the launcher that holds the
/// session's terminal in the foreground. The uid is the tree's own; the hook
/// is told it is its euid.
const HOOK_PID: u32 = 5000;
const SESSION_PID: u32 = 4000;
const HOLDER_PID: u32 = 3000;
const UID: u32 = 1000;
/// The session's terminal, as the holder's stdin and as tmux names it.
const SESSION_TTY: &str = "/dev/pts/42";

/// What the stand-in records for `list-panes`, every argument in brackets
/// so spaces cannot blur two arguments into one.
const LIST_PANES: &str = "[list-panes][-a][-F][#{pane_id} #{pane_tty}]";

/// A listing with the session's terminal on [`FOUND_PANE`].
const LISTING_WITH_SESSION: &str = "%1 /dev/pts/1\n%7 /dev/pts/42\n";
/// A listing with no pane on the session's terminal.
const LISTING_WITHOUT_SESSION: &str = "%1 /dev/pts/1\n%2 /dev/pts/2\n";

/// What the stand-in prints for `capture-pane` in the scenarios whose pane
/// is held on the usage-limit menu: the screen the three standing sessions
/// in #2695 were found on.
const MENU_SCREEN: &str = "\
Claude usage limit reached - resets at 3:00pm

What do you want to do?

  1. Buy more usage
  2. Wait for the limit to reset
";

/// What it prints for a pane that is not on any prompt the keys answer.
const OTHER_SCREEN: &str = "\
user@host:~/src$ git status
On branch main
nothing to commit, working tree clean
user@host:~/src$
";

const NOW: i64 = 1_792_592_000;
/// The first run's swap is this far into its wait.
const SWAP_AFTER: i64 = 61;
/// The second run starts this long after the first run's swap (D8).
const NEXT_ROOM_EVENT: i64 = 3600;

/// The screen Claude Code shows in its own timed limit wait (#3156).
const TIMED_WAIT_SCREEN: &str = "\
Usage limit reached · Continuing automatically at 1:50am

";

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
    /// Room, `$TMUX` set, `$TMUX_PANE` unset, no pane on the terminal.
    NoPane,
    /// Room, `$TMUX_PANE=%5`, `$TMUX` unset: an inherited pane is no pane.
    PaneWithoutTmux,
    /// Room, `$TMUX` set, `$TMUX_PANE` not an id.
    MalformedPane,
    /// Room and its own pane, but the pane is showing a shell prompt and
    /// not a screen the keys answer.
    UnknownScreen,
    /// Room and its own pane, but `capture-pane` prints nothing.
    BlankScreen,
    /// #3104 (a): room, `$TMUX` set, `$TMUX_PANE` unset, the menu on the
    /// pane listed on the session's terminal.
    FoundWithTmux,
    /// #3104 (a): as `FoundWithTmux`, with `$TMUX` unset too.
    FoundWithoutTmux,
    /// #3104 (b): no pane named or listed on the terminal, at two room
    /// events.
    NeverFound,
    /// #3104 (c): the found pane shows a shell at the first room event and
    /// the menu at the next.
    UnrecognisedThenMenu,
    /// A listing past the hook's read bound, with the session's pane first.
    OversizedListing,
    /// #3156: the layout found on a live host, in the timed wait: the hook
    /// runs through a wrapper in its own group; Claude Code holds the
    /// terminal in the foreground.
    HostTimedWait,
    /// As `HostTimedWait`, with a background agent under Claude Code.
    HostTimedWaitWithAgent,
    /// As `HostTimedWait`, but Claude Code leaves the foreground after the
    /// second key.
    HostTimedWaitHolderStopped,
}

impl Scenario {
    fn name(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::NoRoom => "no-room",
            Self::NoPane => "no-pane",
            Self::PaneWithoutTmux => "pane-without-tmux",
            Self::MalformedPane => "malformed-pane",
            Self::UnknownScreen => "unknown-screen",
            Self::BlankScreen => "blank-screen",
            Self::FoundWithTmux => "found-with-tmux",
            Self::FoundWithoutTmux => "found-without-tmux",
            Self::NeverFound => "never-found",
            Self::UnrecognisedThenMenu => "unrecognised-then-menu",
            Self::OversizedListing => "oversized-listing",
            Self::HostTimedWait => "host-timed-wait",
            Self::HostTimedWaitWithAgent => "host-timed-wait-with-agent",
            Self::HostTimedWaitHolderStopped => "host-timed-wait-holder-stopped",
        }
    }

    /// How many hook runs, one per room event.
    fn runs(self) -> u32 {
        match self {
            Self::NeverFound | Self::UnrecognisedThenMenu => 2,
            _ => 1,
        }
    }

    /// What the stand-in prints for `capture-pane` in `run`, or nothing at
    /// all.
    fn screen(self, run: u32) -> Option<&'static str> {
        match (self, run) {
            (
                Self::HostTimedWait
                | Self::HostTimedWaitWithAgent
                | Self::HostTimedWaitHolderStopped,
                _,
            ) => Some(TIMED_WAIT_SCREEN),
            (Self::UnknownScreen, _) | (Self::UnrecognisedThenMenu, 1) => Some(OTHER_SCREEN),
            (Self::BlankScreen, _) => None,
            _ => Some(MENU_SCREEN),
        }
    }

    /// What the stand-in prints for `list-panes`.
    fn listing(self) -> String {
        match self {
            Self::FoundWithTmux | Self::FoundWithoutTmux | Self::UnrecognisedThenMenu => {
                LISTING_WITH_SESSION.to_string()
            }
            Self::OversizedListing => {
                let mut l = LISTING_WITH_SESSION.to_string();
                while l.len() <= 64 * 1024 {
                    l.push_str("%1 /dev/pts/1\n");
                }
                l
            }
            _ => LISTING_WITHOUT_SESSION.to_string(),
        }
    }

    /// The `detail` the log carries when the screen is refused.
    fn screen_detail(self) -> &'static str {
        match self {
            Self::BlankScreen => "screen-unreadable",
            _ => "screen-unknown",
        }
    }

    /// The test function that runs this scenario, for `--exact`.
    fn test_fn(self) -> &'static str {
        match self {
            Self::Ready => "a_session_with_room_and_its_own_pane_gets_four_send_keys_for_that_pane",
            Self::NoRoom => "a_swap_onto_a_full_login_runs_tmux_not_at_all",
            Self::NoPane => "a_session_with_no_pane_only_lists_panes_and_types_nothing",
            Self::PaneWithoutTmux => {
                "an_inherited_pane_without_a_tmux_server_only_lists_panes_and_types_nothing"
            }
            Self::MalformedPane => "a_malformed_pane_id_only_lists_panes_and_types_nothing",
            Self::UnknownScreen => "a_screen_the_hook_does_not_recognise_gets_no_keys",
            Self::BlankScreen => "a_screen_that_cannot_be_read_gets_no_keys",
            Self::FoundWithTmux => {
                "a_session_held_on_the_menu_without_tmux_pane_is_answered_in_its_own_pane"
            }
            Self::FoundWithoutTmux => {
                "a_session_held_on_the_menu_without_tmux_or_tmux_pane_is_answered_in_its_own_pane"
            }
            Self::NeverFound => {
                "a_session_whose_pane_cannot_be_found_is_not_resumed_and_is_woken_at_the_next_room_event"
            }
            Self::UnrecognisedThenMenu => {
                "an_unrecognised_screen_leaves_the_session_armed_for_the_next_room_event"
            }
            Self::OversizedListing => "an_oversized_pane_listing_gives_no_pane",
            Self::HostTimedWait => {
                "a_session_in_its_timed_wait_behind_the_hook_wrapper_gets_the_four_keys"
            }
            Self::HostTimedWaitWithAgent => {
                "a_session_in_its_timed_wait_with_a_background_agent_running_gets_the_four_keys"
            }
            Self::HostTimedWaitHolderStopped => {
                "a_holder_leaving_the_foreground_mid_keys_stops_the_rest"
            }
        }
    }

    /// The `$TMUX` the child is given.
    fn tmux(self, root: &Path) -> Option<String> {
        match self {
            Self::PaneWithoutTmux | Self::FoundWithoutTmux | Self::NeverFound => None,
            _ => Some(format!("{}/{SOCKET_AND_SESSION}", root.display())),
        }
    }

    /// The `$TMUX_PANE` the child is given.
    fn tmux_pane(self) -> Option<&'static str> {
        match self {
            Self::NoPane
            | Self::FoundWithTmux
            | Self::FoundWithoutTmux
            | Self::NeverFound
            | Self::UnrecognisedThenMenu
            | Self::OversizedListing => None,
            // If this reached `send-keys -t` unparsed, the record would hold
            // a second target, `%9`.
            Self::MalformedPane => Some("%5 -t %9"),
            _ => Some(PANE),
        }
    }

    /// The process tree the hook is judged against.
    fn tree(self) -> Tree {
        match self {
            Self::HostTimedWait | Self::HostTimedWaitHolderStopped => host_tree(false),
            Self::HostTimedWaitWithAgent => host_tree(true),
            _ => session_tree(),
        }
    }

    /// The session window of the login the rotator swaps to.
    fn swap_session_percent(self) -> u8 {
        if self == Self::NoRoom { 100 } else { 12 }
    }
}

// ── The injected process tree ───────────────────────────────────────────────

struct Tree(HashMap<u32, ProcEntry>);

impl ProcView for Tree {
    fn entry(&self, pid: u32) -> Option<ProcEntry> {
        self.0.get(&pid).cloned()
    }
}

fn proc_entry(ppid: u32, pgrp: i32, tpgid: i32, stdin: Option<&str>) -> ProcEntry {
    ProcEntry {
        ppid,
        pgrp,
        tpgid,
        uid: UID,
        stdin: stdin.map(PathBuf::from),
    }
}

/// A standing session: the hook runs through a process ([`SESSION_PID`], stdin
/// a pipe) whose parent, the launcher, holds [`SESSION_TTY`]. All share the
/// launcher's process group, the terminal's foreground group. `SESSION_PID` is
/// not Claude Code; the holder is [`HOLDER_PID`].
fn session_tree() -> Tree {
    let group = HOLDER_PID as i32;
    let mut t = HashMap::new();
    t.insert(
        HOOK_PID,
        proc_entry(SESSION_PID, group, group, Some("pipe:[1]")),
    );
    t.insert(
        SESSION_PID,
        proc_entry(HOLDER_PID, group, group, Some("pipe:[2]")),
    );
    t.insert(HOLDER_PID, proc_entry(1, group, group, Some(SESSION_TTY)));
    t.insert(1, proc_entry(0, 1, -1, None));
    Tree(t)
}

/// The layout found on a live host (#3156). Claude Code ([`HOST_CLAUDE`]) has
/// the terminal on stdin and is in the launcher's group, which is the
/// foreground group. The process the hook runs through ([`SESSION_PID`]) is in
/// a group of its own with a pipe on stdin and no foreground. A background
/// agent is a child of Claude Code in a session of its own.
const HOST_CLAUDE: u32 = 3500;
const HOST_AGENT: u32 = 3600;

fn host_tree(with_agent: bool) -> Tree {
    let group = HOLDER_PID as i32;
    let mut t = HashMap::new();
    t.insert(
        HOOK_PID,
        proc_entry(SESSION_PID, SESSION_PID as i32, -1, Some("pipe:[1]")),
    );
    t.insert(
        SESSION_PID,
        proc_entry(HOST_CLAUDE, SESSION_PID as i32, -1, Some("pipe:[2]")),
    );
    t.insert(
        HOST_CLAUDE,
        proc_entry(HOLDER_PID, group, group, Some(SESSION_TTY)),
    );
    t.insert(HOLDER_PID, proc_entry(1, group, group, Some(SESSION_TTY)));
    t.insert(1, proc_entry(0, 1, -1, None));
    if with_agent {
        t.insert(
            HOST_AGENT,
            proc_entry(HOST_CLAUDE, HOST_AGENT as i32, -1, Some("/dev/null")),
        );
    }
    Tree(t)
}

// ── The parent: a sandbox, a stand-in, a child run per room event ───────────

/// What one hook run left behind.
struct Run {
    /// The hook's exit code, as the child printed it.
    exit: i32,
    /// What the hook wrote to its stderr, as the child printed it.
    stderr: String,
    /// How many times the hook asked for a key to be sent.
    attempts: u32,
    /// One entry per run of the stand-in during this hook run.
    record: Vec<String>,
    /// The lines this hook run added to `resume.log`.
    log: Vec<String>,
}

/// What a scenario left behind.
struct Ran {
    runs: Vec<Run>,
    /// The whole `resume.log`.
    log: String,
    /// Event markers in the guard after the last run.
    events: usize,
    /// Session lines in the guard after the last run, over every session.
    session_lines: usize,
}

impl Ran {
    /// The one run of a single-run scenario.
    fn only(&self) -> &Run {
        assert_eq!(self.runs.len(), 1, "a single-run scenario");
        &self.runs[0]
    }
}

impl Run {
    /// This run's line for `outcome`.
    fn line(&self, outcome: &str) -> &str {
        let field = format!("outcome={outcome}");
        self.log
            .iter()
            .find(|l| l.split_whitespace().any(|f| f == field))
            .unwrap_or_else(|| panic!("no {outcome} line in this run's log:\n{:?}", self.log))
    }

    /// The `event=` tag of this run's lines; they must all agree.
    fn event_tag(&self) -> String {
        let tags: Vec<&str> = self
            .log
            .iter()
            .filter_map(|l| l.split_whitespace().find(|f| f.starts_with("event=")))
            .collect();
        assert!(!tags.is_empty(), "no log line in this run");
        assert!(tags.iter().all(|t| *t == tags[0]), "{tags:?}");
        tags[0].to_string()
    }
}

fn private_dir(p: &Path) {
    fs::create_dir(p).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
}

/// A sandbox path the stand-in script quotes in single quotes.
fn quotable(p: &Path) -> String {
    let s = p.to_str().unwrap().to_string();
    assert!(
        !s.contains('\'') && !s.contains('\n'),
        "the sandbox path must be quotable: {s}"
    );
    s
}

/// A stand-in `tmux` in `<root>/bin`: a shell script that first appends its
/// arguments, in brackets, to `<root>/record`, then prints `<root>/listing`
/// for `list-panes`, `<root>/tty` for `display-message` and `<root>/screen`
/// for `capture-pane`, and exits 0. It reads neither the environment nor
/// its arguments as anything but text, and a missing fixture makes it print
/// nothing.
fn write_standin(root: &Path) -> PathBuf {
    let bin = root.join("bin");
    private_dir(&bin);
    let record = quotable(&root.join("record"));
    let listing = quotable(&root.join("listing"));
    let tty = quotable(&root.join("tty"));
    let screen = quotable(&root.join("screen"));
    // Only shell built-ins: `PATH` is this directory alone, so the script
    // cannot reach `cat` or any other program.
    let script = format!(
        "#!/bin/sh\n{MARKER}\n{{\n  for a in \"$@\"; do printf '[%s]' \"$a\"; done\n  \
         printf '\\n'\n}} >> '{record}'\ncase \"$1\" in\n  list-panes) f='{listing}' ;;\n  \
         display-message) f='{tty}' ;;\n  capture-pane) f='{screen}' ;;\n  *) f= ;;\nesac\n\
         if [ -n \"$f\" ] && [ -f \"$f\" ]; then\n  \
         while IFS= read -r l; do printf '%s\\n' \"$l\"; done < \"$f\"\nfi\nexit 0\n"
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

/// Files directly under `dir`, or 0 when it does not exist.
fn count_entries(dir: &Path) -> usize {
    fs::read_dir(dir).map_or(0, |d| d.count())
}

/// Lines over every session record under `dir`, or 0 when it does not
/// exist.
fn count_session_lines(dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .map(|e| {
            fs::read_to_string(e.unwrap().path())
                .unwrap()
                .lines()
                .count()
        })
        .sum()
}

fn parent(scenario: Scenario) -> Ran {
    let _turn = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let sandbox = tempfile::tempdir().unwrap();
    fs::set_permissions(sandbox.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let root = sandbox.path();
    let bin = write_standin(root);
    fs::write(root.join("listing"), scenario.listing()).unwrap();
    fs::write(root.join("tty"), format!("{SESSION_TTY}\n")).unwrap();
    let home = root.join("home");
    private_dir(&home);
    let profile = root.join("profile");
    private_dir(&profile);
    private_dir(&profile.join(".auto-switch.d"));

    let mut runs = Vec::new();
    let mut log_lines = 0;
    for run in 1..=scenario.runs() {
        let screen = root.join("screen");
        match scenario.screen(run) {
            Some(text) => fs::write(&screen, text).unwrap(),
            None => {
                if screen.exists() {
                    fs::remove_file(&screen).unwrap();
                }
            }
        }
        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .args(["--exact", scenario.test_fn(), "--nocapture"])
            .env_clear()
            // The stand-in's directory and nothing else: the host's `tmux`,
            // if it has one, cannot be found whatever goes wrong.
            .env("PATH", &bin)
            .env("HOME", &home)
            .env(SCENARIO_ENV, scenario.name())
            .env(ROOT_ENV, root)
            .env(RUN_ENV, run.to_string())
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
        let name = scenario.name();
        assert!(
            out.status.success(),
            "the {name} child (run {run}) failed: {:?}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
            out.status
        );
        // The child must have run its one test to its end. A filter that
        // names no test also exits 0, and would pass every "nothing was
        // sent" check.
        assert!(
            stdout.contains("running 1 test") && stdout.contains("1 passed"),
            "the {name} child did not run exactly its own test:\n{stdout}"
        );
        assert!(
            stdout.contains(&format!("@@standin complete {name} run={run}")),
            "the {name} child did not reach the end of its fixture:\n{stdout}"
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
        // The stand-in creates the record on its first run; it is taken
        // away after each hook run, so each run's record is its own.
        let record_file = root.join("record");
        let record = match fs::read_to_string(&record_file) {
            Ok(text) => {
                fs::remove_file(&record_file).unwrap();
                text.lines().map(str::to_string).collect()
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => panic!("the record cannot be read: {e}"),
        };
        let log = fs::read_to_string(resume_log(&profile)).unwrap_or_default();
        let lines: Vec<String> = log.lines().map(str::to_string).collect();
        let this_run = lines[log_lines..].to_vec();
        log_lines = lines.len();
        runs.push(Run {
            exit: field("exit").parse().unwrap(),
            // The child prints stderr with `{:?}`; so is the expectation.
            stderr: field("stderr"),
            attempts: field("attempts").parse().unwrap(),
            record,
            log: this_run,
        });
    }
    let resume = resume_dir(&profile);
    Ran {
        runs,
        log: fs::read_to_string(resume_log(&profile)).unwrap_or_default(),
        events: count_entries(&resume.join("events")),
        session_lines: count_session_lines(&resume.join("sessions")),
    }
}

fn debug_str(s: &str) -> String {
    format!("{s:?}")
}

// ── The child: the hook, run for real up to the process tree ────────────────

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

/// The hook's environment with the rotator, the clock and the process tree
/// scripted, and everything else the production one: the swap record is
/// read from disk, the guard and log are the real files under the sandbox,
/// and every tmux run goes through `keys.rs` to the stand-in.
struct StandInDeps {
    profile: PathBuf,
    guard: FsGuard,
    clock: i64,
    start: i64,
    /// The `seq` of the swap this run's room event writes.
    seq: u64,
    session_percent: u8,
    /// The pane the child's environment names, parsed as the hook does.
    env_pane: Option<Pane>,
    armed: bool,
    swapped: bool,
    /// The tree the hook is judged against, owned so a scenario can change it.
    tree: Tree,
    scenario: Scenario,
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
        if self.swapped {
            self.clock = deadline;
            return Event::Deadline;
        }
        self.swapped = true;
        self.clock = self.start + SWAP_AFTER;
        write_swap(&self.profile, self.seq, self.clock, self.session_percent);
        Event::Swap
    }
    fn read_swap(&mut self, now: i64) -> Option<SwapRecord> {
        read_swap(&self.profile, now)
    }
    fn stop_time(&mut self, _transcript: Option<&Path>, _start: i64) -> Option<i64> {
        None
    }
    /// The production pane check over the injected tree: the environment's
    /// pane, or else the lookup by terminal, each proved through the
    /// stand-in's `display-message`.
    fn pane_check(&mut self) -> PaneCheck {
        keys::check_pane_owned_in(&self.tree, HOOK_PID, UID, self.env_pane.clone())
    }
    /// The production screen check: `tmux capture-pane -p` against the
    /// stand-in, then the hook's own rule on what it printed.
    fn screen_check(&mut self, pane: &Pane) -> ScreenCheck {
        keys::screen_of(pane)
    }
    fn send_keys(&mut self, pane: &Pane, key: Key) -> io::Result<()> {
        self.attempts += 1;
        let sent = keys::tmux_send(pane, key);
        if self.scenario == Scenario::HostTimedWaitHolderStopped && self.attempts == 2 {
            // Claude Code is suspended: another group has the terminal.
            if let Some(e) = self.tree.0.get_mut(&HOST_CLAUDE) {
                e.tpgid = 9999;
            }
        }
        sent
    }
    /// The production rule over the injected tree: the holder the pane check
    /// proved is still the terminal's foreground group.
    fn session_alive_now(&mut self, holder: u32) -> bool {
        self.tree.entry(holder).is_some_and(|e| e.foreground())
    }
}

/// Stops the child before it can reach anything real: it must be the child
/// of [`parent`], under a private directory, with the stand-in as the only
/// `tmux` it can find. Returns the sandbox and the run number.
fn prove_sandbox(scenario: Scenario) -> (PathBuf, u32) {
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
    let run: u32 = std::env::var(RUN_ENV)
        .ok()
        .and_then(|r| r.parse().ok())
        .unwrap_or_else(|| panic!("{RUN_ENV} must name the run"));
    assert!(
        (1..=scenario.runs()).contains(&run),
        "run {run} of {}",
        scenario.runs()
    );
    (root, run)
}

/// The hook, from a `StopFailure` payload on stdin to an exit code, with the
/// rotator and the process tree scripted as described above. Run 2 is the
/// next room event: the same session and payload, an hour after run 1's
/// swap, and a swap with the next `seq`. Prints what the parent asserts on,
/// one `@@standin` line each.
fn child(scenario: Scenario) {
    let (root, run) = prove_sandbox(scenario);
    let profile = root.join("profile");
    let start = if run == 1 {
        NOW
    } else {
        NOW + SWAP_AFTER + NEXT_ROOM_EVENT
    };
    let var = |k: &str| std::env::var(k).ok();
    let mut deps = StandInDeps {
        profile: profile.clone(),
        guard: FsGuard::new(&profile),
        clock: start,
        start,
        seq: u64::from(run),
        session_percent: scenario.swap_session_percent(),
        env_pane: Pane::parse(var("TMUX").as_deref(), var("TMUX_PANE").as_deref()),
        armed: false,
        swapped: false,
        tree: scenario.tree(),
        scenario,
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
    println!("@@standin complete {} run={run}", scenario.name());
}

/// In the child, runs the fixture and gives `None`; in the parent, runs the
/// children and gives what they left. A child named for another scenario is
/// a fault, not a skip.
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

// ── What the stand-in is expected to record ─────────────────────────────────

/// The proof's one `display-message` for `pane`.
fn display(pane: &str) -> String {
    format!("[display-message][-p][-t][{pane}][#{{pane_tty}}]")
}

/// The one screen read for `pane`.
fn capture(pane: &str) -> String {
    format!("[capture-pane][-p][-t][{pane}]")
}

/// The four keys for `pane`, in order.
fn keys_for(pane: &str) -> Vec<String> {
    vec![
        format!("[send-keys][-t][{pane}][Escape]"),
        format!("[send-keys][-t][{pane}][C-u]"),
        format!("[send-keys][-t][{pane}][-l][continue]"),
        format!("[send-keys][-t][{pane}][Enter]"),
    ]
}

/// A proved pane on the menu: the proof, the screen, then the four keys.
fn proved_and_typed(pane: &str) -> Vec<String> {
    let mut r = vec![display(pane), capture(pane)];
    r.extend(keys_for(pane));
    r
}

/// A pane found by the lookup, proved and typed into.
fn found_and_typed(pane: &str) -> Vec<String> {
    let mut r = vec![LIST_PANES.to_string()];
    r.extend(proved_and_typed(pane));
    r
}

/// Every run that names a target names `pane` and no other.
fn assert_only_target(run: &Run, pane: &str) {
    for l in &run.record {
        if l.contains("[-t]") {
            assert!(
                l.contains(&format!("[-t][{pane}]")),
                "every target is {pane}: {:?}",
                run.record
            );
        }
    }
    for other in ["[%5]", "[%7]", "[%9]"] {
        if other != format!("[{pane}]") {
            assert!(
                !run.record.iter().any(|l| l.contains(other)),
                "{other} must never be a target: {:?}",
                run.record
            );
        }
    }
}

/// A run that sent the four keys to `pane`, logged `resumed-keys` with
/// `keys_sent=4` and the pane's tag, exited 0 and wrote nothing to stderr.
fn assert_resumed_with_keys(run: &Run, pane: &str) {
    assert_only_target(run, pane);
    assert_eq!(run.attempts, 4);
    assert_eq!(run.exit, 0, "keys were sent, so there is no exit-2 wake");
    assert_eq!(
        run.stderr,
        debug_str(""),
        "the keys wake the session; stderr stays empty"
    );
    let tag = Pane::parse(Some("x"), Some(pane)).unwrap().log_tag();
    let line = run.line("resumed-keys");
    assert!(line.contains("keys_sent=4"), "{line}");
    assert!(line.contains("swap_live=spare"), "{line}");
    assert!(line.contains(&format!("pane={tag}")), "{line}");
    for l in &run.log {
        assert!(!l.contains(pane), "the pane id never reaches the log: {l}");
        assert!(!l.contains(SESSION_TTY), "no tty reaches the log: {l}");
    }
}

/// A run that typed nothing, logged `not-resumed` with
/// `detail=no-tmux-pane`, and woke the session with exit 2 and exactly
/// `RESUME_MSG`.
fn assert_not_resumed_without_a_pane(run: &Run) {
    assert_eq!(
        run.record,
        [LIST_PANES],
        "one listing and nothing else: no proof, screen read or key"
    );
    assert_eq!(run.attempts, 0, "no key may even be attempted");
    assert_eq!(run.exit, 2, "the exit-2 wake is what is left");
    assert_eq!(run.stderr, debug_str(&format!("{RESUME_MSG}\n")));
    let line = run.line("not-resumed");
    assert!(
        !line.contains("keys_sent=") && !line.contains("pane="),
        "a wake without keys names neither: {line}"
    );
    assert!(
        line.split_whitespace().any(|f| f == "detail=no-tmux-pane"),
        "a wake without a pane says so: {line}"
    );
    for l in &run.log {
        assert!(
            !l.contains("outcome=resumed "),
            "never logged as resumed: {l}"
        );
        assert!(!l.contains("outcome=duplicate"), "{l}");
    }
}

/// Nothing was recorded: the event stays armed.
fn assert_nothing_recorded(ran: &Ran) {
    assert_eq!(
        (ran.events, ran.session_lines),
        (0, 0),
        "a wake records nothing:\n{}",
        ran.log
    );
}

// ── What a session with room and its own pane gets ──────────────────────────

#[test]
fn a_session_with_room_and_its_own_pane_gets_four_send_keys_for_that_pane() {
    let Some(ran) = run(Scenario::Ready) else {
        return;
    };
    let r = ran.only();
    assert_eq!(
        r.record,
        proved_and_typed(PANE),
        "tmux must run exactly six times: one display-message for the proof, \
         one capture-pane, then the four keys"
    );
    assert!(
        !r.record.iter().any(|l| l.starts_with("[list-panes]")),
        "an environment pane is never looked up: {:?}",
        r.record
    );
    assert_resumed_with_keys(r, PANE);
    assert_eq!((ran.events, ran.session_lines), (1, 1), "{}", ran.log);
}

// ── #3104 (a): a session whose hook environment lacks $TMUX_PANE ────────────

/// The pane on the session's terminal is listed, proved, read, and typed
/// into, and the resume is recorded once.
fn assert_found_and_answered(ran: &Ran) {
    let r = ran.only();
    assert_eq!(
        r.record,
        found_and_typed(FOUND_PANE),
        "one listing, then the proof and the screen read for the found pane, \
         then the four keys to it"
    );
    assert_resumed_with_keys(r, FOUND_PANE);
    assert_eq!(
        (ran.events, ran.session_lines),
        (1, 1),
        "one event marker and one session line:\n{}",
        ran.log
    );
}

#[test]
fn a_session_held_on_the_menu_without_tmux_pane_is_answered_in_its_own_pane() {
    let Some(ran) = run(Scenario::FoundWithTmux) else {
        return;
    };
    assert_found_and_answered(&ran);
}

#[test]
fn a_session_held_on_the_menu_without_tmux_or_tmux_pane_is_answered_in_its_own_pane() {
    let Some(ran) = run(Scenario::FoundWithoutTmux) else {
        return;
    };
    assert_found_and_answered(&ran);
}

// ── What a session with no pane named or found gets ─────────────────────────

#[test]
fn a_session_with_no_pane_only_lists_panes_and_types_nothing() {
    let Some(ran) = run(Scenario::NoPane) else {
        return;
    };
    assert_not_resumed_without_a_pane(ran.only());
    assert_nothing_recorded(&ran);
}

#[test]
fn an_inherited_pane_without_a_tmux_server_only_lists_panes_and_types_nothing() {
    let Some(ran) = run(Scenario::PaneWithoutTmux) else {
        return;
    };
    assert_not_resumed_without_a_pane(ran.only());
    assert_nothing_recorded(&ran);
}

#[test]
fn a_malformed_pane_id_only_lists_panes_and_types_nothing() {
    let Some(ran) = run(Scenario::MalformedPane) else {
        return;
    };
    assert_not_resumed_without_a_pane(ran.only());
    assert_nothing_recorded(&ran);
}

#[test]
fn an_oversized_pane_listing_gives_no_pane() {
    // The session's pane is the first line, so a reader that cut the
    // listing at the bound instead of refusing it would find it.
    let Some(ran) = run(Scenario::OversizedListing) else {
        return;
    };
    assert_not_resumed_without_a_pane(ran.only());
    assert_nothing_recorded(&ran);
}

// ── #3104 (b): a pane that cannot be found, at two room events ──────────────

#[test]
fn a_session_whose_pane_cannot_be_found_is_not_resumed_and_is_woken_at_the_next_room_event() {
    let Some(ran) = run(Scenario::NeverFound) else {
        return;
    };
    assert_eq!(ran.runs.len(), 2);
    for r in &ran.runs {
        assert_not_resumed_without_a_pane(r);
    }
    assert_eq!(
        ran.runs[0].event_tag(),
        ran.runs[1].event_tag(),
        "both runs are for the same event, so the second is not a duplicate \
         only because nothing was recorded:\n{}",
        ran.log
    );
    assert_nothing_recorded(&ran);
}

// ── What a pane the hook cannot recognise gets ──────────────────────────────

/// The pane was proved, so the screen is read once; it is not one the keys
/// answer, so nothing is typed. The wake says so on stderr before the
/// continue message, the log names the reason, and nothing is recorded.
fn assert_reads_then_types_nothing(run: &Run, pane: &str, detail: &str) {
    assert_eq!(run.attempts, 0, "no key may even be attempted");
    assert!(
        !run.record.iter().any(|l| l.starts_with("[send-keys]")),
        "nothing is typed: {:?}",
        run.record
    );
    assert_only_target(run, pane);
    assert_eq!(run.exit, 2, "the exit-2 wake is what is left");
    assert_eq!(
        run.stderr,
        debug_str(&format!("{UNRECOGNISED_MSG}\n{RESUME_MSG}\n")),
        "the wake names the unrecognised screen before the continue message"
    );
    let line = run.line("screen-unrecognised");
    assert!(
        line.split_whitespace()
            .any(|f| f == format!("detail={detail}")),
        "the line names why the screen was refused: {line}"
    );
    assert!(
        !line.contains("keys_sent=") && !line.contains("pane="),
        "a wake without keys names neither: {line}"
    );
    for l in &run.log {
        assert!(
            !l.contains("What do you want to do") && !l.contains("git status"),
            "no screen text reaches the log: {l}"
        );
    }
}

#[test]
fn a_screen_the_hook_does_not_recognise_gets_no_keys() {
    let Some(ran) = run(Scenario::UnknownScreen) else {
        return;
    };
    let r = ran.only();
    assert_eq!(
        r.record,
        [display(PANE), capture(PANE)],
        "the pane is proved, the screen is read once and nothing is typed"
    );
    assert_reads_then_types_nothing(r, PANE, Scenario::UnknownScreen.screen_detail());
    assert_nothing_recorded(&ran);
}

#[test]
fn a_screen_that_cannot_be_read_gets_no_keys() {
    let Some(ran) = run(Scenario::BlankScreen) else {
        return;
    };
    let r = ran.only();
    assert_eq!(
        r.record,
        [display(PANE), capture(PANE)],
        "the pane is proved, the screen is read once and nothing is typed"
    );
    assert_reads_then_types_nothing(r, PANE, Scenario::BlankScreen.screen_detail());
    assert_nothing_recorded(&ran);
}

// ── #3104 (c): an unrecognised screen, then the menu at the next room event ─

#[test]
fn an_unrecognised_screen_leaves_the_session_armed_for_the_next_room_event() {
    let Some(ran) = run(Scenario::UnrecognisedThenMenu) else {
        return;
    };
    assert_eq!(ran.runs.len(), 2);
    let (first, second) = (&ran.runs[0], &ran.runs[1]);
    assert_eq!(
        first.record,
        [
            LIST_PANES.to_string(),
            display(FOUND_PANE),
            capture(FOUND_PANE)
        ],
        "found, proved, read once, and nothing typed"
    );
    assert_reads_then_types_nothing(first, FOUND_PANE, "screen-unknown");

    assert_eq!(
        second.record,
        found_and_typed(FOUND_PANE),
        "the next room event finds the same pane on the menu and types into it"
    );
    assert_resumed_with_keys(second, FOUND_PANE);
    assert!(
        !second.log.iter().any(|l| l.contains("outcome=duplicate")),
        "{:?}",
        second.log
    );
    assert_eq!(
        first.event_tag(),
        second.event_tag(),
        "both runs are for the same event:\n{}",
        ran.log
    );
    assert_eq!(
        (ran.events, ran.session_lines),
        (1, 1),
        "exactly one record, written by the delivered resume:\n{}",
        ran.log
    );
}

// ── What a swap onto a full login gets ──────────────────────────────────────

#[test]
fn a_swap_onto_a_full_login_runs_tmux_not_at_all() {
    let Some(ran) = run(Scenario::NoRoom) else {
        return;
    };
    let r = ran.only();
    assert_eq!(
        r.record,
        Vec::<String>::new(),
        "no room, so tmux must not run at all"
    );
    assert_eq!(r.attempts, 0);
    // The hook does not resume: it logs the swap, waits again and ends on
    // its own deadline, which wakes nobody.
    assert_eq!(r.exit, 0);
    assert_eq!(r.stderr, debug_str(""));
    let outcomes: Vec<&str> = r
        .log
        .iter()
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
    assert_nothing_recorded(&ran);
}

// ── #3156: the layout found on a live host ──────────────────────────────────

#[test]
fn a_session_in_its_timed_wait_behind_the_hook_wrapper_gets_the_four_keys() {
    let Some(ran) = run(Scenario::HostTimedWait) else {
        return;
    };
    let r = ran.only();
    assert_eq!(r.record, proved_and_typed(PANE), "{:?}", r.record);
    assert_resumed_with_keys(r, PANE);
    assert_eq!((ran.events, ran.session_lines), (1, 1), "{}", ran.log);
}

#[test]
fn a_session_in_its_timed_wait_with_a_background_agent_running_gets_the_four_keys() {
    let Some(ran) = run(Scenario::HostTimedWaitWithAgent) else {
        return;
    };
    let r = ran.only();
    assert_eq!(r.record, proved_and_typed(PANE), "{:?}", r.record);
    assert_resumed_with_keys(r, PANE);
    assert_eq!((ran.events, ran.session_lines), (1, 1), "{}", ran.log);
}

#[test]
fn a_holder_leaving_the_foreground_mid_keys_stops_the_rest() {
    let Some(ran) = run(Scenario::HostTimedWaitHolderStopped) else {
        return;
    };
    let r = ran.only();
    let sends = r
        .record
        .iter()
        .filter(|l| l.starts_with("[send-keys]"))
        .count();
    assert_eq!(sends, 2, "no third key once the holder is not foreground");
    let line = r.line("resume-keys-failed");
    assert!(line.contains("keys_sent=2"), "{line}");
    assert!(line.contains("detail=session-gone"), "{line}");
}
