//! `rotate::damp`: failed-probe damping on top of the pure decision rule.
//!
//! Contract:
//! - `damp::FAILED_PROBE_LIMIT: u32` is 3.
//! - `damp::next_failed(prev: u32, live: &Reading) -> u32`: `prev + 1`
//!   (saturating) for `Reading::Failed`, else 0. `observe` stores it in
//!   `Ctx.failed_live`; a completed swap resets it to 0.
//! - `damp::damp(d: Decision, live: &Reading, consecutive_failed: u32,
//!   swapped_at: Option<i64>, now: i64, s: &Settings) -> (Decision,
//!   Option<String>)`. Acts only on `(Reading::Failed, Swap { reason:
//!   Threshold, .. })`:
//!   1. `k < 3`: `Stay`, note `stay: live probe failed (k/3)`;
//!   2. `decide::held(s.hold, swapped_at, now)`: `Stay`, note
//!      `stay: live probe failed; held after last swap (Ns left)`, with
//!      `N = hold - (now - swapped_at)`, or `hold` for a future `swapped_at`;
//!   3. otherwise `Swap { to, reason: Reason::FailedProbe }`, no note.
//!
//!   Every other input comes back unchanged with no note.
//! - `decide::held(hold: u64, swapped_at: Option<i64>, now: i64) -> bool` is
//!   the existing upgrade hold, moved out of `decide` unchanged.
//! - `Reason::FailedProbe.as_str()` is `failed-probe`.

use super::alert::{alert_key, should_alert};
use super::damp::{FAILED_PROBE_LIMIT, damp, next_failed};
use super::decide::{Decision, Measured, Reading, Reason, Windows, decide, held};
use super::ledger::RotateLedger;
use super::log::msg;
use super::settings::Settings;
use crate::commands::profile::name::ProfileName;

const NOW: i64 = 1_760_000_000;
const INTERVAL: i64 = 600;

fn settings() -> Settings {
    Settings {
        session_at: 85,
        week_at: 90,
        margin: 5,
        upgrade_margin: 30,
        hold: 3600,
        interval: 600,
        overflow_wait: 1800,
        overflow_unknown_reset: false,
    }
}

fn name(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn r(session: f64, week: f64) -> Reading {
    Reading::Measured { session, week }
}

fn c(n: &str, reading: Reading) -> Measured {
    Measured {
        name: name(n),
        reading,
        windows: Windows::default(),
        spend_limit: false,
    }
}

fn swap(to: &str, reason: Reason) -> Decision {
    Decision::Swap {
        to: name(to),
        reason,
    }
}

fn threshold_swap() -> Decision {
    swap("spare", Reason::Threshold)
}

// --- Pieces ----------------------------------------------------------------

#[test]
fn the_limit_is_three() {
    assert_eq!(FAILED_PROBE_LIMIT, 3);
}

#[test]
fn failed_probe_is_its_own_reason() {
    assert_eq!(Reason::FailedProbe.as_str(), "failed-probe");
    assert_eq!(Reason::Threshold.as_str(), "threshold");
    assert_eq!(Reason::Upgrade.as_str(), "upgrade");
}

#[test]
fn the_counter_counts_failed_live_probes_and_resets_on_anything_else() {
    assert_eq!(next_failed(0, &Reading::Failed), 1);
    assert_eq!(next_failed(2, &Reading::Failed), 3);
    assert_eq!(
        next_failed(u32::MAX, &Reading::Failed),
        u32::MAX,
        "saturates"
    );
    assert_eq!(next_failed(5, &r(10.0, 10.0)), 0);
    assert_eq!(
        next_failed(5, &r(95.0, 99.0)),
        0,
        "a measured breach resets"
    );
    assert_eq!(next_failed(5, &Reading::Exhausted), 0);
}

#[test]
fn held_is_the_upgrade_hold() {
    let hold = 3600;
    assert!(!held(hold, None, NOW), "never swapped");
    assert!(held(hold, Some(NOW), NOW));
    assert!(held(hold, Some(NOW - 3599), NOW));
    assert!(!held(hold, Some(NOW - 3600), NOW), "the hold has ended");
    assert!(held(hold, Some(NOW + 500), NOW), "a future swap time holds");
    assert!(!held(0, Some(NOW), NOW), "0 disables the hold");
}

// --- The damping table -----------------------------------------------------

#[test]
fn fewer_than_three_failures_stay_with_a_count() {
    for k in [1, 2] {
        let (d, note) = damp(
            threshold_swap(),
            &Reading::Failed,
            k,
            None,
            NOW,
            &settings(),
        );
        assert_eq!(d, Decision::Stay, "k={k}");
        assert_eq!(
            note.as_deref(),
            Some(&*format!("stay: live probe failed ({k}/3)"))
        );
    }
}

#[test]
fn a_zero_count_still_stays() {
    // One-shot runs pass 1; 0 must not be read as "no failures, swap".
    let (d, note) = damp(
        threshold_swap(),
        &Reading::Failed,
        0,
        None,
        NOW,
        &settings(),
    );
    assert_eq!(d, Decision::Stay);
    assert_eq!(note.as_deref(), Some("stay: live probe failed (0/3)"));
}

#[test]
fn the_third_failure_swaps_with_the_failed_probe_reason() {
    for k in [3, 4, 50] {
        let (d, note) = damp(
            threshold_swap(),
            &Reading::Failed,
            k,
            None,
            NOW,
            &settings(),
        );
        assert_eq!(d, swap("spare", Reason::FailedProbe), "k={k}");
        assert_eq!(note, None);
    }
}

#[test]
fn a_recent_swap_holds_a_failed_probe_swap() {
    let (d, note) = damp(
        threshold_swap(),
        &Reading::Failed,
        3,
        Some(NOW - 1800),
        NOW,
        &settings(),
    );
    assert_eq!(d, Decision::Stay);
    assert_eq!(
        note.as_deref(),
        Some("stay: live probe failed; held after last swap (1800s left)")
    );
}

#[test]
fn one_second_left_still_holds() {
    let (d, note) = damp(
        threshold_swap(),
        &Reading::Failed,
        3,
        Some(NOW - 3599),
        NOW,
        &settings(),
    );
    assert_eq!(d, Decision::Stay);
    assert_eq!(
        note.as_deref(),
        Some("stay: live probe failed; held after last swap (1s left)")
    );
}

#[test]
fn no_time_left_swaps() {
    let (d, note) = damp(
        threshold_swap(),
        &Reading::Failed,
        3,
        Some(NOW - 3600),
        NOW,
        &settings(),
    );
    assert_eq!(d, swap("spare", Reason::FailedProbe));
    assert_eq!(note, None);
}

#[test]
fn a_future_swap_time_holds_for_the_whole_hold() {
    let (d, note) = damp(
        threshold_swap(),
        &Reading::Failed,
        3,
        Some(NOW + 10_000),
        NOW,
        &settings(),
    );
    assert_eq!(d, Decision::Stay);
    assert_eq!(
        note.as_deref(),
        Some("stay: live probe failed; held after last swap (3600s left)")
    );
}

#[test]
fn the_count_rule_comes_before_the_hold() {
    let (d, note) = damp(
        threshold_swap(),
        &Reading::Failed,
        1,
        Some(NOW - 10),
        NOW,
        &settings(),
    );
    assert_eq!(d, Decision::Stay);
    assert_eq!(note.as_deref(), Some("stay: live probe failed (1/3)"));
}

#[test]
fn hold_zero_disables_only_the_hold() {
    let s = Settings {
        hold: 0,
        ..settings()
    };
    let (d, _) = damp(threshold_swap(), &Reading::Failed, 3, Some(NOW), NOW, &s);
    assert_eq!(d, swap("spare", Reason::FailedProbe));
    let (d, note) = damp(threshold_swap(), &Reading::Failed, 2, Some(NOW), NOW, &s);
    assert_eq!(d, Decision::Stay);
    assert_eq!(note.as_deref(), Some("stay: live probe failed (2/3)"));
}

#[test]
fn other_decisions_and_readings_pass_through_unchanged() {
    let s = settings();
    let cases = [
        // A measured breach and an exhausted live login swap at once.
        (threshold_swap(), r(90.0, 10.0)),
        (threshold_swap(), Reading::Exhausted),
        // Upgrades keep their own hold inside `decide`.
        (swap("spare", Reason::Upgrade), r(50.0, 10.0)),
        (swap("spare", Reason::Upgrade), Reading::Failed),
        // A failed probe with nowhere to go is still an alert.
        (Decision::Alert, Reading::Failed),
        (Decision::Alert, r(95.0, 95.0)),
        (Decision::Stay, Reading::Failed),
        (Decision::Stay, r(10.0, 10.0)),
    ];
    for (d, live) in cases {
        for k in [0, 1, 3] {
            let got = damp(d.clone(), &live, k, Some(NOW - 10), NOW, &s);
            assert_eq!(got, (d.clone(), None), "{d:?} {live:?} k={k}");
        }
    }
}

// --- Log lines -------------------------------------------------------------

#[test]
fn failed_probe_log_lines() {
    assert_eq!(
        msg::stay_failed_probe(1, 3),
        "stay: live probe failed (1/3)"
    );
    assert_eq!(
        msg::stay_failed_probe(2, 3),
        "stay: live probe failed (2/3)"
    );
    assert_eq!(
        msg::stay_held_failed(1800),
        "stay: live probe failed; held after last swap (1800s left)"
    );
    assert_eq!(
        msg::swap(
            &name("work"),
            &name("spare"),
            &Reading::Failed,
            &r(0.0, 12.0),
            Reason::FailedProbe
        ),
        "swap work->spare live failed target s=0% w=12% reason=failed-probe"
    );
}

// --- The swap loop (issue: rotator swaps every cycle on a failed probe) ----

/// One simulated `watch` cycle: what `observe` and `cycle` do with the
/// counter and the last swap time around `decide` and `damp`.
struct Sim {
    failed: u32,
    swapped_at: Option<i64>,
    swaps: Vec<i64>,
    notes: Vec<String>,
}

impl Sim {
    fn new() -> Self {
        Self {
            failed: 0,
            swapped_at: None,
            swaps: Vec::new(),
            notes: Vec::new(),
        }
    }

    fn cycle(&mut self, s: &Settings, live: &Reading, cands: &[Measured], now: i64) -> Decision {
        let raw = decide(s, live, cands, self.swapped_at, now);
        self.failed = next_failed(self.failed, live);
        let (d, note) = damp(raw, live, self.failed, self.swapped_at, now, s);
        if let Some(n) = note {
            self.notes.push(n);
        }
        if let Decision::Swap { .. } = d {
            self.swaps.push(now);
            self.swapped_at = Some(now);
            self.failed = 0;
        }
        d
    }
}

#[test]
fn without_damping_a_persistently_failed_live_probe_swaps_every_cycle() {
    // The bug, reproduced at the decision level: `decide` alone treats a
    // failed live probe as headroom 0 and the threshold swap ignores the
    // hold, so ten cycles give ten swaps (10/10).
    let s = settings();
    let cands = [c("spare", r(0.0, 12.0))];
    let mut swapped_at = None;
    let mut swaps = 0;
    for i in 0..10 {
        let now = NOW + i * INTERVAL;
        if let Decision::Swap { reason, .. } = decide(&s, &Reading::Failed, &cands, swapped_at, now)
        {
            assert_eq!(reason, Reason::Threshold);
            swaps += 1;
            swapped_at = Some(now);
        }
    }
    assert_eq!(
        swaps, 10,
        "decide is unchanged: the loop exists without damp"
    );
}

#[test]
fn damping_breaks_the_swap_loop() {
    let s = settings();
    let cands = [c("spare", r(0.0, 12.0))];
    let mut sim = Sim::new();
    let decisions: Vec<Decision> = (0..10)
        .map(|i| sim.cycle(&s, &Reading::Failed, &cands, NOW + i * INTERVAL))
        .collect();

    assert_eq!(decisions[0], Decision::Stay, "cycle 1");
    assert_eq!(decisions[1], Decision::Stay, "cycle 2");
    assert_eq!(decisions[2], swap("spare", Reason::FailedProbe), "cycle 3");
    assert!(sim.swaps.len() <= 2, "swaps at {:?}", sim.swaps);
    for pair in sim.swaps.windows(2) {
        assert!(
            pair[1] - pair[0] >= i64::try_from(s.hold).unwrap(),
            "two swaps inside one hold window: {:?}",
            sim.swaps
        );
    }
    assert_eq!(
        &sim.notes[..5],
        [
            "stay: live probe failed (1/3)",
            "stay: live probe failed (2/3)",
            "stay: live probe failed (1/3)",
            "stay: live probe failed (2/3)",
            "stay: live probe failed; held after last swap (1800s left)",
        ],
        "matches the example in docs/howto/claude-profile-rotation.md"
    );
}

#[test]
fn with_hold_zero_at_most_one_swap_every_three_cycles() {
    let s = Settings {
        hold: 0,
        ..settings()
    };
    let cands = [c("spare", r(0.0, 12.0))];
    let mut sim = Sim::new();
    for i in 0..12 {
        sim.cycle(&s, &Reading::Failed, &cands, NOW + i * INTERVAL);
    }
    assert_eq!(sim.swaps.len(), 4, "cycles 3, 6, 9 and 12: {:?}", sim.swaps);
    for pair in sim.swaps.windows(2) {
        assert!(pair[1] - pair[0] >= 3 * INTERVAL, "{:?}", sim.swaps);
    }
}

#[test]
fn a_good_reading_between_failures_restarts_the_count() {
    let s = settings();
    let cands = [c("spare", r(0.0, 12.0))];
    let mut sim = Sim::new();
    let readings = [
        Reading::Failed,
        Reading::Failed,
        r(10.0, 10.0),
        Reading::Failed,
        Reading::Failed,
    ];
    for (i, live) in readings.iter().enumerate() {
        sim.cycle(&s, live, &cands, NOW + i as i64 * INTERVAL);
    }
    assert!(sim.swaps.is_empty(), "never three failures in a row");
    assert_eq!(sim.failed, 2);
}

#[test]
fn a_measured_breach_still_swaps_at_once() {
    let s = settings();
    let cands = [c("spare", r(0.0, 12.0))];
    let mut sim = Sim::new();
    let d = sim.cycle(&s, &r(90.0, 10.0), &cands, NOW);
    assert_eq!(d, swap("spare", Reason::Threshold));
    assert!(sim.notes.is_empty());
}

// --- Every account at or above the threshold: alert once, stay put --------

#[test]
fn every_account_over_the_threshold_alerts_once_and_never_swaps() {
    let s = settings();
    let live = name("work");
    let cands = [
        c("spare", r(88.0, 92.0)),
        c("third", Reading::Exhausted),
        c("fourth", Reading::Failed),
    ];
    let mut ledger = RotateLedger::default();
    let key = alert_key("no-capacity", Some(&live));
    let mut sim = Sim::new();
    let mut alerts = Vec::new();
    for i in 0..10 {
        let d = sim.cycle(&s, &r(90.0, 95.0), &cands, NOW + i * INTERVAL);
        assert_eq!(d, Decision::Alert, "cycle {}", i + 1);
        alerts.push(should_alert(&mut ledger, &key));
    }
    assert!(sim.swaps.is_empty());
    assert!(alerts[0], "the first cycle writes the ALERT");
    assert!(alerts[1..].iter().all(|a| !a), "and later ones do not");
    assert_eq!(key, "no-capacity:work");
}

#[test]
fn a_failed_live_probe_with_no_usable_candidate_alerts_and_never_swaps() {
    let s = settings();
    let cands = [c("spare", Reading::Exhausted), c("third", Reading::Failed)];
    let mut sim = Sim::new();
    for i in 0..6 {
        let d = sim.cycle(&s, &Reading::Failed, &cands, NOW + i * INTERVAL);
        assert_eq!(d, Decision::Alert, "cycle {}", i + 1);
    }
    assert!(sim.swaps.is_empty());
    assert!(sim.notes.is_empty(), "an alert is not damped");
}

// --- Overflow (issue #1606) ------------------------------------------------

#[test]
fn overflow_moves_pass_through_damping_on_a_failed_live_probe() {
    // A spend-limit reading on a live overflow profile reads Failed; the move
    // it causes is an overflow move, not a threshold swap, so damping must
    // not hold it for three cycles or the hold.
    for reason in [Reason::OverflowIn, Reason::OverflowOut] {
        for k in [0, 1, FAILED_PROBE_LIMIT] {
            let d = swap("spare", reason);
            let (out, note) = damp(
                d.clone(),
                &Reading::Failed,
                k,
                Some(NOW - 10),
                NOW,
                &settings(),
            );
            assert_eq!(out, d, "{reason:?} k={k}");
            assert_eq!(note, None);
        }
    }
}

#[test]
fn overflow_moves_pass_through_damping_on_a_measured_live_probe() {
    for reason in [Reason::OverflowIn, Reason::OverflowOut] {
        let d = swap("spare", reason);
        let (out, note) = damp(d.clone(), &r(90.0, 40.0), 0, Some(NOW), NOW, &settings());
        assert_eq!(out, d);
        assert_eq!(note, None);
    }
}
