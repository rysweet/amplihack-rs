//! Issue #2820: unreadable processes do not prevent a confirmed swap off a
//! blocked live login. Issue #3301 requires direct target confirmation even
//! when the live login has a block. Wired from `prelude.rs`.
//!
//! The fixture is `tests_issue_2910_fixture`: the real swap, ledger and log
//! under a temporary HOME and profile root (0700), a stubbed probe, a
//! synthetic decision clock, and no `claude`, network, tmux or inherited
//! profile root. The process table is a stand-in `/proc` tree under the
//! test's temporary folder, installed on the test thread with
//! `override_proc_root` and read by the real scan (`ctx.scan =
//! scan_system`): its processes carry the real uid in `status`, an
//! `environ` that is a directory (so it cannot be read, as for a process
//! that is not dumpable), and pids above `pid_max`, so none is this test's
//! own process. Nothing reads the real `/proc`, `~/.claude` or any login.
//!
//! Every case runs at the deployed `--session-at 95 --week-at 98` and at
//! 85/90 (`BOTH`). Whether a waiting run wakes is read as the usage-limit
//! wait reads it, through `rotator_status::parse_cycle`, with the helpers of
//! `tests_issue_2910_deployed.rs` copied below. A login-change wait (a spend
//! limit with no reset) ends on the swap itself; that is pinned by
//! `limit_wait::tests_wait_ends::a_login_change_wait_still_ends_when_the_login_changes`.

use crate::commands::agent::procfs::override_proc_root;
use crate::commands::profile::auto_switch::kit::{now, scan_system};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::rotate::cycle::{Ctx, CycleOutcome, Mode, run_cycle};
use crate::commands::profile::rotate::decide::target::LiveRoom;
use crate::commands::profile::rotate::decide::target_confirm::{
    Facts, Limit, Step, Unconfirmed, after_unknown, next,
};
use crate::commands::profile::rotate::decide::target_msg::unconfirmed;
use crate::commands::profile::rotate::decide::target_seen::Extra;
use crate::commands::profile::rotate::decide::target_seen::tests_issue_2910_fixture::{
    BOTH, Fleet, clock, expired, fleet, name, run, run_full, run_tuned, set_clock, stub, unread,
};
use crate::commands::profile::rotate::decide::{Decision, Probed};
use crate::commands::profile::rotate::fresh::{self, MAX_REREADS_PER_CYCLE, Reread};
use crate::commands::profile::rotate::ledger::SPEND_LIMIT;
use crate::commands::profile::rotate::log::{Sink, msg, open_or_warn};
use crate::commands::profile::rotate::measure::ProbeLabel;
use crate::commands::profile::rotate::observe::{decide_with, snapshot};
use crate::commands::profile::rotate::tests_issue_1870::{exh, rd};
use crate::commands::profile::rotate::tests_revalidate_cause::ctx_with;
use amplihack_utils::rotator_status::{CycleStatus, parse_cycle};
use std::cell::Cell;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

/// Four hours: the spend block of 23 November (11:03Z to 15:20Z).
const BLOCK: i64 = 4 * 3600;

// --- The stand-in process table -------------------------------------------------------

/// A stand-in `/proc` under the test's temporary folder, 0700.
struct Table {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

/// The first pid of the stand-in processes: above the largest `pid_max`
/// (2^22), so it is never this test's own process.
const PID: u32 = 4_200_001;

impl Table {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("proc");
        fs::create_dir_all(root.join("self")).unwrap();
        fs::write(root.join("self/environ"), b"HOME=/stand-in\0").unwrap();
        Self { _tmp: tmp, root }
    }

    fn dir(&self, pid: u32, name: &str, argv: &[String]) -> PathBuf {
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        let dir = self.root.join(pid.to_string());
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("status"),
            format!("Name:\t{name}\nState:\tS (sleeping)\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n"),
        )
        .unwrap();
        let mut raw = Vec::new();
        for a in argv {
            raw.extend_from_slice(a.as_bytes());
            raw.push(0);
        }
        fs::write(dir.join("cmdline"), raw).unwrap();
        dir
    }

    /// A process whose environment cannot be read, as a process that is not
    /// dumpable leaves it: `environ` is a directory.
    fn unreadable(&self, pid: u32, name: &str, argv: &[String]) {
        let dir = self.dir(pid, name, argv);
        fs::create_dir(dir.join("environ")).unwrap();
    }

    /// A process whose environment can be read.
    fn readable(&self, pid: u32, name: &str, argv: &[String], env: &[String]) {
        let dir = self.dir(pid, name, argv);
        let mut raw = Vec::new();
        for e in env {
            raw.extend_from_slice(e.as_bytes());
            raw.push(0);
        }
        fs::write(dir.join("environ"), raw).unwrap();
    }
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_string()).collect()
}

/// The 19 and 23 November processes: a Tailscale SSH child whose remote
/// command mentions amplihack, an unreadable `claude` naming `busy`, and an
/// unreadable amplihack executable naming no profile.
fn nov23_table(f: &Fleet) -> Table {
    let t = Table::new();
    t.unreadable(
        PID,
        "tailscaled",
        &args(&[
            "tailscaled",
            "be-child",
            "ssh",
            "--remote-ip=100.64.0.7",
            "amplihack recipe run wait",
        ]),
    );
    t.unreadable(
        PID + 1,
        "claude",
        &[
            "claude".to_string(),
            format!("CLAUDE_CONFIG_DIR={}", f.e.root.join("busy").display()),
            "TOKEN=sentinel-argv-secret".to_string(),
        ],
    );
    t.unreadable(PID + 2, "amplihack", &args(&["amplihack", "recipe", "run"]));
    t
}

/// The cycle reads sessions through the real scan of the stand-in table.
fn real_scan(ctx: &mut Ctx) {
    ctx.scan = scan_system;
}

fn block(f: &Fleet, who: &str, until: i64) {
    f.block(who, SPEND_LIMIT, until);
}

/// The live login's spend block on 23 November: until four hours from now.
fn block_live(f: &Fleet) -> i64 {
    let until = clock() + BLOCK;
    block(f, "work", until);
    until
}

fn count(log: &str, needle: &str) -> usize {
    log.lines().filter(|l| l.contains(needle)).count()
}

// --- AC2 and AC4: one unreadable process leaves the other profiles readable ------------

/// The 23 November cycle with the stand-in processes: before #2820 every
/// login was `not read (session running)` and the rotator stayed on the
/// spend-blocked live login. Now only `busy`, the profile the unreadable
/// `claude` names, is skipped; the others are read, and the rotator moves to
/// the best of them on the first cycle.
#[test]
fn ac2_one_unreadable_process_leaves_the_other_profiles_readable_and_the_rotator_moves() {
    for (label, s) in BOTH {
        let f = fleet(&["busy", "free1", "free2"], &[]);
        block_live(&f);
        let table = nov23_table(&f);
        let _proc = override_proc_root(&table.root);
        let out = run_tuned(
            &f,
            s(),
            rd(94.0, 75.0, None),
            &[
                ("busy", rd(5.0, 5.0, None)),
                ("free1", rd(30.0, 40.0, None)),
                ("free2", rd(50.0, 50.0, None)),
            ],
            &[],
            Mode::Watch,
            &real_scan,
        );
        let log = f.log();
        assert_eq!(out, CycleOutcome::Swapped, "{label}: {log}");
        assert!(f.live_is("free1"), "{label}: {log}");
        assert!(!log.contains(msg::NO_LIVENESS), "{label}: {log}");
        assert!(
            !log.contains("not read (session running)"),
            "{label}: {log}"
        );
        assert_eq!(
            count(&log, "skip: busy (session running)"),
            1,
            "{label}: {log}"
        );
        assert!(!log.contains("skip: free1"), "{label}: {log}");
        assert!(!log.contains("skip: free2"), "{label}: {log}");
        assert!(!log.contains("unconfirmed"), "{label}: read fresh: {log}");
    }
}

/// AC4: each unreadable process is named once, from the cycle's first
/// snapshot only (revalidation of the swap does not name it again), with
/// its pid, executable, command line and verdict. An environment
/// assignment in the arguments is masked, and the environment of a process
/// is never read or logged, readable or not.
#[test]
fn ac4_each_unreadable_process_is_named_once_and_no_environment_reaches_the_log() {
    for (label, s) in BOTH {
        let f = fleet(&["busy", "free1", "free2"], &[]);
        block_live(&f);
        let table = nov23_table(&f);
        table.readable(
            PID + 3,
            "claude",
            &args(&["claude"]),
            &[
                "SECRET=sentinel-environ-secret".to_string(),
                format!("CLAUDE_CONFIG_DIR={}", f.e.root.join("free2").display()),
            ],
        );
        let _proc = override_proc_root(&table.root);
        let out = run_tuned(
            &f,
            s(),
            rd(94.0, 75.0, None),
            &[
                ("free1", rd(30.0, 40.0, None)),
                ("free2", rd(5.0, 5.0, None)),
            ],
            &[],
            Mode::Watch,
            &real_scan,
        );
        let log = f.log();
        assert_eq!(out, CycleOutcome::Swapped, "{label}: {log}");
        assert!(
            f.live_is("free1"),
            "{label}: free2 is live by its environment: {log}"
        );
        let tailscaled = format!(
            "unreadable process {PID} (tailscaled) \"tailscaled be-child ssh --remote-ip=100.64.0.7 amplihack recipe run wait\": not counted (not a Claude executable)"
        );
        let busy = format!(
            "unreadable process {} (claude) \"claude CLAUDE_CONFIG_DIR=<hidden> TOKEN=<hidden>\": counted as a session on busy",
            PID + 1
        );
        let unknown = format!(
            "unreadable process {} (amplihack) \"amplihack recipe run\": not counted (profile cannot be told)",
            PID + 2
        );
        for want in [&tailscaled, &busy, &unknown] {
            assert_eq!(count(&log, want), 1, "{label}: once: {want}\n{log}");
        }
        assert_eq!(count(&log, "unreadable process"), 3, "{label}: {log}");
        for secret in [
            "sentinel-argv-secret",
            "sentinel-environ-secret",
            &f.e.root.join("busy").display().to_string(),
        ] {
            assert!(!log.contains(secret), "{label}: {secret} in\n{log}");
        }
    }
}

/// A long-lived unreadable process is named on the first cycle only: a
/// second cycle of the same rotator that sees the same set repeats nothing.
#[test]
fn a_long_lived_unreadable_process_is_named_on_the_first_cycle_only() {
    let f = fleet(&["busy", "free1"], &[]);
    let table = nov23_table(&f);
    let _proc = override_proc_root(&table.root);
    // The live login has room: both cycles stay. The first runs inside the
    // tuning, on the same context as the second.
    let tune = |ctx: &mut Ctx| {
        ctx.scan = scan_system;
        let first = run_cycle(ctx, Mode::Watch).expect("the first cycle");
        assert_eq!(first, CycleOutcome::Stayed);
    };
    let table_rows = [("free1", rd(30.0, 40.0, None))];
    let out = run_tuned(
        &f,
        BOTH[0].1(),
        rd(20.0, 20.0, None),
        &table_rows,
        &[],
        Mode::Watch,
        &tune,
    );
    let log = f.log();
    assert_eq!(out, CycleOutcome::Stayed, "{log}");
    assert_eq!(count(&log, "unreadable process"), 3, "{log}");
    assert_eq!(count(&log, " cycle live=work"), 2, "two cycles ran: {log}");
}

#[test]
fn note_unreadable_logs_only_the_lines_the_last_cycle_did_not_give() {
    let f = fleet(&[], &[]);
    let mut ctx = ctx_with(&f.e, stub);
    let mut out = Sink::new(open_or_warn(&f.e.rs.log, &f.e.rs.log1), false, true);
    for lines in [vec!["a"], vec!["a", "b"], vec!["b"], vec![], vec!["a", "b"]] {
        let lines = lines.into_iter().map(str::to_string).collect();
        ctx.note_unreadable(&mut out, now(), lines);
    }
    let logged: Vec<String> = f
        .log()
        .lines()
        .map(|l| l.split_once(' ').unwrap().1.to_string())
        .collect();
    assert_eq!(logged, ["a", "b", "a", "b"]);
    assert_eq!(ctx.unreadable_notes, ["a", "b"]);
}

// --- AC5: a blocked live login moves, on the first cycle -------------------------------

fn assert_moved(label: &str, f: &Fleet, out: CycleOutcome, to: &str) -> String {
    let log = f.log();
    assert_eq!(out, CycleOutcome::Swapped, "{label}: {log}");
    assert!(f.live_is(to), "{label}: {log}");
    assert!(
        !log.contains("stay: no candidate confirmed by a fresh reading"),
        "{label}: {log}"
    );
    log
}

/// Last good readings rank candidates, but a failed confirmation leaves
/// the live login unchanged even when it has a spend block.
#[test]
fn ac5_i_a_blocked_live_login_stays_when_confirmation_of_the_last_good_target_fails() {
    for (label, s) in BOTH {
        let f = fleet(&["aa", "bb"], &[]);
        block_live(&f);
        f.history("aa", 1800, 30, 40);
        f.history("bb", 1800, 50, 50);
        let out = run(
            &f,
            s(),
            rd(94.0, 75.0, None),
            &[("aa", unread()), ("bb", unread())],
            &[("aa", ProbeLabel::Http429), ("bb", ProbeLabel::Http429)],
        );
        assert_ne!(out, CycleOutcome::Swapped, "{label}: {}", f.log());
        assert!(f.live_is("work"), "{label}: {}", f.log());
        assert_waits_held(label, &wait_view(&f), limit_hits());
    }
}

/// AC5 (ii), stay route 2: the blocked live login's own probe failed. Before
/// #2820 the failed-probe damping held the move (`stay: live probe failed
/// (1/3)`) although the recorded block is evidence of no room. Now it moves
/// on the first cycle, onto a login read fresh.
#[test]
fn ac5_ii_a_blocked_live_login_whose_probe_failed_moves_at_once() {
    for (label, s) in BOTH {
        let f = fleet(&["aa", "bb"], &[]);
        block_live(&f);
        let out = run(
            &f,
            s(),
            unread(),
            &[("aa", rd(30.0, 40.0, None)), ("bb", rd(50.0, 50.0, None))],
            &[],
        );
        let log = assert_moved(label, &f, out, "aa");
        assert!(!log.contains("stay: live probe failed"), "{label}: {log}");
        assert!(!log.contains("unconfirmed"), "{label}: read fresh: {log}");
    }
}

thread_local! {
    static TARGET_READS: Cell<usize> = const { Cell::new(0) };
}

/// Confirmation fails for a reason about the login.
fn reread_fails(_: &Ctx, _: &ProfileName, _: Duration) -> (Probed, Option<ProbeLabel>) {
    TARGET_READS.with(|reads| reads.set(reads.get() + 1));
    (expired(), None)
}

/// Three candidates read 15 minutes ago, which rank first, and `d`, read 20
/// minutes ago: all stale against the live reading.
fn stale_four() -> [(&'static str, Probed); 4] {
    [
        ("a1", rd(10.0, 10.0, Some(clock() - 900))),
        ("a2", rd(15.0, 15.0, Some(clock() - 900))),
        ("a3", rd(20.0, 20.0, Some(clock() - 900))),
        ("d", rd(30.0, 30.0, Some(clock() - 1200))),
    ]
}

type SettingsFn = fn() -> crate::commands::profile::rotate::settings::Settings;

fn route3(f: &Fleet, s: SettingsFn, live: Probed) -> CycleOutcome {
    route3_on(f, s, live, &stale_four())
}

fn route3_on(f: &Fleet, s: SettingsFn, live: Probed, table: &[(&str, Probed)]) -> CycleOutcome {
    TARGET_READS.with(|reads| reads.set(0));
    run_full(
        f,
        s(),
        live,
        table,
        &[],
        Mode::Watch,
        Some(reread_fails),
        stub,
    )
}

/// A failed confirmation does not start another target's probe or permit
/// a swap, even when the live login has a spend block.
#[test]
fn ac5_iii_a_blocked_live_login_stays_after_one_failed_target_probe() {
    for (label, s) in BOTH {
        let f = fleet(&["a1", "a2", "a3", "d"], &[]);
        block_live(&f);
        let out = route3(&f, s, rd(94.0, 75.0, None));
        assert_ne!(out, CycleOutcome::Swapped, "{label}: {}", f.log());
        assert!(f.live_is("work"), "{label}: {}", f.log());
        assert_eq!(TARGET_READS.with(Cell::get), 1, "{label}");
        assert_waits_held(label, &wait_view(&f), limit_hits());
    }
}

// --- AC6: never a blocked target ------------------------------------------------------

/// AC6: the best last good reading belongs to a login that is itself
/// spend-blocked. It is never chosen; the move goes to the next login.
#[test]
fn ac6_a_blocked_candidate_is_never_chosen_on_its_last_good_reading() {
    fn confirm_bb(_: &Ctx, who: &ProfileName, _: Duration) -> (Probed, Option<ProbeLabel>) {
        assert_eq!(who, &name("bb"));
        (rd(50.0, 50.0, None), None)
    }
    for (label, s) in BOTH {
        let f = fleet(&["aa", "bb"], &[]);
        block_live(&f);
        block(&f, "aa", clock() + BLOCK);
        f.history("aa", 1800, 5, 5);
        f.history("bb", 1800, 50, 50);
        let out = run_full(
            &f,
            s(),
            rd(94.0, 75.0, None),
            &[("aa", unread()), ("bb", unread())],
            &[("aa", ProbeLabel::Http429), ("bb", ProbeLabel::Http429)],
            Mode::Watch,
            Some(confirm_bb),
            stub,
        );
        assert_moved(label, &f, out, "bb");
    }
}

/// A blocked candidate remains ineligible, and a failed confirmation of
/// another candidate cannot permit an unconfirmed swap.
#[test]
fn ac6_a_blocked_candidate_is_not_chosen_after_another_confirmation_fails() {
    for (label, s) in BOTH {
        let f = fleet(&["blocked", "a1", "a2", "a3", "d"], &[]);
        block_live(&f);
        block(&f, "blocked", clock() + BLOCK);
        let mut table = vec![("blocked", rd(1.0, 1.0, Some(clock() - 900)))];
        table.extend(stale_four());
        let out = route3_on(&f, s, rd(94.0, 75.0, None), &table);
        assert_ne!(out, CycleOutcome::Swapped, "{label}: {}", f.log());
        assert!(f.live_is("work"), "{label}: {}", f.log());
        assert_eq!(TARGET_READS.with(Cell::get), 1, "{label}");
    }
}

// --- AC7: no change when the live login is not blocked --------------------------------

/// AC7, stay route 2: with no block, a failed live probe is still damped.
#[test]
fn ac7_without_a_block_a_failed_live_probe_is_still_damped() {
    for (label, s) in BOTH {
        let f = fleet(&["aa", "bb"], &[]);
        let out = run(
            &f,
            s(),
            unread(),
            &[("aa", rd(30.0, 40.0, None)), ("bb", rd(50.0, 50.0, None))],
            &[],
        );
        let log = f.log();
        assert_ne!(out, CycleOutcome::Swapped, "{label}: {log}");
        assert!(f.live_is("work"), "{label}: {log}");
        assert!(
            log.contains("stay: live probe failed (1/3)"),
            "{label}: {log}"
        );
    }
}

/// The same failed confirmation rule applies without a live spend block.
#[test]
fn ac7_with_room_on_the_live_login_one_failed_confirmation_leaves_it_unchanged() {
    for (label, s) in BOTH {
        let f = fleet(&["a1", "a2", "a3", "d"], &[]);
        let out = route3(&f, s, rd(96.0, 40.0, None));
        let log = f.log();
        assert_ne!(out, CycleOutcome::Swapped, "{label}: {log}");
        assert!(f.live_is("work"), "{label}: {log}");
        assert_eq!(TARGET_READS.with(Cell::get), 1, "{label}");
    }
}

/// A full live login also stays when the chosen target's confirmation
/// fails. Its last good reading cannot establish current capacity.
#[test]
fn ac7_a_full_live_login_with_no_block_does_not_move_on_last_good_readings() {
    for (label, s) in BOTH {
        let f = fleet(&["aa", "bb"], &[]);
        f.history("aa", 1800, 30, 40);
        f.history("bb", 1800, 50, 50);
        let out = run(
            &f,
            s(),
            exh(100.0, 75.0),
            &[("aa", unread()), ("bb", unread())],
            &[("aa", ProbeLabel::Http429), ("bb", ProbeLabel::Http429)],
        );
        let log = f.log();
        assert_ne!(out, CycleOutcome::Swapped, "{label}: {log}");
        assert!(f.live_is("work"), "{label}: {log}");
        assert_waits_held(label, &wait_view(&f), limit_hits());
    }
}

// --- AC8: a waiting step wakes on the move --------------------------------------------
// The four helpers below are copied from `tests_issue_2910_deployed.rs`,
// which keeps them private.

/// The ledger as the usage-limit wait reads it.
fn wait_view(f: &Fleet) -> CycleStatus {
    let raw = fs::read(&f.e.rs.ledger).expect("the cycle wrote a ledger");
    parse_cycle(&raw, now() + 1).expect("the wait parses the ledger")
}

/// When the waiting runs met their limits: three hours, ten minutes and one
/// second before the decision clock.
fn limit_hits() -> [i64; 3] {
    [clock() - 3 * 3600, clock() - 600, clock() - 1]
}

/// Every run waiting since one of `hits` wakes: on the rotator's capacity,
/// and on the login change the cycle made.
fn assert_waits_wake(what: &str, v: &CycleStatus, hits: [i64; 3]) {
    for start in hits {
        assert!(
            v.fresh_room(start),
            "{what}: a run waiting since {start}: {v:?}"
        );
    }
    assert!(v.has_capacity() && v.says_room(), "{what}: {v:?}");
}

/// No run waiting since one of `hits` wakes, on either ground.
fn assert_waits_held(what: &str, v: &CycleStatus, hits: [i64; 3]) {
    for start in hits {
        assert!(
            !v.fresh_room(start),
            "{what}: a run waiting since {start}: {v:?}"
        );
    }
    assert!(!(v.has_capacity() && v.says_room()), "{what}: {v:?}");
}

/// AC8 T3a: a move off the blocked live login confirmed by a fresh reading
/// below `--session-at` records room in its own cycle record, so every
/// waiting run wakes on it.
#[test]
fn t3a_a_confirmed_move_off_a_blocked_live_login_wakes_the_waits_on_its_own_record() {
    for (label, s) in BOTH {
        let f = fleet(&["aa", "bb"], &[]);
        block_live(&f);
        let hits = limit_hits();
        let out = run(
            &f,
            s(),
            unread(),
            &[("aa", rd(30.0, 40.0, None)), ("bb", rd(50.0, 50.0, None))],
            &[],
        );
        assert_moved(label, &f, out, "aa");
        let v = wait_view(&f);
        assert_eq!(v.live_room, Some(true), "{label}: {v:?}");
        assert_waits_wake(label, &v, hits);
    }
}

/// Waiting work stays pending after failed confirmation and wakes when
/// a subsequent cycle confirms and swaps to a login with room.
#[test]
fn t3b_failed_confirmation_keeps_waits_pending_until_a_later_confirmed_swap() {
    for (label, s) in BOTH {
        let f = fleet(&["a1", "a2", "a3", "d"], &[]);
        set_clock(now() - 600);
        let until = clock() + BLOCK;
        block(&f, "work", until);
        let hits = limit_hits();
        let out = route3(&f, s, rd(94.0, 75.0, None));
        assert_eq!(out, CycleOutcome::StayedUnread, "{label}: {}", f.log());
        assert!(f.live_is("work"), "{label}: {}", f.log());
        assert_eq!(TARGET_READS.with(Cell::get), 1, "{label}");
        let v = wait_view(&f);
        assert_waits_held(label, &v, hits);

        set_clock(now());
        let table = [
            ("d", rd(30.0, 30.0, None)),
            ("a1", rd(10.0, 10.0, None)),
            ("a2", rd(15.0, 15.0, None)),
            ("a3", rd(20.0, 20.0, None)),
        ];
        let out = run(&f, s(), rd(94.0, 75.0, None), &table, &[]);
        let log = f.log();
        assert_eq!(out, CycleOutcome::Swapped, "{label}: {log}");
        assert!(f.live_is("a1"), "{label}: {log}");
        let v = wait_view(&f);
        assert_eq!(v.live_room, Some(true), "{label}: {v:?}");
        assert_waits_wake(&format!("{label}, after the next cycle"), &v, hits);
    }
}

// --- The pure parts of the stay-or-leave decision -------------------------------------

fn facts() -> Facts {
    Facts {
        failed_now: false,
        attempts: 0,
        can_read: true,
        live_blocked: false,
        label: None,
        recent: false,
    }
}

#[test]
fn a_recent_reading_is_accepted_at_a_limit_only_when_the_live_login_is_blocked() {
    let capped = Facts {
        attempts: MAX_REREADS_PER_CYCLE,
        ..facts()
    };
    let timed_out = Facts {
        can_read: false,
        ..facts()
    };
    for (f, limit) in [(capped, Limit::Count), (timed_out, Limit::Time)] {
        let blocked = Facts {
            live_blocked: true,
            ..f
        };
        assert_eq!(
            next(&Facts {
                recent: true,
                ..blocked
            }),
            Step::AcceptStale(limit)
        );
        assert_eq!(next(&blocked), Step::NotTried(limit), "an old reading");
        assert_eq!(
            next(&Facts { recent: true, ..f }),
            Step::NotTried(limit),
            "the live login has no block"
        );
    }
    // A failed probe with a label is taken on its recent last good reading
    // first, before the cap: that reads nothing.
    let failed = Facts {
        failed_now: true,
        live_blocked: true,
        recent: true,
        label: Some(ProbeLabel::Http429),
        attempts: MAX_REREADS_PER_CYCLE,
        ..facts()
    };
    assert_eq!(next(&failed), Step::AcceptUnconfirmed(ProbeLabel::Http429));
    assert_eq!(
        next(&Facts {
            live_blocked: false,
            ..failed
        }),
        Step::NotTried(Limit::Count),
        "issue #3105: a full live login with no block never takes it"
    );
    assert_eq!(
        after_unknown(ProbeLabel::Transport, &failed),
        Step::AcceptUnconfirmed(ProbeLabel::Transport)
    );
    // A target that can be read is read, recent or not.
    let readable = Facts {
        live_blocked: true,
        recent: true,
        ..facts()
    };
    assert_eq!(next(&readable), Step::ReadAgain);
}

#[test]
fn the_unconfirmed_suffix_names_the_label_or_the_limit() {
    assert_eq!(unconfirmed(ProbeLabel::Http429), " unconfirmed (HTTP 429)");
    let count = Unconfirmed::NotTried {
        limit: Limit::Count,
        age: 60,
    };
    assert_eq!(
        unconfirmed(count),
        format!(" unconfirmed (re-read limit {MAX_REREADS_PER_CYCLE})")
    );
    let time = Unconfirmed::NotTried {
        limit: Limit::Time,
        age: 60,
    };
    assert_eq!(unconfirmed(time), " unconfirmed (time limit)");
    assert_eq!(count.label(), None);
    assert_eq!(
        Unconfirmed::from(ProbeLabel::Transport).label(),
        Some(ProbeLabel::Transport)
    );
}

#[test]
fn taken_at_and_recent_date_the_reading_the_decision_used() {
    use crate::commands::profile::rotate::decide::last_good::LastGood;
    let t = 1_000_000;
    let lg = LastGood {
        reading: rd(10.0, 10.0, None).reading,
        at: t - 1800,
    };
    assert_eq!(fresh::taken_at(&unread(), t, Some(&lg)), Some(t - 1800));
    assert_eq!(fresh::taken_at(&unread(), t, None), None);
    let carried = Probed {
        carried: true,
        ..rd(10.0, 10.0, None)
    };
    assert_eq!(
        fresh::taken_at(&carried, t, None),
        None,
        "no time of its own"
    );
    assert_eq!(
        fresh::taken_at(&rd(1.0, 1.0, Some(t - 60)), t, None),
        Some(t - 60)
    );
    assert_eq!(fresh::taken_at(&rd(1.0, 1.0, None), t, None), Some(t));

    assert!(fresh::recent(Some(t - 1800), None, t));
    assert!(!fresh::recent(None, None, t), "no time");
    assert!(!fresh::recent(Some(t - 18_001), None, t), "too old");
    assert!(!fresh::recent(Some(t + 1), None, t), "in the future");
    assert!(
        !fresh::recent(Some(t - 1800), Some(t - 600), t),
        "taken before the login was left"
    );
    assert!(fresh::recent(Some(t - 300), Some(t - 600), t));
}

/// The decision records the live login's standing block, with its kind and
/// end, only for a block: a live login that is only full (a window at 100%)
/// has none, so issue #3105's rule holds for it and nothing is taken unread.
#[test]
fn the_decision_records_only_a_standing_block_on_the_live_login() {
    let f = fleet(&["aa"], &[]);
    let decide = |live: Probed| {
        let mut ctx = ctx_with(&f.e, stub);
        ctx.now = clock;
        let snap = snapshot(
            &ctx,
            &crate::commands::profile::auto_switch::kit::State::new(ctx.store.root()).unwrap(),
        )
        .unwrap();
        let probes = vec![(name("aa"), rd(10.0, 10.0, None))];
        decide_with(&ctx, &snap, live, probes, clock(), &Extra::default()).unwrap()
    };
    assert_eq!(
        decide(exh(100.0, 50.0)).seen.live_blocked,
        None,
        "only full"
    );
    assert_eq!(decide(unread()).seen.live_blocked, None, "probe failed");
    let until = block_live(&f);
    for live in [exh(100.0, 50.0), rd(10.0, 10.0, None), unread()] {
        assert_eq!(
            decide(live).seen.live_blocked,
            Some((until, "spend limit")),
            "a block, whatever the probe"
        );
    }
}

// --- The gate, directly: the time limit, no swap back, and C3 --------------------------

/// The gate settled on `live` and `probes`, with the re-read hook set and
/// no time left, so every stale target meets the time limit.
fn gate_out_of_time(
    f: &Fleet,
    live: Probed,
    probes: Vec<(ProfileName, Probed)>,
    seed: impl FnOnce(&mut Extra),
) -> crate::commands::profile::rotate::observe::Measurement {
    let mut ctx = ctx_with(&f.e, stub);
    ctx.now = clock;
    ctx.reread = Some(reread_fails);
    let snap = snapshot(
        &ctx,
        &crate::commands::profile::auto_switch::kit::State::new(ctx.store.root()).unwrap(),
    )
    .unwrap();
    let at = clock();
    let mut extra = Extra::default();
    seed(&mut extra);
    let m = decide_with(&ctx, &snap, live, probes, at, &extra).unwrap();
    fresh::settle_timed(
        &ctx,
        m,
        Reread::Allowed,
        Duration::ZERO,
        || Duration::ZERO,
        |live, probes, extra, at| decide_with(&ctx, &snap, live, probes, at, extra),
    )
    .unwrap()
}

fn swap_to(m: &crate::commands::profile::rotate::observe::Measurement) -> Option<&ProfileName> {
    match &m.decision {
        Decision::Swap { to, .. } => Some(to),
        _ => None,
    }
}

/// A spent probe budget cannot confirm a swap onto a recent reading,
/// even when the live login has a spend block.
#[test]
fn at_the_time_limit_a_blocked_live_login_cannot_move_on_an_unconfirmed_reading() {
    let f = fleet(&["aa"], &[]);
    block_live(&f);
    let m = gate_out_of_time(
        &f,
        rd(94.0, 75.0, None),
        vec![(name("aa"), rd(30.0, 30.0, Some(clock() - 900)))],
        |_| {},
    );
    assert_eq!(swap_to(&m), None, "{:?}", m.decision);
    assert_eq!(m.seen.unconfirmed, None);
    assert!(m.seen.extra.confirmed.is_empty());
    assert_eq!(
        m.seen.live_blocked.map(|(_, kind)| kind),
        Some("spend limit")
    );
}

/// C2: the only usable target is the login the last swap left, and its
/// reading is from before it was left. It is never accepted on that
/// reading, so the rotator does not swap straight back onto it.
#[test]
fn a_reading_from_before_the_target_was_left_never_moves_back_onto_it() {
    let f = fleet(&["aa"], &[]);
    block_live(&f);
    let mut l = f.e.rs.load_ledger().unwrap().unwrap();
    l.previous = Some("aa".into());
    l.swapped_at = Some(clock() - 600);
    f.e.rs.save_ledger(&l).unwrap();
    let m = gate_out_of_time(
        &f,
        rd(94.0, 75.0, None),
        vec![(name("aa"), rd(30.0, 30.0, Some(clock() - 900)))],
        |_| {},
    );
    assert_eq!(swap_to(&m), None, "{:?}", m.decision);
    assert_eq!(m.seen.unconfirmed, None);
    let m = gate_out_of_time(
        &f,
        rd(94.0, 75.0, None),
        vec![(name("aa"), rd(30.0, 30.0, Some(clock() - 120)))],
        |_| {},
    );
    assert_eq!(
        swap_to(&m),
        None,
        "a later reading still needs confirmation"
    );
    assert!(m.seen.extra.confirmed.is_empty());
}

/// C3: a target an earlier gate accepted at a limit is not accepted again
/// once the live login is no longer blocked: it is unknown, with no
/// `reject:` line.
#[test]
fn a_target_accepted_at_a_limit_is_unknown_once_the_live_login_is_not_blocked() {
    let f = fleet(&["aa"], &[]);
    let mut ctx = ctx_with(&f.e, stub);
    ctx.now = clock;
    let snap = snapshot(
        &ctx,
        &crate::commands::profile::auto_switch::kit::State::new(ctx.store.root()).unwrap(),
    )
    .unwrap();
    let at = clock();
    let probes = vec![(name("aa"), rd(10.0, 10.0, Some(clock() - 900)))];
    let mut m = decide_with(
        &ctx,
        &snap,
        rd(96.0, 40.0, None),
        probes,
        at,
        &Extra::default(),
    )
    .unwrap();
    assert_eq!(
        swap_to(&m),
        Some(&name("aa")),
        "without the gate: {:?}",
        m.decision
    );
    assert_eq!(m.seen.live_blocked, None);
    m.seen.extra.unconfirmed.insert(
        name("aa"),
        Unconfirmed::NotTried {
            limit: Limit::Count,
            age: 900,
        },
    );
    let got = fresh::settle(
        &ctx,
        m,
        Reread::Never,
        Duration::from_secs(120),
        |live, probes, extra, at| decide_with(&ctx, &snap, live, probes, at, extra),
    )
    .unwrap();
    assert_eq!(swap_to(&got), None, "{:?}", got.decision);
    assert!(
        !got.fresh_lines.iter().any(|l| l.starts_with("reject:")),
        "{:?}",
        got.fresh_lines
    );
    assert_eq!(got.seen.unconfirmed, None);
}

/// The live room the decision reads for a blocked live login is no room,
/// whatever its probe, so the failed-probe rules cannot hold it (route 2).
#[test]
fn a_blocked_live_login_has_no_room_whatever_its_probe() {
    use crate::commands::profile::rotate::decide::target::live_room;
    for probe in [unread(), rd(10.0, 10.0, None), exh(100.0, 50.0)] {
        assert_eq!(live_room(&probe.reading, true), LiveRoom::NoRoom);
    }
}
