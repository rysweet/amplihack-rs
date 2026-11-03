//! Issue #1880: unit tests for the live-probe classification
//! (`live_limit`), the stored failed-probe count (`failed_store` and the
//! ledger's `failed_live` field) and `damp` without the hold.
//!
//! Contract (see "Rotate failed live probes" in
//! docs/reference/profile-command.md):
//! - `live_limit::classify(p, live_overflow)`: a `Measured` probe is `Fresh`,
//!   or `Carried` when `p.carried`; `Exhausted` is `Limit { spend_limit:
//!   false }`; a spend limit is `Limit { spend_limit: true }` on a plan login
//!   and `OverflowBlocked` on an overflow one; every other `Failed` probe
//!   (timeout, still running, error, unparsed, HTTP 429) is `Unknown`.
//! - `for_decision`: only a plan spend limit changes, to `Reading::Exhausted`.
//! - `next_count`: `Unknown` +1 saturating; `Fresh`, `Limit` 0; `Carried`,
//!   `OverflowBlocked` unchanged.
//! - `relabel`: `Threshold` or `PlanRoom` with a plan spend limit becomes
//!   `spend-limit`; nothing else changes.
//! - `blocks_room(reading, k)`: `Failed` and `k < FAILED_PROBE_LIMIT`.
//! - `failed_store::prior` matches the key of a `LiveId::Known` profile, or
//!   `failed_store::ADOPTED` for `LiveId::Adopt`, exactly; `record` is the
//!   only writer; a malformed stored value loads as absent and never fails
//!   the ledger or reaches a log.
//!
//! The ledger's field is read and written through its JSON form here, so
//! these tests name no Rust field. Every test uses a temporary root; none
//! reads a real HOME, profile root or tmux server.

use super::damp::{FAILED_PROBE_LIMIT, damp};
use super::decide::{Decision, Probed, Reading, Reason, Windows};
use super::failed_store::{self, ADOPTED, MAX_STORED};
use super::identify::LiveId;
use super::ledger::{RotateLedger, RotateState, for_older_readers};
use super::live_limit::{LiveOutcome, blocks_room, classify, for_decision, next_count, relabel};
use super::tests_revalidate_cause::ctx_with;
use super::tests_swap::{env, n};
use crate::commands::profile::name::ProfileName;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;

const NOW: i64 = 1_760_000_000;

fn probe(reading: Reading) -> Probed {
    Probed {
        reading,
        windows: Windows::default(),
        spend_limit: false,
        unknown: false,
        still_running: false,
        observed_at: None,
        carried: false,
    }
}

fn measured(s: f64, w: f64) -> Probed {
    probe(Reading::Measured {
        session: s,
        week: w,
    })
}

fn spend_limited() -> Probed {
    Probed {
        spend_limit: true,
        ..probe(Reading::Failed)
    }
}

fn timed_out() -> Probed {
    Probed::failed()
}

fn still_running() -> Probed {
    Probed {
        still_running: true,
        ..Probed::failed()
    }
}

fn errored() -> Probed {
    probe(Reading::Failed)
}

fn swap(to: &str, reason: Reason) -> Decision {
    Decision::Swap { to: n(to), reason }
}

fn reason_tag(d: &Decision) -> Option<&'static str> {
    match d {
        Decision::Swap { reason, .. } => Some(reason.as_str()),
        _ => None,
    }
}

fn known(name: &str) -> LiveId {
    LiveId::Known(n(name))
}

/// A ledger whose `failed_live` is `v`, built from JSON as a file would be.
fn ledger_with(v: Value) -> RotateLedger {
    let mut l = serde_json::to_value(RotateLedger::default()).unwrap();
    l["failed_live"] = v;
    serde_json::from_value(l).unwrap()
}

fn failed_live_of(l: &RotateLedger) -> Value {
    serde_json::to_value(l).unwrap()["failed_live"].clone()
}

/// Writes `ledger.json` with `failed_live` set to `v`, byte for byte.
fn write_raw(rs: &RotateState, failed_live: Value) {
    let mut l = serde_json::to_value(RotateLedger::default()).unwrap();
    l["failed_live"] = failed_live;
    rs.write("ledger.json", l.to_string().as_bytes()).unwrap();
}

// --- live_limit::classify --------------------------------------------------

#[test]
fn classify_a_measured_reading_read_this_cycle_is_fresh() {
    assert_eq!(classify(&measured(10.0, 20.0), false), LiveOutcome::Fresh);
    let passive = Probed {
        observed_at: Some(NOW - 30),
        ..measured(10.0, 20.0)
    };
    assert_eq!(classify(&passive, false), LiveOutcome::Fresh, "passive");
}

#[test]
fn classify_a_measured_reading_from_an_earlier_read_is_carried() {
    let carried = Probed {
        carried: true,
        ..measured(10.0, 20.0)
    };
    assert_eq!(classify(&carried, false), LiveOutcome::Carried);
}

#[test]
fn classify_session_or_weekly_exhausted_is_a_limit_without_spend_limit() {
    for r in [
        Reading::EXHAUSTED,
        Reading::Exhausted {
            session: Some(100.0),
            week: Some(40.0),
        },
    ] {
        assert_eq!(
            classify(&probe(r), false),
            LiveOutcome::Limit { spend_limit: false },
            "{r:?}"
        );
    }
}

#[test]
fn classify_a_spend_limit_on_a_plan_login_is_a_spend_limit() {
    assert_eq!(
        classify(&spend_limited(), false),
        LiveOutcome::Limit { spend_limit: true }
    );
}

#[test]
fn classify_a_spend_limit_on_an_overflow_login_is_left_to_its_block() {
    assert_eq!(
        classify(&spend_limited(), true),
        LiveOutcome::OverflowBlocked
    );
}

#[test]
fn classify_timeouts_errors_and_reads_still_running_are_unknown() {
    // D2: an HTTP 429 from the usage request reads `Failed` like any other
    // error; it is not a limit.
    for (what, p) in [
        ("timeout", timed_out()),
        ("still running", still_running()),
        ("error / unparsed / 429", errored()),
    ] {
        assert_eq!(classify(&p, false), LiveOutcome::Unknown, "{what}");
        assert_eq!(classify(&p, true), LiveOutcome::Unknown, "{what} overflow");
    }
}

// --- live_limit::for_decision ----------------------------------------------

#[test]
fn a_plan_spend_limit_reads_as_exhausted_for_the_decision() {
    let p = spend_limited();
    let got = for_decision(&p, classify(&p, false));
    assert!(
        matches!(got.reading, Reading::Exhausted { .. }),
        "{:?}",
        got.reading
    );
    assert!(got.spend_limit, "the flag stays, for the log");
    assert_eq!(got.reading.headroom(), Some(0));
}

#[test]
fn every_other_probe_is_unchanged_for_the_decision() {
    for (p, overflow) in [
        (spend_limited(), true),
        (timed_out(), false),
        (still_running(), false),
        (errored(), false),
        (measured(10.0, 20.0), false),
        (probe(Reading::EXHAUSTED), false),
    ] {
        let o = classify(&p, overflow);
        assert_eq!(for_decision(&p, o), p, "{o:?}");
    }
}

// --- live_limit::next_count (D5) -------------------------------------------

#[test]
fn unknown_adds_one_and_saturates() {
    assert_eq!(next_count(0, LiveOutcome::Unknown), 1);
    assert_eq!(next_count(2, LiveOutcome::Unknown), 3);
    assert_eq!(next_count(u32::MAX, LiveOutcome::Unknown), u32::MAX);
}

#[test]
fn fresh_and_limits_clear_the_count() {
    for o in [
        LiveOutcome::Fresh,
        LiveOutcome::Limit { spend_limit: false },
        LiveOutcome::Limit { spend_limit: true },
    ] {
        assert_eq!(next_count(2, o), 0, "{o:?}");
    }
}

#[test]
fn carried_and_overflow_blocked_keep_the_count() {
    for o in [LiveOutcome::Carried, LiveOutcome::OverflowBlocked] {
        assert_eq!(next_count(0, o), 0, "{o:?}");
        assert_eq!(next_count(2, o), 2, "{o:?}");
    }
}

// --- live_limit::relabel ---------------------------------------------------

#[test]
fn a_threshold_or_plan_room_swap_off_a_plan_spend_limit_is_a_spend_limit_swap() {
    let o = LiveOutcome::Limit { spend_limit: true };
    for r in [Reason::Threshold, Reason::PlanRoom] {
        let d = relabel(swap("ffarmer", r), o);
        assert_eq!(reason_tag(&d), Some("spend-limit"), "{r:?}");
        assert!(
            matches!(&d, Decision::Swap { to, .. } if *to == n("ffarmer")),
            "the target is kept: {d:?}"
        );
    }
}

#[test]
fn relabel_leaves_every_other_decision_alone() {
    let spend = LiveOutcome::Limit { spend_limit: true };
    for (d, o) in [
        (swap("spare", Reason::OverflowIn), spend),
        (swap("spare", Reason::OverflowOut), spend),
        (swap("spare", Reason::Upgrade), spend),
        (Decision::Stay, spend),
        (Decision::Alert, spend),
        (
            swap("spare", Reason::Threshold),
            LiveOutcome::Limit { spend_limit: false },
        ),
        (swap("spare", Reason::Threshold), LiveOutcome::Unknown),
        (swap("spare", Reason::FailedProbe), LiveOutcome::Unknown),
        (swap("spare", Reason::PlanRoom), LiveOutcome::Fresh),
        (
            swap("spare", Reason::Threshold),
            LiveOutcome::OverflowBlocked,
        ),
    ] {
        assert_eq!(relabel(d.clone(), o), d, "{o:?}");
    }
}

#[test]
fn the_spend_limit_reason_is_its_own_tag() {
    let d = relabel(
        swap("spare", Reason::Threshold),
        LiveOutcome::Limit { spend_limit: true },
    );
    let tag = reason_tag(&d).unwrap();
    assert_eq!(tag, "spend-limit");
    for other in [
        Reason::Threshold,
        Reason::Upgrade,
        Reason::PlanRoom,
        Reason::FailedProbe,
        Reason::OverflowIn,
        Reason::OverflowOut,
    ] {
        assert_ne!(other.as_str(), tag, "{other:?}");
    }
}

// --- live_limit::blocks_room (D7) ------------------------------------------

#[test]
fn a_failed_live_login_reaches_plan_room_only_at_the_limit() {
    assert_eq!(FAILED_PROBE_LIMIT, 3);
    for k in 0..FAILED_PROBE_LIMIT {
        assert!(blocks_room(&Reading::Failed, k), "k={k}");
    }
    for k in [FAILED_PROBE_LIMIT, 4, u32::MAX] {
        assert!(!blocks_room(&Reading::Failed, k), "k={k}");
    }
}

#[test]
fn only_a_failed_reading_blocks_room() {
    for r in [
        Reading::EXHAUSTED,
        Reading::Measured {
            session: 90.0,
            week: 10.0,
        },
    ] {
        assert!(!blocks_room(&r, 0), "{r:?}");
    }
}

// --- damp without the hold (D1) --------------------------------------------

#[test]
fn the_third_failure_swaps_inside_the_hold() {
    // `damp` takes no swap time, so no hold can reach it (#1894).
    let (d, note) = damp(
        swap("spare", Reason::Threshold),
        &Reading::Failed,
        FAILED_PROBE_LIMIT,
    );
    assert_eq!(d, swap("spare", Reason::FailedProbe));
    assert_eq!(note, None);
}

#[test]
fn below_the_limit_a_failed_probe_still_stays_inside_the_hold() {
    let (d, note) = damp(swap("spare", Reason::Threshold), &Reading::Failed, 2);
    assert_eq!(d, Decision::Stay);
    assert_eq!(note.as_deref(), Some("stay: live probe failed (2/3)"));
}

#[test]
fn a_plan_room_swap_on_the_third_failure_is_a_failed_probe_swap() {
    // D7: with k >= 3 the plan-room swap is reachable for a failed live
    // login; the docs log it `reason=failed-probe`.
    let (d, note) = damp(
        swap("spare", Reason::PlanRoom),
        &Reading::Failed,
        FAILED_PROBE_LIMIT,
    );
    assert_eq!(d, swap("spare", Reason::FailedProbe));
    assert_eq!(note, None);
}

#[test]
fn an_exhausted_live_reading_passes_damp_inside_the_hold() {
    // A plan spend limit reaches `damp` as `Exhausted` (for_decision).
    for r in [Reason::Threshold, Reason::PlanRoom] {
        let d = swap("ffarmer", r);
        let got = damp(d.clone(), &Reading::EXHAUSTED, 0);
        assert_eq!(got, (d, None), "{r:?}");
    }
}

// --- failed_store::prior ---------------------------------------------------

#[test]
fn prior_is_the_stored_count_for_the_same_live_profile() {
    let l = ledger_with(json!({"profile": "work", "count": 2}));
    assert_eq!(failed_store::prior(Some(&l), &known("work")), 2);
}

#[test]
fn prior_is_zero_for_another_profile_or_case() {
    let l = ledger_with(json!({"profile": "spare", "count": 2}));
    assert_eq!(failed_store::prior(Some(&l), &known("work")), 0);
    let l = ledger_with(json!({"profile": "Work", "count": 2}));
    assert_eq!(
        failed_store::prior(Some(&l), &known("work")),
        0,
        "case-sensitive"
    );
}

#[test]
fn prior_is_zero_for_an_identity_without_a_swap_decision() {
    let l = ledger_with(json!({"profile": "work", "count": 2}));
    for live in [
        LiveId::Adopt,
        LiveId::HandChanged(n("work")),
        LiveId::HomeUnreadable(n("work")),
        LiveId::Ambiguous,
        LiveId::NoLogin,
    ] {
        assert_eq!(failed_store::prior(Some(&l), &live), 0, "{live:?}");
    }
}

#[test]
fn an_adopted_login_has_a_count_of_its_own() {
    // An adopted login has no profile name, so it is keyed by `ADOPTED`; a
    // profile's count never applies to it, nor its count to a profile.
    let l = ledger_with(json!({"profile": ADOPTED, "count": 2}));
    assert_eq!(failed_store::prior(Some(&l), &LiveId::Adopt), 2);
    assert_eq!(failed_store::prior(Some(&l), &known("work")), 0);
    let mut l = RotateLedger::default();
    failed_store::record(&mut l, &LiveId::Adopt, 2, false);
    assert_eq!(failed_live_of(&l), json!({"profile": ADOPTED, "count": 2}));
    failed_store::record(&mut l, &LiveId::Adopt, 3, true);
    assert_eq!(failed_live_of(&l), Value::Null, "a completed swap");
}

#[test]
fn the_adopted_key_round_trips_and_is_no_profile_name() {
    assert!(ProfileName::parse(ADOPTED).is_err());
    let e = env();
    write_raw(&e.rs, json!({"profile": ADOPTED, "count": 2}));
    let l = e.rs.load_ledger().unwrap().unwrap();
    assert_eq!(failed_live_of(&l), json!({"profile": ADOPTED, "count": 2}));
}

#[test]
fn prior_is_zero_without_a_ledger_or_a_record() {
    assert_eq!(failed_store::prior(None, &known("work")), 0);
    let l = RotateLedger::default();
    assert_eq!(failed_store::prior(Some(&l), &known("work")), 0);
}

#[test]
fn load_reads_zero_from_a_ledger_that_cannot_be_read() {
    let e = env();
    let ctx = ctx_with(&e, |_, _| unreachable!("not measured"));
    e.rs.write("ledger.json", b"{not json").unwrap();
    assert_eq!(failed_store::load(&ctx, &known("work")), 0);
}

#[test]
fn load_reads_the_stored_count_from_disk() {
    let e = env();
    let ctx = ctx_with(&e, |_, _| unreachable!("not measured"));
    write_raw(&e.rs, json!({"profile": "work", "count": 2}));
    assert_eq!(failed_store::load(&ctx, &known("work")), 2);
    assert_eq!(failed_store::load(&ctx, &known("spare")), 0);
}

// --- failed_store::record --------------------------------------------------

#[test]
fn record_stores_the_live_profile_and_count() {
    let mut l = RotateLedger::default();
    failed_store::record(&mut l, &known("work"), 2, false);
    assert_eq!(failed_live_of(&l), json!({"profile": "work", "count": 2}));
    failed_store::record(&mut l, &known("work"), 3, false);
    assert_eq!(failed_live_of(&l), json!({"profile": "work", "count": 3}));
}

#[test]
fn record_clears_on_zero_and_on_a_swap() {
    let mut l = ledger_with(json!({"profile": "work", "count": 2}));
    failed_store::record(&mut l, &known("work"), 0, false);
    assert_eq!(failed_live_of(&l), Value::Null, "count 0");
    let mut l = ledger_with(json!({"profile": "work", "count": 2}));
    failed_store::record(&mut l, &known("work"), 3, true);
    assert_eq!(failed_live_of(&l), Value::Null, "a completed swap");
}

#[test]
fn record_leaves_the_field_alone_for_an_identity_without_a_swap_decision() {
    for live in [
        LiveId::HandChanged(n("work")),
        LiveId::HomeUnreadable(n("work")),
        LiveId::Ambiguous,
        LiveId::NoLogin,
    ] {
        let mut l = ledger_with(json!({"profile": "work", "count": 2}));
        failed_store::record(&mut l, &live, 1, false);
        assert_eq!(
            failed_live_of(&l),
            json!({"profile": "work", "count": 2}),
            "{live:?}"
        );
    }
}

#[test]
fn record_keys_by_the_new_live_profile() {
    let mut l = ledger_with(json!({"profile": "spare", "count": 2}));
    failed_store::record(&mut l, &known("work"), 1, false);
    assert_eq!(failed_live_of(&l), json!({"profile": "work", "count": 1}));
}

// --- The ledger field ------------------------------------------------------

#[test]
fn the_field_round_trips_through_the_ledger_file() {
    let e = env();
    write_raw(&e.rs, json!({"profile": "work", "count": 2}));
    let l = e.rs.load_ledger().unwrap().unwrap();
    assert_eq!(failed_live_of(&l), json!({"profile": "work", "count": 2}));
    e.rs.save_ledger(&l).unwrap();
    let raw: Value = serde_json::from_slice(&fs::read(&e.rs.ledger).unwrap()).unwrap();
    assert_eq!(raw["failed_live"], json!({"profile": "work", "count": 2}));
    let mode = fs::metadata(&e.rs.ledger).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "private, as the rest of the ledger");
}

#[test]
fn an_absent_field_is_not_written() {
    let e = env();
    e.rs.save_ledger(&RotateLedger::default()).unwrap();
    let raw: Value = serde_json::from_slice(&fs::read(&e.rs.ledger).unwrap()).unwrap();
    assert!(raw.get("failed_live").is_none(), "{raw}");
}

#[test]
fn a_malformed_field_loads_as_absent_and_the_ledger_still_loads() {
    let e = env();
    for bad in [
        json!("garbage"),
        json!(5),
        json!([1, 2]),
        json!({"profile": "work"}),
        json!({"count": 2}),
        json!({"profile": "../x", "count": 2}),
        json!({"profile": "", "count": 2}),
        json!({"profile": 7, "count": 2}),
        json!({"profile": "work", "count": -1}),
        json!({"profile": "work", "count": 2.5}),
        json!({"profile": "work", "count": "2"}),
        json!({"profile": "work", "count": 2, "account": "u-work"}),
        json!({"profile": "work", "count": 0}),
    ] {
        write_raw(&e.rs, bad.clone());
        let l =
            e.rs.load_ledger()
                .unwrap_or_else(|err| panic!("{bad} must not fail the ledger: {err}"))
                .unwrap();
        assert_eq!(failed_live_of(&l), Value::Null, "{bad}");
        assert_eq!(failed_store::prior(Some(&l), &known("work")), 0, "{bad}");
    }
}

#[test]
fn a_huge_count_loads_capped() {
    assert_eq!(MAX_STORED, 1000);
    let e = env();
    for big in [json!(1001), json!(5000), json!(u64::from(u32::MAX) + 1)] {
        write_raw(&e.rs, json!({"profile": "work", "count": big}));
        let l = e.rs.load_ledger().unwrap().unwrap();
        assert_eq!(
            failed_live_of(&l),
            json!({"profile": "work", "count": MAX_STORED}),
            "{big}"
        );
    }
}

#[test]
fn a_malformed_value_never_reaches_an_error_or_the_saved_ledger() {
    let e = env();
    write_raw(&e.rs, json!({"profile": "work", "count": "SECRET-KEY-X"}));
    let l = e.rs.load_ledger().unwrap().unwrap();
    assert!(!format!("{l:?}").contains("SECRET-KEY-X"));
    e.rs.save_ledger(&l).unwrap();
    let raw = fs::read_to_string(&e.rs.ledger).unwrap();
    assert!(!raw.contains("SECRET-KEY"), "{raw}");
}

#[test]
fn older_readers_get_no_failed_live() {
    let l = ledger_with(json!({"profile": "work", "count": 2}));
    let older = serde_json::to_value(for_older_readers(&l, true)).unwrap();
    assert!(older.get("failed_live").is_none(), "{older}");
    let current = serde_json::to_value(for_older_readers(&l, false)).unwrap();
    assert_eq!(
        current["failed_live"],
        json!({"profile": "work", "count": 2})
    );
}

#[test]
fn the_ledger_version_stays_one() {
    let l = ledger_with(json!({"profile": "work", "count": 2}));
    assert_eq!(serde_json::to_value(&l).unwrap()["version"], json!(1));
}
