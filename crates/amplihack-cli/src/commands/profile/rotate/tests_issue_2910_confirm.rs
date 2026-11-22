//! Issue #2910, rule 3: the reading a swap is made on is one this cycle
//! took. A stored reading of the chosen target is read again before the
//! move; a reading that then shows no room drops that target for the next
//! candidate; and where no read can be made the swap stands and says so.
//!
//! On 21 November 2026 at 18:00Z the rotator on calypso swapped to nnorth,
//! which was spent within minutes, and about nine runs went into waits
//! bound to 22:22Z until the 18:10:15Z swap released them. The reading was
//! right when it was taken and wrong when it was used.
//!
//! These run through [`fresh::settle`], so what is pinned is the behaviour
//! of the gate and not the shape of the helper inside it. The re-read is
//! always a stub: nothing here runs `claude`, reads a login, or touches the
//! network, a real home or a tmux server.

use super::super::decide::{Decision, Probed, Reading, Windows};
use super::super::log::msg;
use super::super::observe::decide_measured;
use super::super::tests_issue_1870::{gate_ctx, panics, snap};
use super::super::tests_revalidate_cause::spent;
use super::super::tests_swap::{env, n};
use super::super::{cycle::Ctx, fresh};
use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::name::ProfileName;
use std::sync::Mutex;
use std::time::Duration;

/// A stored reading of `session`/`week`, taken at `at`: passive, so the
/// cycle did not read it itself, and fresh enough that the staleness rule
/// leaves it alone. Only issue #2910's own rule asks for it to be read
/// again.
fn stored(session: f64, week: f64, at: i64) -> Probed {
    Probed {
        reading: Reading::Measured { session, week },
        windows: Windows::default(),
        spend_limit: false,
        unknown: false,
        still_running: false,
        observed_at: Some(at),
        carried: false,
        passive: true,
    }
}

/// A reading taken in this cycle: not passive and not carried, so it is
/// already the confirmation rule 3 asks for.
fn read_now(session: f64, week: f64) -> Probed {
    Probed {
        passive: false,
        observed_at: None,
        ..stored(session, week, 0)
    }
}

/// The logins the re-read stubs were asked about. One record for the whole
/// file, so a test that reads it holds [`SERIAL`] for as long as it runs.
static READS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Held by every test that looks at [`READS`], because the harness runs
/// them on several threads and the record is process-wide.
static SERIAL: Mutex<()> = Mutex::new(());

/// Takes [`SERIAL`] and empties [`READS`], so a test starts from nothing
/// and nothing else writes to it while the test runs.
fn watching() -> std::sync::MutexGuard<'static, ()> {
    let g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    READS.lock().unwrap_or_else(|e| e.into_inner()).clear();
    g
}

fn reads() -> Vec<String> {
    std::mem::take(&mut *READS.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Every login this stub is asked about turns out to be spent.
fn now_full(_: &Ctx, name: &ProfileName, _: Duration) -> Probed {
    READS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(name.to_string());
    read_now(99.0, 20.0)
}

/// `spare` turns out to be spent; anything else still has room.
fn spare_full(_: &Ctx, name: &ProfileName, _: Duration) -> Probed {
    READS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(name.to_string());
    if name.as_str() == "spare" {
        read_now(99.0, 20.0)
    } else {
        read_now(5.0, 5.0)
    }
}

/// The gate's answer for a spent live login and the stored candidate
/// readings in `table`, with `reread` as the re-read hook and `budget` as
/// the time the whole gate has for its reads.
fn settled(
    reread: Option<fresh::RereadFn>,
    budget: Duration,
    mode: fresh::Reread,
    table: &[(&str, Probed)],
) -> (Decision, Vec<String>) {
    let e = env();
    let c = gate_ctx(&e, reread);
    let s = snap(&c);
    let at = now();
    let probes = table
        .iter()
        .map(|(name, p)| {
            let mut p = p.clone();
            if p.passive {
                p.observed_at = Some(at);
            }
            (n(name), p)
        })
        .collect();
    let m = decide_measured(&c, &s, spent(), probes, at).unwrap();
    let got = fresh::settle(&c, m, mode, budget, |live, probes, at| {
        decide_measured(&c, &s, live, probes, at)
    })
    .unwrap();
    // The lines in the order the cycle writes them: the gate's own, then
    // the `reject:` lines of the decision it settled on.
    let lines = got.fresh_lines.into_iter().chain(got.rejected).collect();
    (got.decision, lines)
}

const BUDGET: Duration = Duration::from_secs(30);

fn swapped_to(d: &Decision) -> Option<&str> {
    match d {
        Decision::Swap { to, .. } => Some(to.as_str()),
        _ => None,
    }
}

#[test]
fn a_stored_reading_of_the_chosen_target_is_read_again_before_the_swap() {
    let _watch = watching();
    let (d, lines) = settled(
        Some(spare_full as fresh::RereadFn),
        BUDGET,
        fresh::Reread::Allowed,
        &[("spare", stored(5.0, 5.0, 0))],
    );
    assert_eq!(reads(), ["spare"], "the chosen target was read again");
    assert!(
        lines.iter().any(|l| l.starts_with("confirm: spare was ")),
        "the confirming read is logged: {lines:?}"
    );
    assert_eq!(
        d,
        Decision::Alert,
        "the fresh reading showed no room: {d:?}"
    );
    assert!(
        lines.contains(&msg::no_room(&n("spare"))),
        "the target is dropped with its reason: {lines:?}"
    );
}

#[test]
fn a_target_whose_fresh_reading_shows_no_room_gives_way_to_the_next_candidate() {
    let _watch = watching();
    // `spare` ranks first on its stored figures and is spent in fact;
    // `extra` has less room on paper and is still usable.
    let (d, lines) = settled(
        Some(spare_full as fresh::RereadFn),
        BUDGET,
        fresh::Reread::Allowed,
        &[
            ("spare", stored(1.0, 1.0, 0)),
            ("extra", read_now(40.0, 40.0)),
        ],
    );
    assert_eq!(swapped_to(&d), Some("extra"), "{d:?}");
    assert_eq!(reads(), ["spare"], "only the dropped target was read again");
    assert!(
        lines.contains(&msg::no_room(&n("spare"))),
        "the skip is logged with its reason: {lines:?}"
    );
    assert_eq!(
        lines.last().map(String::as_str),
        Some("reject: spare no room left"),
        "the swap onto extra says why spare was passed over: {lines:?}"
    );
}

#[test]
fn a_reading_this_cycle_took_is_already_the_confirmation_and_is_not_read_twice() {
    let _watch = watching();
    let (d, lines) = settled(
        Some(now_full as fresh::RereadFn),
        BUDGET,
        fresh::Reread::Allowed,
        &[("spare", read_now(5.0, 5.0))],
    );
    assert_eq!(swapped_to(&d), Some("spare"), "{d:?}");
    assert!(
        reads().is_empty(),
        "issue #1870: no second read of a fresh target"
    );
    assert!(
        !lines.iter().any(|l| l.starts_with("confirm:")),
        "nothing to confirm: {lines:?}"
    );
}

#[test]
fn a_swap_made_without_a_confirming_read_says_so_rather_than_being_refused() {
    let _watch = watching();
    // The gate's whole time limit is spent, so no read can be made. The
    // swap stands on the stored reading and the weaker guarantee is said
    // out loud: refusing every swap for want of a reading would be an
    // outage, not a safety check.
    let (d, lines) = settled(
        Some(now_full as fresh::RereadFn),
        Duration::ZERO,
        fresh::Reread::Allowed,
        &[("spare", stored(5.0, 5.0, 0))],
    );
    assert!(reads().is_empty(), "no read was possible");
    assert_eq!(swapped_to(&d), Some("spare"), "{d:?}");
    assert!(
        lines.iter().any(|l| l == msg::CONFIRM_SKIPPED),
        "the skipped confirmation is logged: {lines:?}"
    );
}

#[test]
fn under_the_lock_no_confirming_read_is_made_and_the_skip_is_logged() {
    // `Reread::Never` is the revalidation path: it may not read at all, and
    // `panics` proves none was made. The swap is not refused for it, and
    // the skip is written, as the spent-budget one is, so a swap made on a
    // stored reading can be told from a confirmed one in the log.
    let (d, lines) = settled(
        Some(panics as fresh::RereadFn),
        BUDGET,
        fresh::Reread::Never,
        &[("spare", stored(5.0, 5.0, 0))],
    );
    assert_eq!(swapped_to(&d), Some("spare"), "{d:?}");
    assert!(
        lines.iter().any(|l| l == msg::CONFIRM_LOCKED),
        "the skipped confirmation is logged: {lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.starts_with("confirm: spare ")),
        "no reading was taken, so no figures are reported: {lines:?}"
    );
}

#[test]
fn under_the_lock_a_reading_this_cycle_took_is_swapped_to_with_no_skip_line() {
    // The target the cycle read directly needs no confirmation, so the
    // lock's own pass has nothing to skip and says nothing. Only a swap
    // standing on a stored reading writes `CONFIRM_LOCKED`.
    let (d, lines) = settled(
        Some(panics as fresh::RereadFn),
        BUDGET,
        fresh::Reread::Never,
        &[("spare", read_now(5.0, 5.0))],
    );
    assert_eq!(swapped_to(&d), Some("spare"), "{d:?}");
    assert!(
        !lines.iter().any(|l| l.starts_with("confirm:")),
        "the reading is already the confirmation: {lines:?}"
    );
}

#[test]
fn the_check_under_the_lock_is_quiet_once_the_cycle_has_confirmed_the_target() {
    // The usual order: the cycle's gate replaces the stored reading with
    // one it took, and the lock's pass then has nothing to skip. This is
    // what keeps `CONFIRM_LOCKED` evidence of a real departure rather than
    // a line on every revalidation.
    let _watch = watching();
    let e = env();
    let c = gate_ctx(&e, Some(spare_full as fresh::RereadFn));
    let s = snap(&c);
    let at = now();
    let probes = vec![(n("extra"), stored(5.0, 5.0, at))];
    let m = decide_measured(&c, &s, spent(), probes, at).unwrap();
    let again = |live, probes, at| decide_measured(&c, &s, live, probes, at);
    let cycle = fresh::settle(&c, m, fresh::Reread::Allowed, BUDGET, again).unwrap();
    assert_eq!(
        swapped_to(&cycle.decision),
        Some("extra"),
        "{:?}",
        cycle.decision
    );
    assert_eq!(reads(), ["extra"], "the cycle's own gate read it");
    assert!(
        cycle
            .fresh_lines
            .iter()
            .any(|l| l.starts_with("confirm: extra ")),
        "the cycle logged its confirming read: {:?}",
        cycle.fresh_lines
    );
    let mut locked = cycle;
    locked.fresh_lines.clear();
    let locked = fresh::settle(&c, locked, fresh::Reread::Never, BUDGET, again).unwrap();
    assert_eq!(
        swapped_to(&locked.decision),
        Some("extra"),
        "{:?}",
        locked.decision
    );
    assert!(reads().is_empty(), "the lock reads nothing");
    assert!(
        locked.fresh_lines.is_empty(),
        "a target the cycle confirmed needs no skip line: {:?}",
        locked.fresh_lines
    );
}

#[test]
fn with_no_candidate_left_the_cycle_alerts_and_says_why_it_passed_each_over() {
    let _watch = watching();
    let (d, lines) = settled(
        Some(now_full as fresh::RereadFn),
        BUDGET,
        fresh::Reread::Allowed,
        &[("spare", stored(5.0, 5.0, 0))],
    );
    assert_eq!(d, Decision::Alert, "no swap: {d:?}");
    assert!(
        lines.iter().any(|l| l.starts_with("reject: spare ")),
        "one line says why the only candidate was passed over: {lines:?}"
    );
    assert!(
        !matches!(d, Decision::Swap { .. }),
        "nothing ends and nothing moves: {d:?}"
    );
}

/// The two lines these paths write are fixed text with, at most, a profile
/// name in them: no figure a reading carried reaches either.
#[test]
fn the_confirmation_lines_are_fixed_text_naming_only_the_login() {
    assert_eq!(
        msg::no_room(&n("spare")),
        "reject: spare fresh reading shows no room"
    );
    assert!(!msg::CONFIRM_SKIPPED.contains('%'));
    assert!(!msg::CONFIRM_LOCKED.contains('%'));
    assert_ne!(
        msg::CONFIRM_SKIPPED,
        msg::CONFIRM_LOCKED,
        "the two skips are told apart in the log"
    );
}
