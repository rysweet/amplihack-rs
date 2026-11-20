//! Issue #2857, part 2: the real room gate, `change_may_end(plan, watch)`, and
//! how `Waiter::after_limit` uses it on the path that never sleeps.
//!
//! `wait` asks `WaitDeps::change_may_end` once a login change has been seen and
//! holds the change back while it answers no (`tests_wait_room.rs`, over a
//! Fake). This file pins what the real answer is, and the one place the wait
//! never starts: when the login already changed while the attempt was failing,
//! `after_limit` retries at once, so that retry needs the same answer.
//!
//! The rule, in one place: **a change may end a wait only when the login now
//! live has room**, with three named exceptions that open the gate:
//!
//! - a login-change wait (`Plan::capacity_may_end()` is false, #2731). It waits
//!   for one organisation's spend limit to be replaced by another's credit, and
//!   no capacity reading reports credit. Gating it would bring #2731 back.
//!   Part 1 is the backstop: if it wakes onto a full login, the 429 that
//!   follows re-enters the wait instead of ending the run.
//! - a watch that is not `$HOME/.claude` (an inherited profile, an explicit
//!   `CLAUDE_CONFIG_DIR`). The rotator swaps logins into `$HOME/.claude` and
//!   nowhere else, so its ledger says nothing about those steps' logins and
//!   their own fingerprint exits must go on working.
//! - a `$HOME/.claude` watch whose root has no rotator ledger at all. There is
//!   no rotator record to consult, and a host that never ran one has nothing to
//!   be wrong about.
//!
//! Everything else needs the checked ledger reader to return a record with
//! capacity that `says_room()`: `live_room == Some(true)`, or, for a person's
//! own `profile switch` (outcome `switched`, #1691), any record that does not
//! say `Some(false)`. A switch is the person's evidence about the login, and the
//! rotator measured nothing, so it has no bit to write. Every error path is
//! closed: a readable ledger with no bit, an unreadable ledger, an unknown
//! version, a symlink, a non-regular file, an oversize file, a file the world
//! can write (and one the group can, by the shared reader's own rule). There is
//! no `unwrap_or(true)` anywhere in the gate. Every "closed" case here carries
//! the positive control `live_room: true` on an identical fixture.
//!
//! Contract used here:
//! - `limit_wait::change_may_end(plan: &Plan, watch: &Watch) -> bool`;
//! - `CycleStatus::says_room(&self) -> bool`, the one rule the gate and
//!   `fresh_room` share (pinned in `amplihack-utils/tests/rotator_status.rs`);
//! - `Watch::rotator_state(&self) -> Ledger`, the one checked reader of the
//!   rotate ledger, whose room bit is pinned in `tests_fingerprint_ledgers`;
//! - `Waiter::resumed_on_a_signal(&mut self) -> bool`: whether the `Retry` the
//!   last `after_limit` returned came from a wait that ended on a positive
//!   signal (`End::said_try_now`), true once and then false (consumed on read),
//!   and false after any `after_limit` that did not return `Retry`, so an
//!   auto-switch rerun under another profile can never inherit it.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_room_gate;`.

use super::change_may_end;
use super::events::Met;
use super::fingerprint::Watch;
use super::plan::{Bound, WaitFor};
use super::settings::WaitSettings;
use super::tests_fingerprint::{Env, env, private, rotate_ledger};
use super::tests_wait::{capacity_plan_in, login_change_plan_in, plan_in};
use super::{AfterLimit, Resumed, Waiter};
use crate::commands::profile::attempt::Signals;
use crate::commands::recipe::run::limit_patterns::Harness;
use crate::test_support::{EnvGuard, env_lock};
use std::cell::Cell;
use std::fs;
use std::os::unix::fs::PermissionsExt;

// --- the gate -----------------------------------------------------------------

/// Writes one ledger the gate must read as no room.
type WriteLedger<'a> = Box<dyn Fn() + 'a>;

/// Each such ledger, named by what is wrong with it.
fn no_room_ledgers(e: &Env) -> Vec<(&'static str, WriteLedger<'_>)> {
    let ledger = rotate_ledger(e.store.root());
    let l1 = ledger.clone();
    let l2 = ledger.clone();
    let l3 = ledger.clone();
    let l4 = ledger.clone();
    vec![
        (
            "a login that reads full",
            Box::new(move || e.rotator_cycle_room("b", 1, None, "swapped", false, Some(false))),
        ),
        (
            "a ledger from a rotator that records no room bit",
            Box::new(move || e.rotator_cycle_room("b", 1, None, "swapped", false, None)),
        ),
        (
            "a ledger with an unknown version",
            Box::new(move || {
                private(
                    &l1,
                    r#"{"version":2,"live":"b","last_cycle":{"at":1,"outcome":"swapped","live_room":true}}"#,
                )
            }),
        ),
        (
            "a ledger that is not JSON",
            Box::new(move || private(&l2, "{not json")),
        ),
        (
            "a ledger writable by everyone",
            Box::new(move || {
                private(
                    &l3,
                    r#"{"version":1,"live":"b","last_cycle":{"at":1,"outcome":"swapped","live_room":true}}"#,
                );
                fs::set_permissions(&l3, fs::Permissions::from_mode(0o666)).unwrap();
            }),
        ),
        (
            "a live_room that is the string \"true\"",
            Box::new(move || {
                private(
                    &l4,
                    r#"{"version":1,"live":"b","last_cycle":{"at":1,"outcome":"swapped","live_room":"true"}}"#,
                )
            }),
        ),
    ]
}

#[test]
fn a_reset_or_capacity_wait_is_held_back_unless_the_ledger_records_room() {
    let e = env();
    let home = e.home_watch();
    for plan in [
        plan_in(300, Bound::ResetMargin),
        capacity_plan_in(300, Bound::MaxWait),
    ] {
        e.rotator_cycle_room("b", 1, None, "swapped", false, Some(true));
        assert!(
            change_may_end(&plan, &home),
            "positive control: room recorded"
        );
        for (what, write) in no_room_ledgers(&e) {
            write();
            assert!(!change_may_end(&plan, &home), "{what}");
        }
    }
}

/// The unreadable cases above are closed, and a ledger that is not there at all
/// is open: the two are different answers to different questions, and the
/// contrast is the point of this test. With no ledger there is no rotator
/// record to be wrong about; with an unreadable one there is a record that
/// cannot be trusted.
#[test]
fn a_missing_ledger_opens_the_gate_and_an_unreadable_one_does_not() {
    let e = env();
    let home = e.home_watch();
    let plan = plan_in(300, Bound::ResetMargin);
    assert!(
        change_may_end(&plan, &home),
        "no rotator ledger under the root: a host that never ran a rotator"
    );
    e.rotator_cycle_room("b", 1, None, "swapped", false, Some(true));
    fs::set_permissions(
        rotate_ledger(e.store.root()),
        fs::Permissions::from_mode(0o666),
    )
    .unwrap();
    assert!(
        !change_may_end(&plan, &home),
        "a ledger that is there and cannot be trusted is not a missing one"
    );
}

#[test]
fn a_symlinked_ledger_is_unreadable_not_missing() {
    let e = env();
    let home = e.home_watch();
    let plan = plan_in(300, Bound::ResetMargin);
    e.rotator_cycle_room("b", 1, None, "swapped", false, Some(true));
    assert!(change_may_end(&plan, &home), "positive control");
    let ledger = rotate_ledger(e.store.root());
    let real = e.tmp.join("elsewhere.json");
    fs::rename(&ledger, &real).unwrap();
    std::os::unix::fs::symlink(&real, &ledger).unwrap();
    assert!(!change_may_end(&plan, &home));
}

/// #2731: the organisation spend-limit wait is exempt, whatever the ledger says,
/// including a ledger that says no room and one that cannot be read.
#[test]
fn a_login_change_wait_is_never_held_back() {
    let e = env();
    let home = e.home_watch();
    let plan = login_change_plan_in(300);
    assert!(
        !plan.capacity_may_end(),
        "the plan under test is a login-change plan"
    );
    e.rotator_cycle_room("b", 1, None, "swapped", false, Some(true));
    assert!(change_may_end(&plan, &home), "room recorded");
    for (what, write) in no_room_ledgers(&e) {
        write();
        assert!(change_may_end(&plan, &home), "{what}");
    }
}

/// R1: the fingerprint route is not only for `$HOME/.claude`. An inherited
/// profile's login is moved by auto-switch, and an explicit config dir's login
/// by whoever owns it; neither is in the rotate ledger, so the ledger must not
/// hold their fingerprint exits back.
#[test]
fn a_step_that_is_not_on_home_claude_is_never_held_back() {
    let e = env();
    // The rotator's ledger says no room, loudly: it must make no difference.
    e.rotator_cycle_room("b", 1, None, "swapped", false, Some(false));
    let profile = e.profile("a");
    let cfg = e.tmp.join("cfg");
    let config_dir = Watch::new(Some(&e.store), None, Some(&cfg), Some(&e.home));
    for plan in [
        plan_in(300, Bound::ResetMargin),
        capacity_plan_in(300, Bound::MaxWait),
    ] {
        assert!(change_may_end(&plan, &profile), "an inherited profile");
        assert!(change_may_end(&plan, &config_dir), "CLAUDE_CONFIG_DIR");
    }
    // The positive control for the same fixture on `$HOME/.claude`: held back.
    assert!(!change_may_end(
        &plan_in(300, Bound::ResetMargin),
        &e.home_watch()
    ));
}

#[test]
fn the_gate_reads_the_ledger_each_time_and_follows_it() {
    let e = env();
    let home = e.home_watch();
    let plan = plan_in(300, Bound::ResetMargin);
    e.rotator_cycle_room("b", 1, None, "swapped", false, Some(false));
    assert!(!change_may_end(&plan, &home));
    e.rotator_cycle_room("b", 2, None, "stayed", false, Some(true));
    assert!(change_may_end(&plan, &home), "room appeared");
    e.rotator_cycle_room("c", 3, None, "swapped", false, Some(false));
    assert!(!change_may_end(&plan, &home), "and went again");
}

// --- a person's own `profile switch` (#1691) ----------------------------------------

/// A switch is recorded with outcome `switched` and no room bit, because the
/// rotator measured nothing about the login a person chose. It keeps waking a
/// wait, which is the escape hatch #1691 gave someone unsticking a waiting run.
/// The control on the same fixture is the rotator's own `swapped` with no bit,
/// which is the incident's shape and stays closed.
#[test]
fn a_manual_switch_opens_the_gate_without_a_room_reading() {
    let e = env();
    let home = e.home_watch();
    for plan in [
        plan_in(300, Bound::ResetMargin),
        capacity_plan_in(300, Bound::MaxWait),
    ] {
        e.rotator_cycle_room("b", 1, None, "swapped", false, None);
        assert!(
            !change_may_end(&plan, &home),
            "control: the rotator's own swap with no bit is closed"
        );
        e.rotator_cycle_room("b", 1, None, "switched", false, None);
        assert!(change_may_end(&plan, &home), "a person's switch, no bit");
    }
}

/// A switch that positively recorded no room is not let through: the carve-out
/// is for the absence of a reading, not for a reading of none.
#[test]
fn a_manual_switch_that_recorded_no_room_keeps_the_gate_closed() {
    let e = env();
    let home = e.home_watch();
    let plan = plan_in(300, Bound::ResetMargin);
    e.rotator_cycle_room("b", 1, None, "switched", false, Some(true));
    assert!(change_may_end(&plan, &home), "positive control");
    e.rotator_cycle_room("b", 1, None, "switched", false, Some(false));
    assert!(!change_may_end(&plan, &home));
}

/// The carve-out lifts the bit's requirement and nothing else. A switch the
/// rotator then alerted on has no capacity, so the gate stays closed.
#[test]
fn a_manual_switch_under_an_alert_keeps_the_gate_closed() {
    let e = env();
    let home = e.home_watch();
    let plan = plan_in(300, Bound::ResetMargin);
    e.rotator_cycle_room("b", 1, None, "switched", false, None);
    assert!(change_may_end(&plan, &home), "positive control");
    e.rotator_cycle_room("b", 1, None, "switched", true, None);
    assert!(!change_may_end(&plan, &home));
}

/// The whole table, outcome by reading of the bit, through the real reader.
/// `says_room` is pinned on its own in `amplihack-utils`; this says the gate
/// asks it, and asks it after `has_capacity`, so an outcome that is not
/// capacity is closed whatever the bit says.
#[test]
fn the_gate_answers_each_outcome_and_each_bit_as_says_room_does() {
    let e = env();
    let home = e.home_watch();
    let plan = plan_in(300, Bound::ResetMargin);
    let rows: [(&str, Option<bool>, bool); 15] = [
        ("swapped", None, false),
        ("swapped", Some(true), true),
        ("swapped", Some(false), false),
        ("stayed", None, false),
        ("stayed", Some(true), true),
        ("stayed", Some(false), false),
        ("switched", None, true),
        ("switched", Some(true), true),
        ("switched", Some(false), false),
        ("stayed-unread", Some(true), false),
        ("alerted", Some(true), false),
        ("held", Some(true), false),
        ("idle", Some(true), false),
        ("stopped", Some(true), false),
        ("", Some(true), false),
    ];
    for (outcome, room, want) in rows {
        e.rotator_cycle_room("b", 1, None, outcome, false, room);
        assert_eq!(
            change_may_end(&plan, &home),
            want,
            "outcome {outcome:?}, live_room {room:?}"
        );
    }
}

// --- a ledger the checks refuse is closed, not missing (SEC-3) ------------------------

/// A world write bit means anyone could have written the record the gate is
/// about to trust. Not `Missing`: there is a file, and it cannot be believed.
/// Each mode is checked against the same bytes at mode 0600 as the control, and
/// each says room.
#[test]
fn a_ledger_the_world_can_write_closes_the_gate() {
    let e = env();
    let home = e.home_watch();
    let plan = plan_in(300, Bound::ResetMargin);
    let ledger = rotate_ledger(e.store.root());
    for mode in [0o666, 0o606, 0o602, 0o662, 0o702] {
        e.rotator_cycle_room("b", 1, None, "swapped", false, Some(true));
        assert!(change_may_end(&plan, &home), "control at {mode:o}");
        fs::set_permissions(&ledger, fs::Permissions::from_mode(mode)).unwrap();
        assert!(
            !change_may_end(&plan, &home),
            "mode {mode:o} says room and must still not open the gate"
        );
    }
}

/// Group write is the checked reader's own rule and this change leaves it as it
/// is: it counts against a ledger unless the file's group is the user's private
/// group, the usual layout under a umask of 002, which the reader accepts by
/// design. So the gate is held to the reader's answer on this host, whichever
/// it is, and not to a mode that would pass on one layout and fail on another.
#[test]
fn a_ledger_the_group_can_write_is_judged_as_the_checked_reader_judges_it() {
    let e = env();
    let home = e.home_watch();
    let plan = plan_in(300, Bound::ResetMargin);
    let ledger = rotate_ledger(e.store.root());
    for mode in [0o660, 0o620] {
        e.rotator_cycle_room("b", 1, None, "swapped", false, Some(true));
        assert!(change_may_end(&plan, &home), "control at {mode:o}");
        fs::set_permissions(&ledger, fs::Permissions::from_mode(mode)).unwrap();
        let trusted =
            !crate::commands::profile::fsx::writable_by_others(&fs::metadata(&ledger).unwrap());
        assert_eq!(
            change_may_end(&plan, &home),
            trusted,
            "mode {mode:o}: the gate takes the reader's answer, neither more nor less trusting"
        );
    }
}

/// Over 64 KiB is refused whole, however the record in it reads.
#[test]
fn an_oversize_ledger_closes_the_gate() {
    let e = env();
    let home = e.home_watch();
    let plan = plan_in(300, Bound::ResetMargin);
    let ledger = rotate_ledger(e.store.root());
    e.rotator_cycle_room("b", 1, None, "swapped", false, Some(true));
    assert!(change_may_end(&plan, &home), "positive control");
    private(
        &ledger,
        &format!(
            r#"{{"version":1,"live":"b","pad":"{}","last_cycle":{{"at":1,"outcome":"swapped","live_room":true}}}}"#,
            "x".repeat(70 * 1024)
        ),
    );
    assert!(!change_may_end(&plan, &home));
}

/// Something that is not a regular file at the ledger's path is not a missing
/// ledger: a directory there was put there by something.
#[test]
fn a_ledger_that_is_not_a_regular_file_closes_the_gate() {
    let e = env();
    let home = e.home_watch();
    let plan = plan_in(300, Bound::ResetMargin);
    let ledger = rotate_ledger(e.store.root());
    e.rotator_cycle_room("b", 1, None, "swapped", false, Some(true));
    assert!(change_may_end(&plan, &home), "positive control");
    fs::remove_file(&ledger).unwrap();
    fs::create_dir(&ledger).unwrap();
    assert!(!change_may_end(&plan, &home));
}

// --- the path that never sleeps ----------------------------------------------------

/// A step timeout the step is nowhere near, and a clock fast enough that a
/// wait reaches its first 30-second poll in milliseconds. Neither is a timer
/// of the change under test: the waits here end on a reading the test gives.
const ROOMY_STEP_TIMEOUT: &str = "3600";
const FAST: &str = "1000";
const SILENCED: [&str; 4] = [
    "AMPLIHACK_USAGE_WAIT_LEDGER",
    "AMPLIHACK_RECIPE_LOG",
    "AMPLIHACK_USAGE_LIMIT_WAIT_TEST_NOW",
    "AMPLIHACK_USAGE_LIMIT_WAIT_TEST_SCALE",
];

fn guarded<T>(f: impl FnOnce() -> T) -> T {
    let _lock = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut guard = EnvGuard::set([("AMPLIHACK_STEP_TIMEOUT", ROOMY_STEP_TIMEOUT)]);
    for name in SILENCED {
        guard.clear(name);
    }
    guard.assign("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_SCALE", FAST);
    f()
}

fn waiter(e: &Env, max_wait_secs: Option<u64>) -> Waiter {
    let watch = Watch::new(Some(&e.store), None, None, Some(e.home.as_path()));
    assert!(watch.is_home(), "the test needs a $HOME/.claude step");
    Waiter::new(
        Some(WaitSettings {
            enabled: true,
            max_wait_secs,
            off_in_ci: false,
        }),
        watch,
        // A prompt in argv: nothing on stdin for the retry to need.
        super::claude_args::ClaudeArgs {
            print: true,
            prompt: true,
            stream_stdin: false,
        },
        Box::new(|| false),
        Box::new(|| true),
    )
}

fn signals() -> Signals {
    Signals::register().expect("the test process can register stop signals")
}

fn capacity() -> WaitFor {
    WaitFor::Capacity {
        harness: Harness::Claude,
        reset: None,
    }
}

fn login_change() -> WaitFor {
    WaitFor::LoginChange {
        harness: Harness::Claude,
    }
}

/// One `after_limit` call. The only place these tests name its parameters, so
/// that a change to them is one edit here.
fn after(w: &mut Waiter, wait: WaitFor, n: u32, probe: &dyn Fn(i64, i64) -> bool) -> AfterLimit {
    w.after_limit(
        wait,
        n,
        &signals(),
        probe,
        &mut || None,
        Resumed::Ordinarily,
    )
}

/// A login that changed while the attempt was failing, on a capacity wait:
/// `a` when the attempt was launched, `b` after its limit.
fn swapped_during_the_attempt(e: &Env, w: &mut Waiter, room_on_b: Option<bool>) {
    e.rotator_cycle_room("a", 1, None, "stayed", false, Some(true));
    w.note_attempt_login();
    e.rotator_cycle_room("b", 2, None, "swapped", false, room_on_b);
}

/// The wait that runs, in place of the retry at once, ends here: another login
/// reports capacity, which is what a test needs to end a wait that sleeps.
/// `probed` counts how many times it was read, which is how a test tells that
/// the wait ran from that it never did.
fn probe_counting<'a>(probed: &'a Cell<u32>) -> impl Fn(i64, i64) -> bool + 'a {
    move |_, _| {
        probed.set(probed.get() + 1);
        true
    }
}

#[test]
fn a_swap_onto_a_login_with_room_during_the_attempt_retries_at_once() {
    guarded(|| {
        let e = env();
        let mut w = waiter(&e, None);
        swapped_during_the_attempt(&e, &mut w, Some(true));
        let probed = Cell::new(0);
        let got = after(&mut w, capacity(), 1, &probe_counting(&probed));
        assert!(matches!(got, AfterLimit::Retry));
        assert_eq!(probed.get(), 0, "no wait ran: the login moved and has room");
    });
}

/// The same swap onto a login without room is not a reason to retry: the wait
/// runs, which is observable as the other-capacity reading it takes at its
/// first poll. The positive control above is the identical fixture with room.
#[test]
fn a_swap_onto_a_login_without_room_during_the_attempt_does_not_retry_at_once() {
    guarded(|| {
        for (what, room) in [("reads full", Some(false)), ("records no room bit", None)] {
            let e = env();
            let mut w = waiter(&e, None);
            swapped_during_the_attempt(&e, &mut w, room);
            let probed = Cell::new(0);
            let got = after(&mut w, capacity(), 1, &probe_counting(&probed));
            assert!(matches!(got, AfterLimit::Retry), "{what}");
            assert!(
                probed.get() >= 1,
                "{what}: the wait ran and was ended by another login's capacity, \
                 not skipped by the login having moved"
            );
        }
    });
}

#[test]
fn a_login_change_wait_still_retries_at_once_onto_a_login_without_room() {
    guarded(|| {
        for room in [Some(false), None] {
            let e = env();
            let mut w = waiter(&e, None);
            swapped_during_the_attempt(&e, &mut w, room);
            let probed = Cell::new(0);
            let got = after(&mut w, login_change(), 1, &probe_counting(&probed));
            assert!(matches!(got, AfterLimit::Retry), "{room:?}");
            assert_eq!(
                probed.get(),
                0,
                "{room:?}: #2731's wait is for a login change and no capacity reading \
                 reports the credit it waits for, so it is not gated"
            );
        }
    });
}

/// A person's `profile switch` that landed while the attempt was failing is
/// the same evidence it is during a wait: retry at once, with no reading of room
/// to ask for. Compare `a_swap_onto_a_login_without_room_...` above, where the
/// rotator's own swap with no bit runs the wait.
#[test]
fn a_manual_switch_during_the_attempt_retries_at_once_without_a_room_reading() {
    guarded(|| {
        let e = env();
        let mut w = waiter(&e, None);
        e.rotator_cycle_room("a", 1, None, "stayed", false, Some(true));
        w.note_attempt_login();
        e.rotator_cycle_room("b", 2, None, "switched", false, None);
        let probed = Cell::new(0);
        let got = after(&mut w, capacity(), 1, &probe_counting(&probed));
        assert!(matches!(got, AfterLimit::Retry));
        assert_eq!(probed.get(), 0, "no wait ran: a person moved the login");
        assert!(w.resumed_on_a_signal(), "and the retry said try now");
    });
}

#[test]
fn a_manual_switch_that_recorded_no_room_during_the_attempt_runs_the_wait() {
    guarded(|| {
        let e = env();
        let mut w = waiter(&e, None);
        e.rotator_cycle_room("a", 1, None, "stayed", false, Some(true));
        w.note_attempt_login();
        e.rotator_cycle_room("b", 2, None, "switched", false, Some(false));
        let probed = Cell::new(0);
        let got = after(&mut w, capacity(), 1, &probe_counting(&probed));
        assert!(matches!(got, AfterLimit::Retry));
        assert!(
            probed.get() >= 1,
            "the wait ran and was ended by another login's capacity: \
             a switch that recorded no room did not skip it"
        );
    });
}

// --- what a retry claims --------------------------------------------------------------

/// A wait that ended because another login reported capacity said *try now*:
/// the retry it earned is one a limit can contradict. Read once, then gone.
#[test]
fn a_retry_after_a_wait_that_said_try_now_is_reported_once_and_consumed() {
    guarded(|| {
        let e = env();
        let mut w = waiter(&e, None);
        e.rotator_cycle_room("a", 1, None, "stayed", false, Some(true));
        w.note_attempt_login();
        let probed = Cell::new(0);
        let got = after(&mut w, capacity(), 1, &probe_counting(&probed));
        assert!(matches!(got, AfterLimit::Retry));
        assert!(w.resumed_on_a_signal(), "ended on another login's capacity");
        assert!(!w.resumed_on_a_signal(), "consumed on read");
    });
}

#[test]
fn a_retry_that_skipped_the_wait_because_the_login_had_room_also_said_try_now() {
    guarded(|| {
        let e = env();
        let mut w = waiter(&e, None);
        swapped_during_the_attempt(&e, &mut w, Some(true));
        let got = after(&mut w, capacity(), 1, &|_, _| false);
        assert!(matches!(got, AfterLimit::Retry));
        assert!(
            w.resumed_on_a_signal(),
            "the login changed to one with room"
        );
    });
}

/// A wait that merely reached its maximum retried because the maximum said so,
/// not because anything said the login was usable: a limit after it is no
/// contradiction, and the `already_waited` refusal stands.
#[test]
fn a_retry_after_a_wait_that_ran_out_said_nothing() {
    guarded(|| {
        let e = env();
        // A budget of zero: the reset wait ends at once on its maximum and
        // still earns its one retry.
        let mut w = waiter(&e, Some(0));
        e.rotator_cycle_room("a", 1, None, "stayed", false, Some(true));
        w.note_attempt_login();
        let reset = w.now() + 1800;
        let wait = WaitFor::Reset {
            harness: Harness::Claude,
            reset,
        };
        let got = after(&mut w, wait, 1, &|_, _| false);
        assert!(
            matches!(got, AfterLimit::Retry),
            "max reached still retries"
        );
        assert!(
            !w.resumed_on_a_signal(),
            "`max reached` is the wait's own ceiling: nothing said to try"
        );
    });
}

/// Cleared on every return that is not a `Retry`, and set only at the two
/// `Retry` returns. A flag a `Finish` left set would be inherited by the next
/// attempt, which may be an auto-switch rerun under another profile.
#[test]
fn a_finish_clears_what_an_earlier_retry_set() {
    guarded(|| {
        let e = env();
        let mut w = waiter(&e, None);
        e.rotator_cycle_room("a", 1, None, "stayed", false, Some(true));
        w.note_attempt_login();
        let probed = Cell::new(0);
        let got = after(&mut w, capacity(), 1, &probe_counting(&probed));
        assert!(matches!(got, AfterLimit::Retry));
        // The flag is deliberately not read here. The next attempt runs on the
        // same login, which this step has waited on, and this call (made as an
        // ordinary one, not as the retry a signal earned) is refused: `Finish`.
        w.note_attempt_login();
        let got = after(&mut w, capacity(), 2, &|_, _| false);
        assert!(matches!(got, AfterLimit::Finish(_)), "already waited on a");
        assert!(
            !w.resumed_on_a_signal(),
            "a Finish carries nothing forward, whatever the last Retry said"
        );
    });
}

#[test]
fn a_waiter_that_has_never_waited_has_not_resumed_on_a_signal() {
    guarded(|| {
        let e = env();
        let mut w = waiter(&e, None);
        assert!(!w.resumed_on_a_signal());
        // Nor does noting a login change that.
        w.note_attempt_login();
        assert!(!w.resumed_on_a_signal());
    });
}

// --- what bounds a re-entry when nothing sets a maximum (SEC-7) -------------------------

/// What `plan_capacity` and `refuses_reentry` fall back to for a step with no
/// `max_wait_secs`: the same default, so a chain of re-entries has a budget and
/// not a count or a timer.
const DEFAULT_BUDGET: u64 = super::plan::CAPACITY_DEFAULT_MAX;

/// One `after_limit` call as the retry a wait that said *try now* earned.
fn after_a_signal(w: &mut Waiter, n: u32) -> AfterLimit {
    // A reset a minute away: with no maximum the wait runs to it, which at the
    // test clock's speed is two polls.
    let reset = w.now() + 60;
    let wait = WaitFor::Reset {
        harness: Harness::Claude,
        reset,
    };
    w.after_limit(
        wait,
        n,
        &signals(),
        &|_, _| false,
        &mut || None,
        Resumed::AfterASignal(Met::UsageLimit),
    )
}

/// A step that has already waited on this login and set no maximum is not
/// unlimited. With one second of the default budget left a re-entry is planned
/// (and ends at its own ceiling, so it retries); with none left it is refused
/// and the step ends, as the refusal for an ordinary second wait does.
#[test]
fn a_re_entry_with_no_maximum_set_ends_where_the_default_budget_does() {
    guarded(|| {
        let e = env();
        for (spent, re_entered) in [
            (0, true),
            (DEFAULT_BUDGET - 1, true),
            (DEFAULT_BUDGET, false),
        ] {
            let mut w = waiter(&e, None);
            w.note_attempt_login();
            w.marks.record(None);
            w.marks.spend(spent);
            let got = after_a_signal(&mut w, 2);
            assert_eq!(
                matches!(got, AfterLimit::Retry),
                re_entered,
                "{spent} of {DEFAULT_BUDGET} seconds spent"
            );
            if !re_entered {
                assert!(
                    matches!(got, AfterLimit::Finish(None)),
                    "a spent budget ends the step on the limit it already has"
                );
            }
        }
    });
}

// --- a re-entry needs something to wait on ----------------------------------------------

/// One `after_limit` call as the retry a wait that said *try now* earned, for a
/// limit whose message states a reset `ahead` seconds from now (negative for one
/// that has passed).
fn after_a_signal_for_a_reset(w: &mut Waiter, n: u32, ahead: i64) -> AfterLimit {
    let reset = w.now() + ahead;
    let wait = WaitFor::Reset {
        harness: Harness::Claude,
        reset,
    };
    w.after_limit(
        wait,
        n,
        &signals(),
        &|_, _| false,
        &mut || None,
        Resumed::AfterASignal(Met::UsageLimit),
    )
}

/// The same-login refusal is lifted for a re-entry so a limit that contradicts
/// a wake can be waited on again. It is not lifted for a limit that states a
/// reset which is not in the future: that wait would end at once, spend none of
/// the budget and launch the same attempt again, and so the budget would bound
/// nothing. With no reset ahead and no capacity signal there is nothing to wait
/// on, and the limit is passed up as already waited, as it was before #2857.
/// Issue #2857 ("until the stated reset or the next real capacity signal").
#[test]
fn a_re_entry_for_a_reset_that_is_not_ahead_is_refused_as_already_waited() {
    guarded(|| {
        let e = env();
        // The reset as the retry reads it: just reached, two minutes past (the
        // grace the first limit of a step gets), and far past it.
        for ahead in [0, -1, -120, -599] {
            let mut w = waiter(&e, None);
            w.note_attempt_login();
            w.marks.record(None);
            let got = after_a_signal_for_a_reset(&mut w, 2, ahead);
            assert!(
                matches!(got, AfterLimit::Finish(None)),
                "a reset {ahead}s from now gives a re-entry nothing to wait on"
            );
            assert!(
                !w.resumed_on_a_signal(),
                "a refusal carries no claim to the next attempt (reset {ahead}s)"
            );
        }
    });
}

/// The positive control: a reset still ahead is a wait, so the re-entry runs.
/// One second is enough; what matters is that it is in the future.
#[test]
fn a_re_entry_for_a_reset_that_is_ahead_is_planned() {
    guarded(|| {
        let e = env();
        for ahead in [1, 60, 1800] {
            let mut w = waiter(&e, None);
            w.note_attempt_login();
            w.marks.record(None);
            let got = after_a_signal_for_a_reset(&mut w, 2, ahead);
            assert!(
                matches!(got, AfterLimit::Retry),
                "a reset {ahead}s ahead is a wait"
            );
        }
    });
}

/// The refusal above is of the lifted same-login refusal only. A limit on a
/// login this step has not waited on is an ordinary first wait for that login,
/// and a reset inside the grace on it retries at once, as it always has
/// (`a_reset_inside_the_ten_minute_grace_retries_at_once` in the #1618 tests).
/// Refusing it too would end a step that moved to a fresh login on that login's
/// first limit.
#[test]
fn a_reset_that_is_not_ahead_on_a_login_not_yet_waited_on_still_retries_at_once() {
    guarded(|| {
        let e = env();
        let mut w = waiter(&e, None);
        w.note_attempt_login();
        // Nothing recorded: this step has not waited on this login.
        let got = after_a_signal_for_a_reset(&mut w, 1, -120);
        assert!(
            matches!(got, AfterLimit::Retry),
            "the grace retries at once"
        );
    });
}

/// The same-login rule that ends the chain for a past reset also bounds it: the
/// retry a past-reset wait earned is refused the next time, so the chain is the
/// first at-once retry and then the refusal, whatever the stub keeps saying.
#[test]
fn a_past_reset_chain_ends_at_the_second_limit_on_the_same_login() {
    guarded(|| {
        let e = env();
        let mut w = waiter(&e, None);
        w.note_attempt_login();
        let first = after_a_signal_for_a_reset(&mut w, 1, -120);
        assert!(matches!(first, AfterLimit::Retry), "the grace retries once");
        w.note_attempt_login();
        let second = after_a_signal_for_a_reset(&mut w, 2, -120);
        assert!(
            matches!(second, AfterLimit::Finish(None)),
            "the same message on the same login is refused"
        );
    });
}

/// The same re-entry with a maximum set is bounded by that maximum and not by
/// the default: a step that set 600 and spent 600 is done, though it is far
/// short of the default.
#[test]
fn a_re_entry_with_a_maximum_set_ends_where_that_maximum_does() {
    guarded(|| {
        let e = env();
        for (spent, re_entered) in [(599, true), (600, false)] {
            let mut w = waiter(&e, Some(600));
            w.note_attempt_login();
            w.marks.record(None);
            w.marks.spend(spent);
            let got = after_a_signal(&mut w, 2);
            assert_eq!(
                matches!(got, AfterLimit::Retry),
                re_entered,
                "spent {spent}"
            );
        }
    });
}
