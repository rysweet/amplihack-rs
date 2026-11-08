//! Issue #1797: the capacity summary's arithmetic, worked out by the pure
//! `capacity_summary::compute` from one snapshot of valid readings, the
//! weights, the usable threshold and earlier history entries. No I/O.
//!
//! Contract (docs/reference/profile-command.md, "Capacity summary"):
//! - Headroom is kept in whole percentage points and is unweighted; the
//!   weighted figure is shown only when some current profile's weight is
//!   not 1.0.
//! - Unknown readings are never counted as 0% or 100%: they are left out of
//!   `N`, every sum and the rate, and only counted on the `unknown` line.
//! - Usable now: session below the threshold and week below 100%.
//! - Next session reset: session-limited logins with weekly capacity and a
//!   known reset, grouped within 60 s of the earliest.
//! - Rate: the oldest entry no more than 6 h old, at least 4.5 h old; each
//!   login's net rise in `w` within each week (issue #1970). A reset is
//!   `w_reset` passing or a fall of more than 10 points; a smaller fall is a
//!   stale reading and adds nothing. Never negative; weighted.
//! - Rounding half away from zero, only when printed; non-finite is `null`.

use super::capacity_summary::{
    Inputs, RESET_FALL, RateParts, Summary, compute, rate_parts, rounded,
};
use super::capacity_weight::Weights;
use super::usage_history::{Entry, Point, Snapshot, Writer};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// 2026-10-30T16:15:00Z, the issue's fixture time.
const T: i64 = 1_793_376_900;
const MIN: i64 = 60;
const HOUR: i64 = 3600;
const DAY: i64 = 86_400;

fn pt(s: Option<u8>, w: u8) -> Point {
    Point {
        s,
        w,
        w_reset: None,
        s_reset: None,
        no_session_limit: false,
        passive: false,
    }
}

fn pt_r(s: Option<u8>, w: u8, w_reset: Option<i64>, s_reset: Option<i64>) -> Point {
    Point {
        s,
        w,
        w_reset,
        s_reset,
        no_session_limit: false,
        passive: false,
    }
}

fn snap(at: i64, points: &[(&str, Point)], unknown: &[&str]) -> Snapshot {
    Snapshot {
        at,
        points: points
            .iter()
            .map(|(n, p)| (n.to_string(), p.clone()))
            .collect::<BTreeMap<_, _>>(),
        unknown: unknown.iter().map(|n| n.to_string()).collect(),
    }
}

fn entry(at: i64, points: &[(&str, Point)]) -> Entry {
    Entry {
        at,
        profiles: points
            .iter()
            .map(|(n, p)| (n.to_string(), p.clone()))
            .collect::<BTreeMap<_, _>>(),
        by: None,
        unknown: Vec::new(),
    }
}

fn summary(current: &Snapshot, weights: &Weights, threshold: u8, history: &[Entry]) -> Summary {
    compute(&Inputs {
        current,
        weights,
        threshold,
        history,
        now: current.at,
    })
}

fn plain(current: &Snapshot) -> Summary {
    summary(current, &Weights::default(), 95, &[])
}

fn js(s: &Summary) -> Value {
    serde_json::to_value(s).expect("the summary serialises")
}

fn lines(s: &Summary) -> Vec<String> {
    s.text_lines()
}

// ---------------------------------------------------------------------------
// The issue's fixture: 16 logins at 2026-10-30T16:15Z
// ---------------------------------------------------------------------------

/// Weekly used % from the issue. Sessions are not given there, so `ok` rows
/// read 50%, session-exhausted rows 100% and weekly-exhausted rows 30%.
/// eelsworth and hharkness reset their sessions 30 s apart at 17:50Z;
/// ddrumpf and ggrant at 19:30Z. ryanc2-mistt-personal's week resets at
/// 2026-10-31T08:00Z (inside 24 h) and ryan-mistt-org's at 22:59Z (outside).
fn fixture() -> Snapshot {
    let far = Some(T + 5 * DAY);
    let ok = |w| pt_r(Some(50), w, far, None);
    let sess = |w, at| pt_r(Some(100), w, far, Some(at));
    let full = |at| pt_r(Some(30), 100, Some(at), None);
    snap(
        T,
        &[
            ("jjansport", ok(43)),
            ("ryanc2-mistt-org", ok(80)),
            ("ccapstone", ok(89)),
            ("bbuck", ok(90)),
            ("aanderson", ok(91)),
            ("iivanovich", ok(48)),
            ("recovery-ryansweet", ok(94)),
            ("ryan-mistt-personal", ok(96)),
            ("eelsworth", sess(4, T + 95 * MIN)),
            ("hharkness", sess(4, T + 95 * MIN + 30)),
            ("ddrumpf", sess(92, T + 195 * MIN)),
            ("ggrant", sess(97, T + 195 * MIN)),
            ("ryanc2-mistt-personal", full(T + 15 * HOUR + 45 * MIN)),
            ("ryan-mistt-org", full(T + 30 * HOUR + 44 * MIN)),
            ("ryan-ryansweet", full(T + 3 * DAY)),
            ("rssourcelabs", full(T + 4 * DAY)),
        ],
        &[],
    )
}

#[test]
fn the_issue_fixture_gives_3_72_login_weeks_and_23_percent() {
    let s = plain(&fixture());
    let v = js(&s);
    assert_eq!(v["weekly_headroom_login_weeks"], json!(3.72));
    assert_eq!(v["weekly_headroom_pct"], json!(23.3), "23.25 rounds up");
    assert_eq!(v["weighted_headroom_login_weeks"], json!(3.72));
    assert_eq!(v["valid_readings"], json!(16));
    assert_eq!(v["unknown"], json!([]));
    assert_eq!(
        lines(&s)[0],
        "weekly headroom      3.72 login-weeks of 16 (23%)",
        "no weighted figure when every weight is 1.0"
    );
}

#[test]
fn the_issue_fixture_prints_the_six_documented_lines() {
    assert_eq!(
        lines(&plain(&fixture())),
        [
            "weekly headroom      3.72 login-weeks of 16 (23%)",
            "usable now           8 of 16 logins (session < 95% and weekly < 100%)",
            "next session reset   2026-10-30T17:50:00Z  (eelsworth, hharkness: +2 usable)",
            "runway               unknown",
            "resets in next 24 h  2026-10-31T08:00:00Z ryanc2-mistt-personal +1.00",
            "unknown              0 profiles (no valid reading; excluded from all figures)",
        ]
    );
}

#[test]
fn the_issue_fixture_json_has_every_documented_key() {
    let v = js(&plain(&fixture()));
    let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "next_session_reset",
            "rate_login_weeks_per_hour",
            "runway_hours",
            "unknown",
            "usable_now",
            "valid_readings",
            "weekly_headroom_login_weeks",
            "weekly_headroom_pct",
            "weekly_resets_next_24h",
            "weighted_headroom_login_weeks",
        ]
    );
    assert_eq!(v["usable_now"], json!(8));
    assert_eq!(
        v["next_session_reset"],
        json!({"at": "2026-10-30T17:50:00Z", "profiles": ["eelsworth", "hharkness"]})
    );
    assert_eq!(v["rate_login_weeks_per_hour"], Value::Null);
    assert_eq!(v["runway_hours"], Value::Null);
    assert_eq!(
        v["weekly_resets_next_24h"],
        json!([{"at": "2026-10-31T08:00:00Z", "profile": "ryanc2-mistt-personal", "restores": 1.0}])
    );
}

// ---------------------------------------------------------------------------
// The reference example: four valid logins, two unknown, six hours of history
// ---------------------------------------------------------------------------

/// The `profile usage` example in docs/reference/profile-command.md.
fn docs_now() -> Snapshot {
    snap(
        T,
        &[
            (
                "review",
                pt_r(Some(5), 10, Some(T + 4 * DAY + 16 * HOUR + 45 * MIN), None),
            ),
            (
                "work",
                pt_r(Some(20), 30, Some(T + 16 * HOUR + 45 * MIN), None),
            ),
            (
                "team",
                pt_r(
                    Some(100),
                    68,
                    Some(T + 5 * DAY + 16 * HOUR + 45 * MIN),
                    Some(T + 105 * MIN),
                ),
            ),
            (
                "night",
                pt_r(
                    Some(100),
                    85,
                    Some(T + 9 * HOUR + 45 * MIN),
                    Some(T + 105 * MIN + 40),
                ),
            ),
        ],
        &["scratch", "spare"],
    )
}

/// Six hours earlier, with the same weekly reset times: 138 points used
/// since (10 + 30 + 38 + 60), so 0.230 login-weeks an hour.
fn docs_history() -> Vec<Entry> {
    let now = docs_now();
    let at = T - 6 * HOUR;
    let was = |n: &str, w| {
        let p = &now.points[n];
        (n.to_string(), pt_r(Some(0), w, p.w_reset, None))
    };
    vec![Entry {
        at,
        profiles: [
            was("review", 0),
            was("work", 0),
            was("team", 30),
            was("night", 25),
        ]
        .into_iter()
        .collect(),
        by: None,
        unknown: Vec::new(),
    }]
}

#[test]
fn the_reference_example_prints_its_six_lines() {
    let s = summary(&docs_now(), &Weights::default(), 95, &docs_history());
    assert_eq!(
        lines(&s),
        [
            "weekly headroom      2.07 login-weeks of 4 (52%)",
            "usable now           2 of 4 logins (session < 95% and weekly < 100%)",
            "next session reset   2026-10-30T18:00:00Z  (night, team: +2 usable)",
            "runway               ~9 h at the last 6 h rate (0.230 login-weeks/h)",
            "resets in next 24 h  2026-10-31T02:00:00Z night +0.85; 2026-10-31T09:00:00Z work +0.30",
            "unknown              2 profiles (no valid reading; excluded from all figures)",
        ]
    );
}

#[test]
fn the_reference_example_json_matches_the_documented_object() {
    let s = summary(&docs_now(), &Weights::default(), 95, &docs_history());
    assert_eq!(
        js(&s),
        json!({
            "weekly_headroom_login_weeks": 2.07,
            "weekly_headroom_pct": 51.8,
            "weighted_headroom_login_weeks": 2.07,
            "usable_now": 2,
            "valid_readings": 4,
            "unknown": ["scratch", "spare"],
            "next_session_reset": {"at": "2026-10-30T18:00:00Z", "profiles": ["night", "team"]},
            "rate_login_weeks_per_hour": 0.23,
            "runway_hours": 9.0,
            "weekly_resets_next_24h": [
                {"at": "2026-10-31T02:00:00Z", "profile": "night", "restores": 0.85},
                {"at": "2026-10-31T09:00:00Z", "profile": "work", "restores": 0.3}
            ]
        })
    );
}

// ---------------------------------------------------------------------------
// Weights
// ---------------------------------------------------------------------------

#[test]
fn a_weight_other_than_one_adds_the_weighted_figure_and_weights_the_rate() {
    let w = Weights::from_pairs(&[("team", 2.0), ("scratch", 0.5)]);
    let s = summary(&docs_now(), &w, 95, &docs_history());
    let l = lines(&s);
    assert_eq!(
        l[0], "weekly headroom      2.07 login-weeks of 4 (52%)   [weighted: 2.39 of 5.00]",
        "the first figure stays unweighted; scratch is unknown so not in T"
    );
    let v = js(&s);
    assert_eq!(v["weekly_headroom_login_weeks"], json!(2.07));
    assert_eq!(v["weekly_headroom_pct"], json!(51.8));
    assert_eq!(v["weighted_headroom_login_weeks"], json!(2.39));
    // team's 38 points count twice: (10 + 30 + 76 + 60) / 100 / 6.
    assert_eq!(v["rate_login_weeks_per_hour"], json!(0.293));
    // 2.39 / 0.29333 = 8.15 hours.
    assert_eq!(v["runway_hours"], json!(8.1));
    assert!(
        l[3].starts_with("runway               ~8 h at the last 6 h rate (0.293"),
        "{}",
        l[3]
    );
}

#[test]
fn weights_of_exactly_one_print_no_weighted_figure() {
    let w = Weights::from_pairs(&[("team", 1.0), ("night", 1.0)]);
    let s = summary(&docs_now(), &w, 95, &docs_history());
    assert!(!lines(&s)[0].contains("weighted"), "{}", lines(&s)[0]);
}

#[test]
fn restores_on_the_24_hour_line_are_weighted() {
    let w = Weights::from_pairs(&[("night", 2.0)]);
    let s = summary(&docs_now(), &w, 95, &docs_history());
    assert_eq!(
        js(&s)["weekly_resets_next_24h"][0],
        json!({"at": "2026-10-31T02:00:00Z", "profile": "night", "restores": 1.7})
    );
    assert!(lines(&s)[4].contains("night +1.70;"), "{}", lines(&s)[4]);
}

// ---------------------------------------------------------------------------
// Unknown readings
// ---------------------------------------------------------------------------

#[test]
fn unknown_readings_are_never_counted_as_zero_or_one_hundred() {
    let known = snap(T, &[("a", pt(Some(10), 40)), ("b", pt(Some(10), 60))], &[]);
    let with_unknown = snap(
        T,
        &[("a", pt(Some(10), 40)), ("b", pt(Some(10), 60))],
        &["c", "d"],
    );
    let (k, u) = (js(&plain(&known)), js(&plain(&with_unknown)));
    for key in [
        "weekly_headroom_login_weeks",
        "weekly_headroom_pct",
        "weighted_headroom_login_weeks",
        "usable_now",
        "valid_readings",
    ] {
        assert_eq!(k[key], u[key], "{key} must ignore unknown readings");
    }
    assert_eq!(u["valid_readings"], json!(2));
    assert_eq!(u["weekly_headroom_pct"], json!(50.0));
    assert_eq!(u["unknown"], json!(["c", "d"]));
}

#[test]
fn the_unknown_line_counts_one_profile_in_the_singular() {
    let s = plain(&snap(T, &[("a", pt(Some(10), 40))], &["slow"]));
    assert_eq!(
        lines(&s)[5],
        "unknown              1 profile (no valid reading; excluded from all figures)"
    );
}

#[test]
fn zero_valid_readings_give_unknown_headroom_and_no_division_by_zero() {
    let s = plain(&snap(T, &[], &["a", "b"]));
    let v = js(&s);
    assert_eq!(v["weekly_headroom_login_weeks"], json!(0.0));
    assert_eq!(v["weekly_headroom_pct"], Value::Null);
    assert_eq!(v["weighted_headroom_login_weeks"], json!(0.0));
    assert_eq!(v["usable_now"], json!(0));
    assert_eq!(v["valid_readings"], json!(0));
    assert_eq!(v["unknown"], json!(["a", "b"]));
    assert_eq!(v["next_session_reset"], Value::Null);
    assert_eq!(v["rate_login_weeks_per_hour"], Value::Null);
    assert_eq!(v["runway_hours"], Value::Null);
    assert_eq!(v["weekly_resets_next_24h"], json!([]));
    assert_eq!(
        lines(&s),
        [
            "weekly headroom      unknown (no valid readings)",
            "usable now           0 of 0 logins (session < 95% and weekly < 100%)",
            "next session reset   none",
            "runway               unknown",
            "resets in next 24 h  none",
            "unknown              2 profiles (no valid reading; excluded from all figures)",
        ]
    );
}

#[test]
fn zero_valid_readings_with_history_still_give_unknown_runway() {
    let h = [entry(T - 5 * HOUR, &[("a", pt(Some(0), 10))])];
    let v = js(&summary(&snap(T, &[], &["a"]), &Weights::default(), 95, &h));
    assert_eq!(
        v["rate_login_weeks_per_hour"],
        Value::Null,
        "a is unknown now"
    );
    assert_eq!(v["runway_hours"], Value::Null);
}

// ---------------------------------------------------------------------------
// Usable now
// ---------------------------------------------------------------------------

#[test]
fn a_session_at_94_is_usable_and_at_95_is_not_by_default() {
    let s = plain(&snap(
        T,
        &[("a", pt(Some(94), 10)), ("b", pt(Some(95), 10))],
        &[],
    ));
    assert_eq!(js(&s)["usable_now"], json!(1));
}

#[test]
fn the_threshold_in_force_is_printed_and_applied() {
    let cur = snap(T, &[("a", pt(Some(79), 10)), ("b", pt(Some(80), 10))], &[]);
    let s = summary(&cur, &Weights::default(), 80, &[]);
    assert_eq!(js(&s)["usable_now"], json!(1));
    assert_eq!(
        lines(&s)[1],
        "usable now           1 of 2 logins (session < 80% and weekly < 100%)"
    );
    let all = summary(&cur, &Weights::default(), 100, &[]);
    assert_eq!(js(&all)["usable_now"], json!(2));
}

#[test]
fn a_full_week_or_a_missing_session_figure_is_not_usable_but_counts_in_headroom() {
    let s = plain(&snap(
        T,
        &[
            ("full", pt(Some(0), 100)),
            ("blind", pt(None, 20)),
            ("fine", pt(Some(0), 20)),
        ],
        &[],
    ));
    let v = js(&s);
    assert_eq!(v["usable_now"], json!(1));
    assert_eq!(v["valid_readings"], json!(3));
    assert_eq!(v["weekly_headroom_login_weeks"], json!(1.6));
}

#[test]
fn session_exhausted_logins_count_in_headroom_but_not_usable_now() {
    let s = plain(&snap(
        T,
        &[
            ("out", pt_r(Some(100), 20, None, Some(T + HOUR))),
            ("in", pt(Some(10), 20)),
        ],
        &[],
    ));
    let v = js(&s);
    assert_eq!(v["weekly_headroom_login_weeks"], json!(1.6));
    assert_eq!(v["usable_now"], json!(1));
}

// ---------------------------------------------------------------------------
// Next session reset
// ---------------------------------------------------------------------------

#[test]
fn session_resets_within_60_seconds_of_the_earliest_are_grouped() {
    let r = T + HOUR;
    let s = plain(&snap(
        T,
        &[
            ("c", pt_r(Some(100), 50, None, Some(r + 61))),
            ("b", pt_r(Some(100), 50, None, Some(r + 60))),
            ("a", pt_r(Some(100), 50, None, Some(r))),
        ],
        &[],
    ));
    assert_eq!(
        js(&s)["next_session_reset"],
        json!({"at": "2026-10-30T17:15:00Z", "profiles": ["a", "b"]})
    );
    assert_eq!(
        lines(&s)[2],
        "next session reset   2026-10-30T17:15:00Z  (a, b: +2 usable)"
    );
}

#[test]
fn a_full_week_or_an_unknown_reset_time_is_not_a_session_reset_candidate() {
    let s = plain(&snap(
        T,
        &[
            ("weekly", pt_r(Some(100), 100, None, Some(T + HOUR))),
            ("no-time", pt_r(Some(100), 50, None, None)),
            ("later", pt_r(Some(100), 50, None, Some(T + 3 * HOUR))),
        ],
        &[],
    ));
    assert_eq!(
        js(&s)["next_session_reset"],
        json!({"at": "2026-10-30T19:15:00Z", "profiles": ["later"]})
    );
}

#[test]
fn no_session_limited_login_gives_none_and_null() {
    let s = plain(&snap(
        T,
        &[("a", pt_r(Some(40), 50, None, Some(T + HOUR)))],
        &[],
    ));
    assert_eq!(js(&s)["next_session_reset"], Value::Null);
    assert_eq!(lines(&s)[2], "next session reset   none");
}

#[test]
fn a_session_reset_already_past_is_still_shown() {
    let s = plain(&snap(
        T,
        &[("a", pt_r(Some(100), 50, None, Some(T - 5 * MIN)))],
        &[],
    ));
    assert_eq!(
        js(&s)["next_session_reset"]["at"],
        json!("2026-10-30T16:10:00Z")
    );
}

// ---------------------------------------------------------------------------
// Rate and runway
// ---------------------------------------------------------------------------

fn rate_of(current: &Snapshot, history: &[Entry]) -> Value {
    js(&summary(current, &Weights::default(), 95, history))["rate_login_weeks_per_hour"].clone()
}

#[test]
fn history_younger_than_four_and_a_half_hours_gives_unknown_runway() {
    let cur = snap(T, &[("a", pt(Some(0), 50))], &[]);
    let h = [entry(T - 4 * HOUR, &[("a", pt(Some(0), 10))])];
    let s = summary(&cur, &Weights::default(), 95, &h);
    assert_eq!(js(&s)["rate_login_weeks_per_hour"], Value::Null);
    assert_eq!(js(&s)["runway_hours"], Value::Null);
    assert_eq!(lines(&s)[3], "runway               unknown");
}

#[test]
fn history_exactly_four_and_a_half_hours_old_is_enough() {
    let cur = snap(T, &[("a", pt(Some(0), 55))], &[]);
    let h = [entry(T - 4 * HOUR - 30 * MIN, &[("a", pt(Some(0), 10))])];
    // 45 points over 4.5 h: 0.1 login-weeks an hour.
    assert_eq!(rate_of(&cur, &h), json!(0.1));
}

#[test]
fn history_older_than_six_hours_is_not_used() {
    let cur = snap(T, &[("a", pt(Some(0), 50))], &[]);
    let h = [entry(T - 7 * HOUR, &[("a", pt(Some(0), 10))])];
    assert_eq!(rate_of(&cur, &h), Value::Null);
}

#[test]
fn the_oldest_entry_within_six_hours_is_the_start() {
    let cur = snap(T, &[("a", pt(Some(0), 70))], &[]);
    let h = [
        entry(T - 7 * HOUR, &[("a", pt(Some(0), 0))]),
        entry(T - 6 * HOUR, &[("a", pt(Some(0), 10))]),
        entry(T - 5 * HOUR, &[("a", pt(Some(0), 40))]),
    ];
    // 60 points over 6 h.
    assert_eq!(rate_of(&cur, &h), json!(0.1));
}

#[test]
fn history_out_of_order_is_still_read_by_time() {
    let cur = snap(T, &[("a", pt(Some(0), 70))], &[]);
    let h = [
        entry(T - 5 * HOUR, &[("a", pt(Some(0), 40))]),
        entry(T - 6 * HOUR, &[("a", pt(Some(0), 10))]),
    ];
    assert_eq!(rate_of(&cur, &h), json!(0.1));
}

#[test]
fn a_reset_found_by_w_reset_passing_adds_the_earlier_week_back() {
    // a's week reset at T-5h, after the start entry; by T-3h it read 40%
    // again (no fall), so only `w_reset` shows the reset. Use is 20 before
    // plus 50 since: 50 - 20 + 20 = 50 points over 6 h.
    let cur = snap(T, &[("a", pt_r(Some(0), 50, Some(T + 6 * DAY), None))], &[]);
    let h = [
        entry(
            T - 6 * HOUR,
            &[("a", pt_r(Some(0), 20, Some(T - 5 * HOUR), None))],
        ),
        entry(
            T - 3 * HOUR,
            &[("a", pt_r(Some(0), 40, Some(T + 6 * DAY), None))],
        ),
    ];
    assert_eq!(rate_of(&cur, &h), json!(0.083));
}

#[test]
fn a_reset_found_by_a_fall_in_the_week_adds_the_earlier_week_back() {
    // No reset times known; the week fell from 60 to 10. 30 - 60 + 60 = 30
    // points over 6 h.
    let cur = snap(T, &[("a", pt(Some(0), 30))], &[]);
    let h = [
        entry(T - 6 * HOUR, &[("a", pt(Some(0), 60))]),
        entry(T - 3 * HOUR, &[("a", pt(Some(0), 10))]),
    ];
    assert_eq!(rate_of(&cur, &h), json!(0.05));
}

#[test]
fn a_reset_between_the_last_entry_and_now_is_found_too() {
    let cur = snap(T, &[("a", pt(Some(0), 15))], &[]);
    let h = [entry(
        T - 6 * HOUR,
        &[("a", pt_r(Some(0), 90, Some(T - HOUR), None))],
    )];
    // 15 - 90 + 90 = 15 points over 6 h.
    assert_eq!(rate_of(&cur, &h), json!(0.025));
}

#[test]
fn a_rate_of_zero_gives_unknown_runway_but_keeps_the_rate() {
    let cur = snap(T, &[("a", pt(Some(0), 50))], &[]);
    let h = [entry(T - 6 * HOUR, &[("a", pt(Some(0), 50))])];
    let s = summary(&cur, &Weights::default(), 95, &h);
    assert_eq!(js(&s)["rate_login_weeks_per_hour"], json!(0.0));
    assert_eq!(js(&s)["runway_hours"], Value::Null);
    assert_eq!(lines(&s)[3], "runway               unknown");
}

#[test]
fn profiles_missing_at_either_end_are_left_out_of_the_rate() {
    // Only z is valid at both ends: 30 points over 6 h. x is new; y is
    // unknown now; neither moves the rate.
    let cur = snap(T, &[("x", pt(Some(0), 90)), ("z", pt(Some(0), 40))], &["y"]);
    let h = [entry(
        T - 6 * HOUR,
        &[("y", pt(Some(0), 0)), ("z", pt(Some(0), 10))],
    )];
    assert_eq!(rate_of(&cur, &h), json!(0.05));
}

fn written_by(by: Writer, at: i64, points: &[(&str, Point)], unknown: &[&str]) -> Entry {
    Entry {
        by: Some(by),
        unknown: unknown.iter().map(|n| n.to_string()).collect(),
        ..entry(at, points)
    }
}

/// The crusty round-2 case: four logins, B and C running sessions. Six
/// hours ago the rotator read only A and D, listing the busy B and C as
/// unknown; now `profile usage` reads all four.
fn busy_logins_now() -> Snapshot {
    snap(
        T,
        &[
            ("a", pt(Some(0), 20)),
            ("b", pt(Some(0), 80)),
            ("c", pt(Some(0), 70)),
            ("d", pt(Some(0), 20)),
        ],
        &[],
    )
}

fn rotator_line_without_the_busy_logins() -> Entry {
    written_by(
        Writer::Rotate,
        T - 6 * HOUR,
        &[("a", pt(Some(0), 10)), ("d", pt(Some(0), 20))],
        &["b", "c"],
    )
}

#[test]
fn a_rotator_line_that_left_out_busy_logins_is_never_the_start() {
    // Counting only A from that line would give 0.017 login-weeks/h and a
    // runway of 126 h while B and C are being used up.
    let cur = busy_logins_now();
    let s = summary(
        &cur,
        &Weights::default(),
        95,
        &[rotator_line_without_the_busy_logins()],
    );
    assert_eq!(js(&s)["rate_login_weeks_per_hour"], Value::Null);
    assert_eq!(js(&s)["runway_hours"], Value::Null);
    assert_eq!(lines(&s)[3], "runway               unknown");
}

#[test]
fn the_start_is_the_oldest_line_that_covers_every_current_login() {
    // The usage line 5 h ago read all four: 8 + 50 + 40 + 0 = 98 points
    // over 5 h is 0.196 login-weeks/h, and 2.1 login-weeks of headroom last
    // 10.7 h. The older rotator line is not used.
    let cur = busy_logins_now();
    let h = [
        rotator_line_without_the_busy_logins(),
        written_by(
            Writer::Usage,
            T - 5 * HOUR,
            &[
                ("a", pt(Some(0), 12)),
                ("b", pt(Some(0), 30)),
                ("c", pt(Some(0), 30)),
                ("d", pt(Some(0), 20)),
            ],
            &[],
        ),
    ];
    let s = summary(&cur, &Weights::default(), 95, &h);
    assert_eq!(js(&s)["rate_login_weeks_per_hour"], json!(0.196));
    assert_eq!(js(&s)["runway_hours"], json!(10.7));
}

#[test]
fn a_rotator_line_that_read_every_current_login_can_be_the_start() {
    let cur = snap(T, &[("a", pt(Some(0), 40)), ("b", pt(Some(0), 40))], &[]);
    let h = [written_by(
        Writer::Rotate,
        T - 6 * HOUR,
        &[("a", pt(Some(0), 10)), ("b", pt(Some(0), 10))],
        &[],
    )];
    // 60 points over 6 h.
    assert_eq!(rate_of(&cur, &h), json!(0.1));
}

#[test]
fn a_usage_start_leaves_out_only_the_logins_it_failed_to_read() {
    // Issue #1797: a failed reading at either end is left out. The usage
    // line could not read y, so only z counts: 30 points over 6 h.
    let cur = snap(T, &[("y", pt(Some(0), 90)), ("z", pt(Some(0), 40))], &[]);
    let h = [written_by(
        Writer::Usage,
        T - 6 * HOUR,
        &[("z", pt(Some(0), 10))],
        &["y"],
    )];
    assert_eq!(rate_of(&cur, &h), json!(0.05));
}

#[test]
fn rotator_lines_between_the_ends_still_find_resets() {
    // The rotator lines skipped b but saw a's week reset (90, then 5): a
    // used 10 + 5 + 20 = 35 points and b 30, so 65 points over 6 h.
    let cur = snap(T, &[("a", pt(Some(0), 25)), ("b", pt(Some(0), 40))], &[]);
    let h = [
        written_by(
            Writer::Usage,
            T - 6 * HOUR,
            &[("a", pt(Some(0), 80)), ("b", pt(Some(0), 10))],
            &[],
        ),
        written_by(
            Writer::Rotate,
            T - 4 * HOUR,
            &[("a", pt(Some(0), 90))],
            &["b"],
        ),
        written_by(
            Writer::Rotate,
            T - 3 * HOUR,
            &[("a", pt(Some(0), 5))],
            &["b"],
        ),
    ];
    let r = rate_of(&cur, &h).as_f64().expect("a rate");
    assert!((r - 65.0 / 100.0 / 6.0).abs() < 0.001, "{r}");
}

#[test]
fn runway_under_half_an_hour_prints_as_zero_hours() {
    let cur = snap(T, &[("a", pt(Some(0), 99))], &[]);
    let h = [entry(T - 6 * HOUR, &[("a", pt(Some(0), 0))])];
    let s = summary(&cur, &Weights::default(), 95, &h);
    assert_eq!(
        lines(&s)[3],
        "runway               ~0 h at the last 6 h rate (0.165 login-weeks/h)"
    );
    assert_eq!(js(&s)["runway_hours"], json!(0.1));
}

// ---------------------------------------------------------------------------
// Weekly resets in the next 24 hours
// ---------------------------------------------------------------------------

#[test]
fn weekly_resets_after_now_up_to_24_hours_with_something_to_restore() {
    let at = |d| Some(T + d);
    let s = plain(&snap(
        T,
        &[
            ("now", pt_r(Some(0), 50, at(0), None)),
            ("edge", pt_r(Some(0), 50, at(DAY), None)),
            ("past-edge", pt_r(Some(0), 50, at(DAY + 1), None)),
            ("empty", pt_r(Some(0), 0, at(HOUR), None)),
            ("zb", pt_r(Some(0), 20, at(2 * HOUR), None)),
            ("za", pt_r(Some(0), 75, at(2 * HOUR), None)),
            ("unset", pt_r(Some(0), 50, None, None)),
        ],
        &[],
    ));
    assert_eq!(
        js(&s)["weekly_resets_next_24h"],
        json!([
            {"at": "2026-10-30T18:15:00Z", "profile": "za", "restores": 0.75},
            {"at": "2026-10-30T18:15:00Z", "profile": "zb", "restores": 0.2},
            {"at": "2026-10-31T16:15:00Z", "profile": "edge", "restores": 0.5}
        ])
    );
    assert_eq!(
        lines(&s)[4],
        "resets in next 24 h  2026-10-30T18:15:00Z za +0.75; 2026-10-30T18:15:00Z zb +0.20; \
         2026-10-31T16:15:00Z edge +0.50"
    );
}

// ---------------------------------------------------------------------------
// Rounding
// ---------------------------------------------------------------------------

#[test]
fn rounded_is_half_away_from_zero_and_none_when_not_finite() {
    assert_eq!(rounded(23.25, 1), Some(23.3));
    assert_eq!(rounded(-23.25, 1), Some(-23.3));
    assert_eq!(rounded(0.125, 2), Some(0.13));
    assert_eq!(rounded(0.0004, 3), Some(0.0));
    assert_eq!(rounded(f64::NAN, 2), None);
    assert_eq!(rounded(f64::INFINITY, 1), None);
    assert_eq!(rounded(f64::NEG_INFINITY, 1), None);
}

#[test]
fn text_percentages_round_half_away_from_zero() {
    // 125 points of 200: 62.5%.
    let s = plain(&snap(
        T,
        &[("a", pt(Some(0), 75)), ("b", pt(Some(0), 0))],
        &[],
    ));
    assert_eq!(
        lines(&s)[0],
        "weekly headroom      1.25 login-weeks of 2 (63%)"
    );
    assert_eq!(js(&s)["weekly_headroom_pct"], json!(62.5));
}

#[test]
fn weighted_figures_round_only_when_printed() {
    // 0.5 x 25 / 100 = 0.125, exactly: printed as 0.13, not 0.12.
    let w = Weights::from_pairs(&[("a", 0.5)]);
    let s = summary(&snap(T, &[("a", pt(Some(0), 75))], &[]), &w, 95, &[]);
    assert_eq!(js(&s)["weighted_headroom_login_weeks"], json!(0.13));
    assert_eq!(
        lines(&s)[0],
        "weekly headroom      0.25 login-weeks of 1 (25%)   [weighted: 0.13 of 0.50]"
    );
}

#[test]
fn a_weight_for_an_absent_profile_does_not_show_the_weighted_figure() {
    // `gone` is in the weights file but is no current profile.
    let w = Weights::from_pairs(&[("gone", 2.0)]);
    let s = summary(
        &snap(T, &[("a", pt(Some(0), 75)), ("b", pt(Some(0), 0))], &[]),
        &w,
        95,
        &[],
    );
    assert_eq!(
        lines(&s)[0],
        "weekly headroom      1.25 login-weeks of 2 (63%)"
    );
    assert!(!s.status_line(T, T, None).contains("[weighted:"));
    assert_eq!(
        s,
        plain(&snap(
            T,
            &[("a", pt(Some(0), 75)), ("b", pt(Some(0), 0))],
            &[]
        ))
    );
}

#[test]
fn every_label_pads_to_21_characters() {
    for l in lines(&plain(&fixture())) {
        let (label, value) = l.split_at(21);
        assert!(!label.trim_end().is_empty(), "{l}");
        assert!(label.ends_with(' '), "{l}");
        assert!(!value.starts_with(' '), "{l}");
    }
}

// ---------------------------------------------------------------------------
// The `rotate status` capacity line
// ---------------------------------------------------------------------------

#[test]
fn the_status_line_gives_headroom_usable_and_runway_with_its_age() {
    let s = summary(&docs_now(), &Weights::default(), 95, &docs_history());
    assert_eq!(
        s.status_line(T, T + 3 * MIN, None),
        "capacity: headroom 2.07 login-weeks of 4 (52%), usable 2 of 4 logins, 2 unknown, \
         runway ~9 h (as of 2026-10-30T16:15:00Z, 3m ago)"
    );
}

#[test]
fn the_status_line_age_uses_the_largest_whole_unit() {
    let s = plain(&snap(T, &[("a", pt(Some(0), 50))], &[]));
    for (age, want) in [
        (45, "45s ago"),
        (3 * MIN + 59, "3m ago"),
        (2 * HOUR + 59 * MIN, "2h ago"),
        (DAY + 23 * HOUR, "1d ago"),
    ] {
        let l = s.status_line(T, T + age, None);
        assert!(
            l.ends_with(&format!("(as of 2026-10-30T16:15:00Z, {want})")),
            "{l}"
        );
        assert!(l.contains("runway unknown"), "{l}");
    }
}

#[test]
fn the_status_line_with_no_valid_readings() {
    let s = plain(&snap(T, &[], &[]));
    assert_eq!(
        s.status_line(T, T + 45, None),
        "capacity: headroom unknown (no valid readings), usable 0 of 0 logins, runway unknown \
         (as of 2026-10-30T16:15:00Z, 45s ago)"
    );
}

#[test]
fn the_status_line_adds_the_weighted_figure_after_the_percentage() {
    let w = Weights::from_pairs(&[("team", 2.0)]);
    let s = summary(&docs_now(), &w, 95, &docs_history());
    let l = s.status_line(T, T + 60, None);
    assert!(
        l.starts_with(
            "capacity: headroom 2.07 login-weeks of 4 (52%)   [weighted: 2.39 of 5.00], usable 2 of 4 logins, 2 unknown, runway ~"
        ),
        "{l}"
    );
}

// ---------------------------------------------------------------------------
// Issue #1970: the rate is each login's net rise within each week
// ---------------------------------------------------------------------------
//
// A stale status-line figure that reappears between rising readings made
// the week look as if it fell and rose again. Every fall was counted as a
// reset (issue #1841), so each flicker added the whole stale figure back.
// The rate now splits each login's readings into weeks at each real reset
// (`w_reset` passed, or a fall of more than `RESET_FALL` points) and counts
// the highest figure of each week minus the figure the week started at.

/// 2026-11-05T00:00:00Z, the day of the issue's flicker.
const N5: i64 = 1_793_836_800;

/// `N5` plus `h:m:s`.
fn n5(h: i64, m: i64, s: i64) -> i64 {
    N5 + h * HOUR + m * MIN + s
}

#[test]
fn n5_is_the_fifth_of_november_2026() {
    let t = chrono::DateTime::from_timestamp(N5, 0).unwrap();
    assert_eq!(t.to_rfc3339(), "2026-11-05T00:00:00+00:00");
}

fn wr(w: u8, w_reset: i64) -> Point {
    pt_r(Some(0), w, Some(w_reset), None)
}

fn passive_pt(w: u8, w_reset: Option<i64>) -> Point {
    Point {
        passive: true,
        ..pt_r(Some(0), w, w_reset, None)
    }
}

/// The parts of the rate for `current` at `current.at`, with weights 1.0.
fn parts(current: &Snapshot, history: &[Entry]) -> Option<RateParts> {
    parts_with(current, history, &Weights::default())
}

fn parts_with(current: &Snapshot, history: &[Entry], weights: &Weights) -> Option<RateParts> {
    rate_parts(&Inputs {
        current,
        weights,
        threshold: 95,
        history,
        now: current.at,
    })
}

/// Points `name` used over the window.
fn used(p: &RateParts, name: &str) -> i64 {
    p.used
        .get(name)
        .unwrap_or_else(|| panic!("{name} is not in the rate: {:?}", p.used))
        .0
}

/// The rate before rounding: Σ used · weight / 100 / hours.
fn raw_rate(p: &RateParts) -> f64 {
    let total: f64 = p.used.values().map(|(u, w)| *u as f64 * w).sum();
    total / 100.0 / (p.span as f64 / 3600.0)
}

/// The week of recovery-ryansweet on 5 Nov resets on 10 Nov at 09:00Z.
const FLICKER_RESET: i64 = N5 + 5 * DAY + 9 * HOUR;
const FLICKER: &str = "recovery-ryansweet";

/// The sequence from the issue. A `profile usage` line read 34% at 13:00Z
/// and rotator lines held 34% at 14:00Z and 15:30Z. From 16:48Z the
/// figure rose to 50%, but a cached 38% kept reappearing between rising
/// readings. Every fall is 7 points or less.
fn flicker_history() -> Vec<Entry> {
    let mut h = vec![written_by(
        Writer::Usage,
        n5(13, 0, 0),
        &[(FLICKER, wr(34, FLICKER_RESET))],
        &[],
    )];
    let seq: [(i64, i64, u8); 22] = [
        (14, 0, 34),
        (15, 30, 34),
        (16, 48, 34),
        (17, 0, 35),
        (17, 15, 36),
        (17, 30, 37),
        (17, 45, 38),
        (17, 47, 40),
        (17, 54, 38),
        (17, 56, 42),
        (18, 2, 38),
        (18, 4, 43),
        (18, 8, 38),
        (18, 9, 44),
        (18, 15, 38),
        (18, 17, 45),
        (18, 25, 38),
        (18, 27, 45),
        (18, 35, 38),
        (18, 40, 46),
        (18, 42, 49),
        (18, 48, 50),
    ];
    for (hh, mm, w) in seq {
        h.push(written_by(
            Writer::Rotate,
            n5(hh, mm, 0),
            &[(FLICKER, wr(w, FLICKER_RESET))],
            &[],
        ));
    }
    h
}

fn flicker_now() -> Snapshot {
    snap(n5(18, 55, 0), &[(FLICKER, wr(50, FLICKER_RESET))], &[])
}

#[test]
fn the_5_nov_flicker_counts_its_net_rise_of_16_points() {
    let h = flicker_history();
    // Every fall in the fixture is within the threshold, so none is a
    // reset; the test would not show the fix otherwise.
    let ws: Vec<u8> = h.iter().map(|e| e.profiles[FLICKER].w).collect();
    let biggest_fall = ws
        .windows(2)
        .map(|p| i16::from(p[0]) - i16::from(p[1]))
        .max()
        .unwrap();
    assert_eq!(biggest_fall, 7);
    assert!(biggest_fall <= i16::from(RESET_FALL));

    let p = parts(&flicker_now(), &h).expect("a start 5.92 h old");
    assert_eq!(p.start, n5(13, 0, 0));
    assert_eq!(p.span, 5 * HOUR + 55 * MIN);
    assert_eq!(used(&p, FLICKER), 16, "50 - 34, whatever the flicker");
    assert_eq!(p.used[FLICKER].1, 1.0);

    // 0.16 login-weeks over 5.9167 h.
    let s = summary(&flicker_now(), &Weights::default(), 95, &h);
    let v = js(&s);
    assert_eq!(v["rate_login_weeks_per_hour"], json!(0.027));
    assert!(v["rate_login_weeks_per_hour"].as_f64().unwrap() < 0.05);
    // 0.5 login-weeks of headroom at 0.02704 an hour.
    assert_eq!(v["runway_hours"], json!(18.5));
}

#[test]
fn a_flicker_of_stale_readings_adds_the_same_as_none() {
    // The same day without the cached 38%: the rate must not change.
    let clean: Vec<Entry> = flicker_history()
        .into_iter()
        .enumerate()
        .filter(|(i, e)| *i == 0 || e.at <= n5(17, 45, 0) || e.profiles[FLICKER].w != 38)
        .map(|(_, e)| e)
        .collect();
    assert!(clean.len() < flicker_history().len());
    assert_eq!(
        rate_of(&flicker_now(), &flicker_history()),
        rate_of(&flicker_now(), &clean)
    );
}

// The per-login check from the issue, at the 16:52Z reading: three logins
// with small stale falls, and ryan-mistt-personal's 100 -> 0 reset at
// 12:00:56Z before the `profile usage` start at 13:23:37Z.

const RMP: &str = "ryan-mistt-personal";
const REC: &str = "recovery-ryansweet";
const CC: &str = "ccapstone";

fn per_login_resets() -> (i64, i64, i64) {
    (
        n5(12, 0, 56) + 7 * DAY,
        N5 + 5 * DAY + 9 * HOUR,
        N5 + 3 * DAY + 17 * HOUR,
    )
}

/// One reading per login that has a figure; `None` means the line left
/// that login out.
fn per_login_line(by: Writer, at: i64, rmp: Option<u8>, rec: Option<u8>, cc: Option<u8>) -> Entry {
    let (r1, r2, r3) = per_login_resets();
    let mut points = Vec::new();
    let mut unknown = Vec::new();
    for (n, w, r) in [(RMP, rmp, r1), (REC, rec, r2), (CC, cc, r3)] {
        match w {
            Some(w) => points.push((n, wr(w, r))),
            None => unknown.push(n),
        }
    }
    written_by(by, at, &points, &unknown)
}

fn per_login_history() -> Vec<Entry> {
    let rotate = Writer::Rotate;
    let old_reset = n5(12, 0, 56);
    vec![
        // Before the start: rmp's week was full and reset at 12:00:56Z.
        // These lines left the other logins out, so neither is the start.
        written_by(
            rotate,
            n5(11, 50, 0),
            &[(RMP, wr(100, old_reset))],
            &[REC, CC],
        ),
        per_login_line(rotate, n5(12, 5, 0), Some(0), None, None),
        per_login_line(Writer::Usage, n5(13, 23, 37), Some(8), Some(20), Some(50)),
        per_login_line(rotate, n5(14, 10, 0), Some(12), Some(24), Some(52)),
        per_login_line(rotate, n5(14, 40, 0), Some(15), Some(29), None),
        per_login_line(rotate, n5(15, 5, 0), Some(18), Some(26), Some(55)),
        per_login_line(rotate, n5(15, 20, 0), Some(15), None, Some(54)),
        per_login_line(rotate, n5(15, 50, 0), Some(20), Some(30), None),
        per_login_line(rotate, n5(16, 30, 0), Some(22), Some(31), Some(57)),
    ]
}

fn per_login_now() -> Snapshot {
    let (r1, r2, r3) = per_login_resets();
    snap(
        n5(16, 52, 0),
        &[(RMP, wr(23, r1)), (REC, wr(31, r2)), (CC, wr(57, r3))],
        &[],
    )
}

#[test]
fn the_5_nov_logins_each_count_their_own_net_rise() {
    let h = per_login_history();
    let now = per_login_now();
    let p = parts(&now, &h).expect("the parts are given below 4.5 h too");
    assert_eq!(p.start, n5(13, 23, 37), "the profile usage line");
    assert_eq!(p.span, 3 * HOUR + 28 * MIN + 23);
    // 18 -> 15 and 29 -> 26 and 55 -> 54 each add nothing.
    assert_eq!(used(&p, RMP), 23 - 8);
    assert_eq!(used(&p, REC), 31 - 20);
    assert_eq!(used(&p, CC), 57 - 50);
    assert_eq!(p.used.len(), 3);
    assert!(p.used.values().all(|(_, w)| *w == 1.0));

    let total: i64 = p.used.values().map(|(u, _)| u).sum();
    assert_eq!(total, 33);
    // About 10 points an hour, far below the 0.483 of release 1.2.97.
    let r = raw_rate(&p);
    assert!((0.08..=0.22).contains(&r), "{r}");

    // At 3.48 h the start is too young for the printed rate.
    let v = js(&summary(&now, &Weights::default(), 95, &h));
    assert_eq!(v["rate_login_weeks_per_hour"], Value::Null);
    assert_eq!(v["runway_hours"], Value::Null);
}

#[test]
fn rate_parts_carry_each_logins_weight() {
    let w = Weights::from_pairs(&[(RMP, 2.0), (CC, 0.5)]);
    let p = parts_with(&per_login_now(), &per_login_history(), &w).unwrap();
    assert_eq!(p.used[RMP], (15, 2.0));
    assert_eq!(p.used[REC], (11, 1.0));
    assert_eq!(p.used[CC], (7, 0.5));
}

#[test]
fn rate_parts_are_none_without_a_start() {
    let now = snap(T, &[("a", pt(Some(0), 50))], &[]);
    assert!(parts(&now, &[]).is_none());
    let too_old = [entry(T - 7 * HOUR, &[("a", pt(Some(0), 10))])];
    assert!(parts(&now, &too_old).is_none());
}

#[test]
fn the_reset_threshold_is_ten_points() {
    // Pinned: the reference says "more than 10 points".
    assert_eq!(RESET_FALL, 10);
}

#[test]
fn a_fall_of_exactly_ten_is_not_a_reset() {
    // 50, then a stale 40, then 55: one week, 5 points used.
    let cur = snap(T, &[("a", pt(Some(0), 55))], &[]);
    let h = [
        entry(T - 6 * HOUR, &[("a", pt(Some(0), 50))]),
        entry(T - 3 * HOUR, &[("a", pt(Some(0), 40))]),
    ];
    assert_eq!(used(&parts(&cur, &h).unwrap(), "a"), 5);
    // 5 points over 6 h.
    assert_eq!(rate_of(&cur, &h), json!(0.008));
}

#[test]
fn a_fall_of_eleven_is_a_reset() {
    // 50, 60, then 49: a new week; 10 points before it and 55 since.
    let cur = snap(T, &[("a", pt(Some(0), 55))], &[]);
    let h = [
        entry(T - 6 * HOUR, &[("a", pt(Some(0), 50))]),
        entry(T - 4 * HOUR, &[("a", pt(Some(0), 60))]),
        entry(T - 3 * HOUR, &[("a", pt(Some(0), 49))]),
    ];
    assert_eq!(used(&parts(&cur, &h).unwrap(), "a"), 10 + 55);
}

#[test]
fn w_reset_passing_without_a_fall_is_a_reset() {
    // The week reset at T-4h; 3 -> 5 before it and 7 after it, so no fall
    // shows the reset. 2 points before and 7 since.
    let old = T - 4 * HOUR;
    let new = T + 6 * DAY;
    let cur = snap(T, &[("a", wr(7, new))], &[]);
    let h = [
        entry(T - 6 * HOUR, &[("a", wr(3, old))]),
        entry(T - 5 * HOUR, &[("a", wr(5, old))]),
        entry(T - 3 * HOUR, &[("a", wr(7, new))]),
    ];
    assert_eq!(used(&parts(&cur, &h).unwrap(), "a"), 2 + 7);
}

#[test]
fn a_large_fall_with_w_reset_in_future_is_a_reset() {
    // The week can reset early: 90, 100, then 0 and 12 while `w_reset` is
    // still in the future. (100 - 90) + 12 = 22 points.
    let r = T + 3 * DAY;
    let cur = snap(T, &[("a", wr(12, r))], &[]);
    let h = [
        entry(T - 6 * HOUR, &[("a", wr(90, r))]),
        entry(T - 4 * HOUR, &[("a", wr(100, r))]),
        entry(T - 3 * HOUR, &[("a", wr(0, r))]),
    ];
    assert_eq!(used(&parts(&cur, &h).unwrap(), "a"), 22);
}

#[test]
fn a_reset_then_stale_dips_add_nothing() {
    // 90 -> 100, reset at T-4h30m, then 0, 8, a stale 5, 12, a stale 10,
    // and 12 now: (100 - 90) + 12 = 22. Counting each dip as a reset gave
    // 42.
    let old = T - 4 * HOUR - 30 * MIN;
    let new = T + 6 * DAY + 19 * HOUR;
    let cur = snap(T, &[("a", wr(12, new))], &[]);
    let h = [
        entry(T - 6 * HOUR, &[("a", wr(90, old))]),
        entry(T - 5 * HOUR, &[("a", wr(100, old))]),
        entry(T - 4 * HOUR, &[("a", wr(0, new))]),
        entry(T - 3 * HOUR, &[("a", wr(8, new))]),
        entry(T - 150 * MIN, &[("a", wr(5, new))]),
        entry(T - 2 * HOUR, &[("a", wr(12, new))]),
        entry(T - HOUR, &[("a", wr(10, new))]),
    ];
    assert_eq!(used(&parts(&cur, &h).unwrap(), "a"), 22);
    // 22 points over 6 h.
    assert_eq!(rate_of(&cur, &h), json!(0.037));
}

#[test]
fn a_stale_last_reading_before_a_reset_does_not_hide_the_peak() {
    // 20, 60, a stale 55, then a real reset to 4: the week's use before the
    // reset is its peak, 40, not 55 - 20.
    let r = T + 2 * DAY;
    let cur = snap(T, &[("a", wr(4, r))], &[]);
    let h = [
        entry(T - 6 * HOUR, &[("a", wr(20, r))]),
        entry(T - 4 * HOUR, &[("a", wr(60, r))]),
        entry(T - 3 * HOUR, &[("a", wr(55, r))]),
        entry(T - 2 * HOUR, &[("a", wr(2, r))]),
    ];
    assert_eq!(used(&parts(&cur, &h).unwrap(), "a"), 40 + 4);
}

#[test]
fn a_stale_dip_just_before_now_adds_nothing() {
    // The last reading in the window is 50, and this run's figure a stale
    // 45: the week's peak, 50, still counts and the dip is no reset.
    let cur = snap(T, &[("a", pt(Some(0), 45))], &[]);
    let h = [
        entry(T - 6 * HOUR, &[("a", pt(Some(0), 30))]),
        entry(T - HOUR, &[("a", pt(Some(0), 50))]),
    ];
    assert_eq!(used(&parts(&cur, &h).unwrap(), "a"), 20);
}

#[test]
fn a_reading_from_an_ended_week_is_skipped() {
    // At T-3h a reading of 95% says its week reset at T-4h: a figure from
    // a week already over, so it is not used. 40 -> 50 is 10 points; using
    // it gave (95 - 40) + 50 = 105.
    let r = T + 2 * DAY;
    let cur = snap(T, &[("a", wr(50, r))], &[]);
    let h = [
        entry(T - 6 * HOUR, &[("a", wr(40, r))]),
        entry(T - 3 * HOUR, &[("a", wr(95, T - 4 * HOUR))]),
    ];
    assert_eq!(used(&parts(&cur, &h).unwrap(), "a"), 10);
}

#[test]
fn a_stale_passive_current_reading_is_not_counted() {
    // The history holds 50% for this week; this run's passive reading is a
    // cached 38% with the same reset. A 12-point fall would be a reset and
    // add 38; the reading is stale, so only 40 -> 50 counts.
    let r = T + 2 * DAY;
    let h = [
        entry(T - 6 * HOUR, &[("a", wr(40, r))]),
        entry(T - HOUR, &[("a", wr(50, r))]),
    ];
    let stale_now = snap(T, &[("a", passive_pt(38, Some(r)))], &[]);
    assert_eq!(used(&parts(&stale_now, &h).unwrap(), "a"), 10);

    // A passive reading at or above the recorded one is counted.
    let fresh_now = snap(T, &[("a", passive_pt(52, Some(r)))], &[]);
    assert_eq!(used(&parts(&fresh_now, &h).unwrap(), "a"), 12);

    // A direct reading of 38 is believed: a fall of 12 is a reset.
    let direct_now = snap(T, &[("a", wr(38, r))], &[]);
    assert_eq!(used(&parts(&direct_now, &h).unwrap(), "a"), 10 + 38);
}

#[test]
fn the_rate_is_never_negative() {
    // 50 at the start and a stale 45 now: nothing used, never -5.
    let cur = snap(T, &[("a", pt(Some(0), 45)), ("b", pt(Some(0), 20))], &[]);
    let h = [entry(
        T - 6 * HOUR,
        &[("a", pt(Some(0), 50)), ("b", pt(Some(0), 20))],
    )];
    let p = parts(&cur, &h).unwrap();
    assert!(p.used.values().all(|(u, _)| *u >= 0), "{:?}", p.used);
    assert_eq!(used(&p, "a"), 0);
    let s = summary(&cur, &Weights::default(), 95, &h);
    assert_eq!(js(&s)["rate_login_weeks_per_hour"], json!(0.0));
    assert_eq!(js(&s)["runway_hours"], Value::Null);
}

#[test]
fn the_existing_reset_figures_are_unchanged() {
    // The three #1841 cases keep their figures under the new rule: a reset
    // by `w_reset` passing (50 points), a fall of 50 (30 points) and a
    // reset between the last entry and now (15 points), each over 6 h.
    let a = snap(T, &[("a", wr(50, T + 6 * DAY))], &[]);
    let ha = [
        entry(T - 6 * HOUR, &[("a", wr(20, T - 5 * HOUR))]),
        entry(T - 3 * HOUR, &[("a", wr(40, T + 6 * DAY))]),
    ];
    assert_eq!(used(&parts(&a, &ha).unwrap(), "a"), 50);
    let b = snap(T, &[("a", pt(Some(0), 30))], &[]);
    let hb = [
        entry(T - 6 * HOUR, &[("a", pt(Some(0), 60))]),
        entry(T - 3 * HOUR, &[("a", pt(Some(0), 10))]),
    ];
    assert_eq!(used(&parts(&b, &hb).unwrap(), "a"), 30);
    let c = snap(T, &[("a", pt(Some(0), 15))], &[]);
    let hc = [entry(T - 6 * HOUR, &[("a", wr(90, T - HOUR))])];
    assert_eq!(used(&parts(&c, &hc).unwrap(), "a"), 15);
}
