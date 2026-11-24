//! Issue #2695: a standing session held on the usage-limit menu reaches the
//! keys path, and every branch that does not send keys is named.
//!
//! Contract (design: ProcView, tty_ancestor, check_pane_owned, check_pane_held,
//! PaneCheck):
//! - `ProcView::entry(pid) -> Option<ProcEntry { ppid, pgrp, tpgid, uid,
//!   stdin }>` is the seam over `/proc`; production reads `/proc`, tests
//!   inject a map.
//! - `tty_ancestor(view, hook_pid, euid)` walks up from the hook's parent
//!   (at most `MAX_ANCESTRY` steps, stopping at pid 1, never visiting a pid
//!   twice, same uid only) and returns the nearest process whose stdin is a
//!   terminal. Claude Code's own stdin is a pipe, so it is not the holder.
//! - `check_pane_with(view, hook_pid, parent, euid, pane, pane_tty)` is the
//!   pure core of the pane check: `Ready` only when the pane's tty is the
//!   holder's tty, the holder is that tty's foreground group, and Claude
//!   Code (`parent`) is in that same foreground group. Every refusal is a
//!   `PaneCheck::Unverified(reason)` with a reason from
//!   `UNVERIFIED_REASONS`, or `NotReading`.
//! - `check_pane_owned` validates the pane id, walks the ancestry once, asks
//!   tmux only after a holder is known, and hands the holder to
//!   `check_pane_held`; nothing walks twice.
//! - `find_pane_with(view, hook_pid, parent, euid, env_pane, list, tty)`
//!   (#3104): an environment pane takes exactly that path and a refusal
//!   never falls back. With no environment pane, a walk that finds no
//!   terminal is `None` and asks tmux nothing; otherwise one listing, the
//!   single pane on the holder's terminal, then the same proof with the same
//!   refusal words.
//! - `Outcome::{PaneUnverified, PaneNotReading}` are `pane-unverified` and
//!   `pane-not-reading`; `Outcome::wakes()` holds for `NotResumed`, those
//!   two and `ScreenUnrecognised`, and `exit_code()` is 2 exactly when
//!   `wakes()` holds. A wake records nothing.
//!
//! No test talks to a tmux server, reads the real `/proc` tree for a
//! decision, or touches a real home.

use amplihack_hooks::limit_resume::guard::{FsGuard, GuardStore};
use amplihack_hooks::limit_resume::keys::{
    Key, MAX_ANCESTRY, Pane, PaneCheck, ProcEntry, ProcView, RESUME_KEYS, ScreenCheck,
    UNVERIFIED_REASONS, check_pane_with, find_pane_with, parse_stat, tty_ancestor,
};
use amplihack_hooks::limit_resume::log::{LogEntry, append, format_line};
use amplihack_hooks::limit_resume::{Deps, Event, Outcome, RESUME_MSG, run_with, run_with_io};
use amplihack_types::HookInput;
use amplihack_utils::rotator_status::CycleStatus;
use amplihack_utils::rotator_status::swap_record::{
    SwapReading, SwapRecord, read_stop_time, read_swap, rotate_dir,
};
use amplihack_utils::secure_files::write_private_atomic;
use serde_json::json;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const NOW: i64 = 1_792_592_000;
const UID: u32 = 1000;
const TTY: &str = "/dev/pts/7";
const TMUX: Option<&str> = Some("/run/user/1000/tmux-1000/default,4242,0");

// ── An injected process tree ────────────────────────────────────────────────

#[derive(Default)]
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

/// The shape measured on a standing session: the hook (500) is started by
/// Claude Code (400, stdin a pipe), whose parent is the amplihack launcher
/// (300) holding the terminal. All share process group 300, which is the
/// terminal's foreground group.
fn standing_session() -> Tree {
    let mut t = Tree::default();
    t.0.insert(500, proc_entry(400, 300, 300, Some("pipe:[1]")));
    t.0.insert(400, proc_entry(300, 300, 300, Some("pipe:[2]")));
    t.0.insert(300, proc_entry(200, 300, 300, Some(TTY)));
    t.0.insert(200, proc_entry(1, 200, 300, Some(TTY)));
    t.0.insert(1, proc_entry(0, 1, -1, None));
    t
}

fn pane(id: &str) -> Pane {
    Pane::parse(TMUX, Some(id)).unwrap_or_else(|| panic!("{id} is a pane id"))
}

fn tty_line() -> Vec<u8> {
    format!("{TTY}\n").into_bytes()
}

fn check(t: &Tree) -> PaneCheck {
    check_pane_with(t, 500, 400, UID, Some(pane("%3")), &tty_line())
}

fn reason(c: PaneCheck) -> &'static str {
    match c {
        PaneCheck::Unverified(r) => r,
        other => panic!("expected Unverified, got {other:?}"),
    }
}

// ── tty_ancestor ────────────────────────────────────────────────────────────

#[test]
fn the_holder_is_the_nearest_ancestor_with_a_terminal_not_the_parent() {
    assert_eq!(tty_ancestor(&standing_session(), 500, UID), Ok(300));
}

#[test]
fn a_parent_that_holds_the_terminal_is_the_holder() {
    let mut t = standing_session();
    t.0.get_mut(&400).unwrap().stdin = Some(PathBuf::from(TTY));
    assert_eq!(tty_ancestor(&t, 500, UID), Ok(400));
}

#[test]
fn a_chain_without_a_terminal_holder_is_refused() {
    let mut t = standing_session();
    for pid in [300, 200] {
        t.0.get_mut(&pid).unwrap().stdin = Some(PathBuf::from("pipe:[9]"));
    }
    assert_eq!(tty_ancestor(&t, 500, UID), Err("no-tty-ancestor"));
}

#[test]
fn the_walk_stops_at_pid_1_and_never_reports_it() {
    let mut t = standing_session();
    t.0.get_mut(&1).unwrap().stdin = Some(PathBuf::from(TTY));
    for pid in [300, 200] {
        t.0.get_mut(&pid).unwrap().stdin = None;
    }
    assert!(tty_ancestor(&t, 500, UID).is_err(), "init is not an owner");
}

#[test]
fn a_holder_owned_by_another_user_is_refused() {
    let mut t = standing_session();
    t.0.get_mut(&300).unwrap().uid = UID + 1;
    assert!(tty_ancestor(&t, 500, UID).is_err());
}

#[test]
fn an_unreadable_link_in_the_chain_is_refused() {
    let mut t = standing_session();
    t.0.remove(&400);
    assert_eq!(tty_ancestor(&t, 500, UID), Err("unreadable-proc"));
}

#[test]
fn a_cyclic_chain_ends_in_a_refusal() {
    let mut t = Tree::default();
    t.0.insert(500, proc_entry(400, 300, 300, None));
    t.0.insert(400, proc_entry(300, 300, 300, None));
    t.0.insert(300, proc_entry(400, 300, 300, None));
    assert!(tty_ancestor(&t, 500, UID).is_err());
}

#[test]
fn a_chain_longer_than_the_limit_is_refused_even_with_a_holder_beyond_it() {
    let mut t = Tree::default();
    let top = 1000 + MAX_ANCESTRY as u32 + 4;
    for pid in 1000..top {
        t.0.insert(pid, proc_entry(pid + 1, 1, 1, None));
    }
    t.0.insert(top, proc_entry(1, 1, 1, Some(TTY)));
    assert_eq!(tty_ancestor(&t, 1000, UID), Err("walk-limit"));
}

#[test]
fn a_parent_pid_of_zero_ends_the_walk_as_unreadable() {
    let mut t = Tree::default();
    t.0.insert(500, proc_entry(0, 500, 500, None));
    assert!(tty_ancestor(&t, 500, UID).is_err());
}

// ── check_pane_with ─────────────────────────────────────────────────────────

#[test]
fn a_standing_session_whose_claude_code_is_not_the_tty_holder_is_ready() {
    // The measured shape: this is the case the old gate refused.
    assert_eq!(check(&standing_session()), PaneCheck::Ready(pane("%3")));
}

#[test]
fn no_pane_is_none() {
    let t = standing_session();
    assert_eq!(
        check_pane_with(&t, 500, 400, UID, None, &tty_line()),
        PaneCheck::None
    );
}

#[test]
fn a_pane_on_another_tty_is_refused_as_a_mismatch() {
    let t = standing_session();
    let c = check_pane_with(&t, 500, 400, UID, Some(pane("%3")), b"/dev/pts/9\n");
    assert_eq!(reason(c), "tty-mismatch");
}

#[test]
fn a_tty_line_that_is_not_a_device_path_is_refused() {
    let t = standing_session();
    for bad in [&b"../../etc/passwd\n"[..], b"/dev/../etc/x\n", b"\n", b""] {
        let c = check_pane_with(&t, 500, 400, UID, Some(pane("%3")), bad);
        assert!(matches!(c, PaneCheck::Unverified(_)), "{bad:?}");
    }
}

#[test]
fn a_holder_that_is_not_the_foreground_group_is_refused() {
    let mut t = standing_session();
    t.0.get_mut(&300).unwrap().tpgid = 999; // another group has the terminal
    assert_eq!(reason(check(&t)), "not-foreground");
}

#[test]
fn a_suspended_holder_is_refused() {
    let mut t = standing_session();
    t.0.get_mut(&300).unwrap().tpgid = -1;
    assert_eq!(reason(check(&t)), "not-foreground");
}

#[test]
fn claude_code_in_a_background_group_behind_a_wrapper_is_not_reading() {
    // The wrapper holds the terminal in the foreground; Claude Code is in a
    // group of its own that is not the foreground group.
    let mut t = standing_session();
    t.0.get_mut(&400).unwrap().pgrp = 400;
    assert_eq!(check(&t), PaneCheck::NotReading);
}

#[test]
fn an_unreadable_parent_is_never_ready() {
    let mut t = standing_session();
    t.0.remove(&400);
    assert_ne!(check(&t), PaneCheck::Ready(pane("%3")));
}

#[test]
fn no_refusal_is_ever_ready_and_every_reason_is_from_the_closed_set() {
    let mut cases: Vec<PaneCheck> = Vec::new();
    cases.push(check_pane_with(
        &standing_session(),
        500,
        400,
        UID,
        Some(pane("%3")),
        b"/dev/pts/9\n",
    ));
    let mut a = standing_session();
    a.0.get_mut(&300).unwrap().tpgid = 1;
    cases.push(check(&a));
    let mut b = standing_session();
    b.0.get_mut(&300).unwrap().stdin = None;
    b.0.get_mut(&200).unwrap().stdin = None;
    cases.push(check(&b));
    cases.push(check(&Tree::default()));
    for c in cases {
        match c {
            PaneCheck::Unverified(r) => assert!(UNVERIFIED_REASONS.contains(&r), "{r}"),
            PaneCheck::Ready(_) => panic!("a refusal was Ready"),
            _ => {}
        }
    }
    assert_eq!(
        UNVERIFIED_REASONS,
        [
            "no-tty-ancestor",
            "tty-mismatch",
            "not-foreground",
            "walk-limit",
            "unreadable-proc",
            "bad-pane-id"
        ]
    );
}

// ── /proc parsing ───────────────────────────────────────────────────────────

#[test]
fn a_command_name_with_spaces_and_brackets_still_parses() {
    let line = "400 (cl) au (de) S 300 400 300 34823 400 4194560 1 0";
    let s = parse_stat(line).expect("parses after the last ')'");
    assert_eq!((s.pgrp, s.tpgid), (400, 400));
}

#[test]
fn a_truncated_or_non_numeric_stat_is_none() {
    for bad in [
        "",
        "400 (x) S 1",
        "400 (x) S a b c d e",
        "no parens S 1 2 3 4 5",
    ] {
        assert!(parse_stat(bad).is_none(), "{bad:?}");
    }
}

// ── Pane ids ────────────────────────────────────────────────────────────────

#[test]
fn only_a_whole_percent_digits_id_is_a_pane() {
    for bad in ["-t", "%1;x", "%", "%1 ", "%1\n", "--help"] {
        assert!(Pane::parse(TMUX, Some(bad)).is_none(), "{bad:?}");
    }
}

// ── Outcomes ────────────────────────────────────────────────────────────────

#[test]
fn the_new_outcomes_have_their_names() {
    assert_eq!(Outcome::PaneUnverified.as_str(), "pane-unverified");
    assert_eq!(Outcome::PaneNotReading.as_str(), "pane-not-reading");
}

#[test]
fn exit_2_and_the_wake_message_follow_one_predicate() {
    let all = [
        Outcome::NotResumed,
        Outcome::ResumedKeys,
        Outcome::ResumeKeysFailed,
        Outcome::PaneUnverified,
        Outcome::PaneNotReading,
        Outcome::WatchLost,
        Outcome::SwapStale,
        Outcome::SwapNoRoom,
        Outcome::NoCapacityTimeout,
        Outcome::MovedOn,
        Outcome::Duplicate,
        Outcome::SessionRecordReset,
        Outcome::StopHookActive,
        Outcome::SessionGone,
        Outcome::RotatorDown,
        Outcome::GuardFailed,
        Outcome::Disabled,
        Outcome::NotLimit,
        Outcome::ScreenUnrecognised,
    ];
    for o in all {
        assert_eq!(o.exit_code() == 2, o.wakes(), "{o:?}");
    }
    let waking: Vec<_> = all.into_iter().filter(|o| o.wakes()).collect();
    assert_eq!(
        waking,
        [
            Outcome::NotResumed,
            Outcome::PaneUnverified,
            Outcome::PaneNotReading,
            Outcome::ScreenUnrecognised
        ]
    );
}

// ── The hook, end to end over a real swap record ────────────────────────────

/// A temporary profile root with a 0700 mode, created under `$TMPDIR`.
fn root() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    fs::set_permissions(t.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let d = t.path().join(".auto-switch.d");
    fs::create_dir(&d).unwrap();
    fs::set_permissions(&d, fs::Permissions::from_mode(0o700)).unwrap();
    t
}

fn write_swap(root: &Path, at: i64) {
    let dir = rotate_dir(root);
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let r = SwapRecord {
        from_org: None,
        to_org: None,
        affinity: None,
        seq: 1,
        live_login: "spare".into(),
        from_login: "work".into(),
        swapped_at: at,
        reading: SwapReading {
            session: 12,
            week: 40,
            measured_at: at - 30,
        },
    };
    write_private_atomic(
        &dir,
        "swap.json",
        serde_json::to_string(&r).unwrap().as_bytes(),
    )
    .unwrap();
}

struct PaneDeps {
    root: PathBuf,
    guard: FsGuard,
    clock: i64,
    woken: bool,
    check: PaneCheck,
    sent: Vec<Key>,
    fail_on: Option<usize>,
}

impl Deps for PaneDeps {
    fn auto_switch_on(&mut self) -> bool {
        true
    }
    fn now(&mut self) -> i64 {
        self.clock
    }
    fn read_cycle(&mut self, now: i64) -> Option<CycleStatus> {
        amplihack_utils::rotator_status::read_cycle(&self.root, now)
    }
    fn guard(&mut self) -> &mut dyn GuardStore {
        &mut self.guard
    }
    fn log(&mut self, now: i64, entry: &LogEntry) {
        append(
            &self.root.join(".auto-switch.d").join("resume"),
            &format_line(now, entry),
        );
    }
    fn rotator_down(&mut self, _now: i64) -> bool {
        false
    }
    fn arm(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn wait_event(&mut self, deadline: i64) -> Event {
        if self.woken {
            self.clock = deadline;
            return Event::Deadline;
        }
        self.woken = true;
        self.clock = NOW + 61;
        write_swap(&self.root, NOW + 61);
        Event::Swap
    }
    fn read_swap(&mut self, now: i64) -> Option<SwapRecord> {
        read_swap(&self.root, now)
    }
    fn stop_time(&mut self, transcript: Option<&Path>, start: i64) -> Option<i64> {
        transcript.and_then(|p| read_stop_time(p, start))
    }
    fn pane_check(&mut self) -> PaneCheck {
        self.check.clone()
    }
    /// This file covers the ownership proof; the screen is the subject of
    /// `limit_resume_tmux_standin.rs`, so a proved pane is taken to be
    /// showing the menu here.
    fn screen_check(&mut self, _pane: &Pane) -> ScreenCheck {
        ScreenCheck::Held
    }
    fn send_keys(&mut self, _pane: &Pane, key: Key) -> io::Result<()> {
        self.sent.push(key);
        if self.fail_on == Some(self.sent.len()) {
            return Err(io::Error::other("injected"));
        }
        Ok(())
    }
    fn session_alive_now(&mut self) -> bool {
        true
    }
}

fn payload() -> String {
    json!({
        "hook_event_name": "StopFailure",
        "session_id": "s2695",
        "error": "rate_limit",
        "last_assistant_message": "You've hit your session limit"
    })
    .to_string()
}

fn deps(r: &tempfile::TempDir, check: PaneCheck) -> PaneDeps {
    PaneDeps {
        root: r.path().to_path_buf(),
        guard: FsGuard::new(r.path()),
        clock: NOW,
        woken: false,
        check,
        sent: Vec::new(),
        fail_on: None,
    }
}

fn run(d: &mut PaneDeps) -> Outcome {
    let input: HookInput = serde_json::from_str(&payload()).unwrap();
    run_with(&input, d)
}

fn log_of(r: &tempfile::TempDir) -> String {
    let p = r
        .path()
        .join(".auto-switch.d")
        .join("resume")
        .join("resume.log");
    fs::read_to_string(p).unwrap_or_default()
}

#[test]
fn a_ready_pane_gets_escape_clear_continue_enter_in_order() {
    let r = root();
    let mut d = deps(&r, PaneCheck::Ready(pane("%3")));
    assert_eq!(run(&mut d), Outcome::ResumedKeys);
    assert_eq!(d.sent, RESUME_KEYS);
    assert_eq!(
        d.sent,
        [
            Key::Escape,
            Key::ClearLine,
            Key::LiteralContinue,
            Key::Enter
        ]
    );
    assert!(log_of(&r).contains("outcome=resumed-keys"));
}

#[test]
fn an_unverified_pane_is_named_with_its_reason_and_sends_nothing() {
    let r = root();
    let mut d = deps(&r, PaneCheck::Unverified("tty-mismatch"));
    assert_eq!(run(&mut d), Outcome::PaneUnverified);
    assert!(d.sent.is_empty());
    let log = log_of(&r);
    assert!(log.contains("outcome=pane-unverified"), "{log}");
    assert!(log.contains("detail=tty-mismatch"), "{log}");
}

#[test]
fn a_session_not_reading_the_terminal_is_named_and_sends_nothing() {
    let r = root();
    let mut d = deps(&r, PaneCheck::NotReading);
    assert_eq!(run(&mut d), Outcome::PaneNotReading);
    assert!(d.sent.is_empty());
    assert!(log_of(&r).contains("outcome=pane-not-reading"));
}

#[test]
fn no_tmux_pane_takes_the_wake_path_and_is_logged_as_not_resumed() {
    let r = root();
    let mut d = deps(&r, PaneCheck::None);
    assert_eq!(run(&mut d), Outcome::NotResumed);
    assert!(d.sent.is_empty());
    let log = log_of(&r);
    assert!(log.contains("outcome=not-resumed"), "{log}");
    assert!(log.contains("detail=no-tmux-pane"), "{log}");
    assert!(
        !log.contains("outcome=resumed "),
        "never logged as resumed: {log}"
    );
    assert_eq!(Outcome::NotResumed.as_str(), "not-resumed");
}

#[test]
fn a_refused_pane_records_nothing_so_the_next_room_event_can_resume() {
    // #3104: only a resume that reached the session is recorded.
    for check in [
        PaneCheck::None,
        PaneCheck::Unverified("not-foreground"),
        PaneCheck::NotReading,
    ] {
        let r = root();
        let mut d = deps(&r, check.clone());
        run(&mut d);
        let events = r
            .path()
            .join(".auto-switch.d")
            .join("resume")
            .join("events");
        let n = fs::read_dir(events).map_or(0, |d| d.count());
        assert_eq!(n, 0, "{check:?}: a wake leaves the event armed");
    }
    // A pane that was reached still writes exactly one marker.
    let r = root();
    let mut d = deps(&r, PaneCheck::Ready(pane("%3")));
    run(&mut d);
    let events = r
        .path()
        .join(".auto-switch.d")
        .join("resume")
        .join("events");
    assert_eq!(fs::read_dir(events).unwrap().count(), 1);
}

#[test]
fn a_failing_send_is_reported_not_swallowed() {
    let r = root();
    let mut d = deps(&r, PaneCheck::Ready(pane("%3")));
    d.fail_on = Some(2);
    assert_eq!(run(&mut d), Outcome::ResumeKeysFailed);
    assert_eq!(d.sent.len(), 2, "stops at the first failure");
    assert!(log_of(&r).contains("outcome=resume-keys-failed"));
}

#[test]
fn every_wake_outcome_writes_the_message_and_exits_2_and_no_other_does() {
    for (check, wakes) in [
        (PaneCheck::None, true),
        (PaneCheck::Unverified("walk-limit"), true),
        (PaneCheck::NotReading, true),
        (PaneCheck::Ready(pane("%3")), false),
    ] {
        let r = root();
        let mut d = deps(&r, check.clone());
        let mut stderr = Vec::new();
        let code = run_with_io(payload().as_bytes(), &mut stderr, &mut d);
        let text = String::from_utf8(stderr).unwrap();
        assert_eq!(code == 2, wakes, "{check:?}");
        assert_eq!(text.contains(RESUME_MSG), wakes, "{check:?}: {text}");
    }
}

#[test]
fn no_detail_in_the_log_can_carry_free_text() {
    for reason in UNVERIFIED_REASONS {
        let r = root();
        let mut d = deps(&r, PaneCheck::Unverified(reason));
        run(&mut d);
        let log = log_of(&r);
        let detail = log
            .split_whitespace()
            .find_map(|f| f.strip_prefix("detail="))
            .unwrap_or_else(|| panic!("no detail for {reason}: {log}"));
        assert_eq!(detail, reason);
    }
}

// ── Log contents ────────────────────────────────────────────────────────────

fn assert_no_pane_in(log: &str) {
    assert!(!log.contains("%3"), "pane id reached the log: {log}");
    assert!(!log.contains(TTY), "tty path reached the log: {log}");
    assert!(
        !log.contains("/dev/"),
        "a device path reached the log: {log}"
    );
}

#[test]
fn a_log_line_carries_a_hash_not_the_pane_id() {
    let r = root();
    let mut d = deps(&r, PaneCheck::Ready(pane("%3")));
    run(&mut d);
    let log = log_of(&r);
    assert_no_pane_in(&log);
    let tag = pane("%3").log_tag();
    assert!(tag.bytes().all(|b| b.is_ascii_hexdigit()), "{tag}");
    assert!(log.contains(&tag), "tag missing: {log}");
}

#[test]
fn a_refused_pane_puts_nothing_of_the_pane_in_the_log() {
    for c in [
        PaneCheck::Unverified("tty-mismatch"),
        PaneCheck::Unverified("not-foreground"),
        PaneCheck::NotReading,
    ] {
        let r = root();
        let mut d = deps(&r, c);
        run(&mut d);
        assert_no_pane_in(&log_of(&r));
    }
}

// ── Hostile inputs ──────────────────────────────────────────────────────────

#[test]
fn a_hostile_command_name_cannot_shift_the_group_fields() {
    // The name closes itself early and imitates the fields after it.
    let line = "400 (a) S 1 1 1 0) S 300 400 300 34823 999";
    let s = parse_stat(line).expect("parses after the last ')'");
    assert_eq!((s.pgrp, s.tpgid), (400, 999));
}

#[test]
fn a_pane_id_with_a_dash_space_or_nul_is_never_a_pane() {
    for bad in ["-%3", " %3", "%3\u{0}", "%\u{0}3", "%-3", "\u{0}"] {
        assert!(Pane::parse(TMUX, Some(bad)).is_none(), "{bad:?}");
    }
}

#[test]
fn a_walk_stops_at_a_uid_change_even_with_a_holder_beyond_it() {
    let mut t = standing_session();
    t.0.get_mut(&400).unwrap().uid = UID + 1;
    assert!(tty_ancestor(&t, 500, UID).is_err());
}

#[test]
fn a_deleted_tty_link_is_refused_not_stripped_into_a_match() {
    let mut t = standing_session();
    for pid in [300, 200] {
        t.0.get_mut(&pid).unwrap().stdin = Some(PathBuf::from(format!("{TTY} (deleted)")));
    }
    // Either no ancestor counts as a terminal holder or the path differs;
    // in neither case does the suffix get stripped into a match.
    assert!(matches!(check(&t), PaneCheck::Unverified(_)));
}

#[test]
fn a_valid_pane_id_on_another_terminal_never_gets_keys() {
    // $TMUX_PANE is a claim: a hostile value naming another pane fails on tty.
    let t = standing_session();
    let c = check_pane_with(&t, 500, 400, UID, Some(pane("%99")), b"/dev/pts/12\n");
    assert_eq!(reason(c), "tty-mismatch");
}

// ── Finding the pane of a session by its terminal (#3104) ───────────────────

/// Records which tmux questions were asked, in order.
#[derive(Default)]
struct Asked(RefCell<Vec<String>>);

impl Asked {
    fn log(&self) -> Vec<String> {
        self.0.borrow().clone()
    }
}

fn discover(
    t: &Tree,
    env: Option<Pane>,
    listing: io::Result<Vec<u8>>,
    tty_answer: Vec<u8>,
    asked: &Asked,
) -> PaneCheck {
    find_pane_with(
        t,
        500,
        400,
        UID,
        env,
        || {
            asked.0.borrow_mut().push("list".into());
            listing
        },
        |p: &Pane| {
            asked.0.borrow_mut().push(format!("tty({})", p.id()));
            Ok(tty_answer)
        },
    )
}

fn listing_of(id: &str) -> io::Result<Vec<u8>> {
    Ok(format!("%1 /dev/pts/1\n{id} {TTY}\n").into_bytes())
}

#[test]
fn a_pane_found_by_terminal_is_proved_like_an_environment_pane() {
    let asked = Asked::default();
    let got = discover(
        &standing_session(),
        None,
        listing_of("%7"),
        tty_line(),
        &asked,
    );
    match got {
        PaneCheck::Ready(p) => assert_eq!(p.id(), "%7"),
        other => panic!("expected Ready, got {other:?}"),
    }
    assert_eq!(
        asked.log(),
        ["list", "tty(%7)"],
        "one listing, then one proof"
    );
}

#[test]
fn a_found_pane_whose_proof_names_another_tty_is_a_mismatch() {
    let asked = Asked::default();
    let got = discover(
        &standing_session(),
        None,
        listing_of("%7"),
        b"/dev/pts/9\n".to_vec(),
        &asked,
    );
    assert_eq!(reason(got), "tty-mismatch");
}

#[test]
fn a_found_pane_whose_holder_is_not_foreground_is_refused() {
    let mut t = standing_session();
    t.0.insert(300, proc_entry(200, 300, 999, Some(TTY)));
    let asked = Asked::default();
    let got = discover(&t, None, listing_of("%7"), tty_line(), &asked);
    assert_eq!(reason(got), "not-foreground");
}

#[test]
fn a_found_pane_the_session_is_not_reading_is_not_reading() {
    let mut t = standing_session();
    t.0.insert(400, proc_entry(300, 400, 300, Some("pipe:[2]")));
    let asked = Asked::default();
    let got = discover(&t, None, listing_of("%7"), tty_line(), &asked);
    assert!(matches!(got, PaneCheck::NotReading), "{got:?}");
}

#[test]
fn a_refused_environment_pane_never_falls_back_to_discovery() {
    let asked = Asked::default();
    let got = discover(
        &standing_session(),
        Some(pane("%3")),
        listing_of("%7"),
        b"/dev/pts/9\n".to_vec(),
        &asked,
    );
    assert_eq!(reason(got), "tty-mismatch");
    assert_eq!(asked.log(), ["tty(%3)"], "the listing is never asked for");
}

#[test]
fn a_walk_that_finds_no_terminal_gives_none_and_runs_nothing() {
    let mut t = Tree::default();
    t.0.insert(500, proc_entry(400, 300, 300, Some("pipe:[1]")));
    t.0.insert(400, proc_entry(300, 300, 300, Some("pipe:[2]")));
    t.0.insert(300, proc_entry(1, 300, 300, Some("pipe:[3]")));
    t.0.insert(1, proc_entry(0, 1, -1, None));
    let asked = Asked::default();
    let got = discover(&t, None, listing_of("%7"), tty_line(), &asked);
    assert!(matches!(got, PaneCheck::None), "{got:?}");
    assert!(asked.log().is_empty(), "tmux is not run: {:?}", asked.log());
}

#[test]
fn a_walk_that_cannot_read_the_tree_gives_none_and_runs_nothing() {
    let asked = Asked::default();
    let got = discover(&Tree::default(), None, listing_of("%7"), tty_line(), &asked);
    assert!(matches!(got, PaneCheck::None), "{got:?}");
    assert!(asked.log().is_empty());
}

#[test]
fn a_failed_or_unmatched_listing_gives_none_and_asks_nothing_more() {
    for listing in [
        Err(io::Error::other("tmux failed")),
        Ok(b"%1 /dev/pts/1\n".to_vec()),
        Ok(b"garbage\n".to_vec()),
        Ok(format!("%7 {TTY}\n%8 {TTY}\n").into_bytes()),
        Ok(Vec::new()),
    ] {
        let asked = Asked::default();
        let got = discover(&standing_session(), None, listing, tty_line(), &asked);
        assert!(matches!(got, PaneCheck::None), "{got:?}");
        assert_eq!(asked.log(), ["list"], "no proof is asked for");
    }
}

#[test]
fn a_session_not_in_the_window_listing_is_never_typed_into_another_pane() {
    // The only pane listed is on another terminal.
    let asked = Asked::default();
    let got = discover(
        &standing_session(),
        None,
        Ok(b"%1 /dev/pts/1\n%2 /dev/pts/2\n".to_vec()),
        tty_line(),
        &asked,
    );
    assert!(matches!(got, PaneCheck::None), "{got:?}");
}
