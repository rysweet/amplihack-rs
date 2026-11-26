//! Issue #3181: the highest reading kept for each login and window.
//!
//! The contract they pin:
//!
//! - `HighWater` (serde transparent map of profile name to peaks) with
//!   `raise(&mut self, name, &Probed, now)`, `floor(&self, name, &Probed,
//!   now) -> Probed`, `below(&self, name, &Probed, now) -> bool` and
//!   `is_empty()`.
//! - `high_water::validate_on_load(HighWater, now) -> HighWater`, and
//!   `validate`, which also returns the counts its warning reports.
//! - Serialised form: `{name: {"session": {"used", "resets_at"}, "week":
//!   {...}}}`, nothing else.

use super::super::decide::{Probed, Reading, Win, Windows};
use super::super::ledger::high_water::{Dropped, HighWater, validate, validate_on_load};
use super::super::tests_issue_1870::rd;
use serde_json::{Value, json};

const NOW: i64 = 1_760_000_000;
const R: i64 = NOW + 7200;

/// `rd` with both windows resetting at `r` (`None`: no reset time).
fn at(s: f64, w: f64, r: Option<i64>) -> Probed {
    let mut p = rd(s, w, None);
    p.windows = Windows {
        session: Some(Win {
            used: s,
            resets_at: r,
        }),
        week: Some(Win {
            used: w,
            resets_at: r.map(|x| x.saturating_add(86_400)),
        }),
        no_session_limit: false,
    };
    p
}

fn figures(p: &Probed) -> (f64, f64) {
    match p.reading {
        Reading::Measured { session, week } => (session, week),
        ref other => panic!("not a measured reading: {other:?}"),
    }
}

fn kept(h: &HighWater) -> Value {
    serde_json::to_value(h).unwrap()
}

fn peak(h: &HighWater, name: &str) -> Option<f64> {
    kept(h)[name]["session"]["used"].as_f64()
}

#[test]
fn a_lower_reading_in_the_same_window_is_raised_to_the_peak() {
    let mut h = HighWater::default();
    h.raise("a", &at(91.0, 14.0, Some(R)), NOW);
    let got = h.floor("a", &at(70.0, 11.0, Some(R)), NOW + 60);
    assert_eq!(figures(&got), (91.0, 14.0));
    assert!(h.below("a", &at(70.0, 11.0, Some(R)), NOW + 60));
    assert!(!h.below("a", &at(92.0, 14.0, Some(R)), NOW + 60));
}

#[test]
fn a_higher_reading_replaces_the_peak() {
    let mut h = HighWater::default();
    h.raise("a", &at(60.0, 10.0, Some(R)), NOW);
    h.raise("a", &at(75.0, 10.0, Some(R)), NOW);
    h.raise("a", &at(70.0, 10.0, Some(R)), NOW);
    assert_eq!(peak(&h, "a"), Some(75.0));
}

#[test]
fn a_later_window_replaces_the_peak_even_when_lower() {
    let mut h = HighWater::default();
    h.raise("a", &at(91.0, 14.0, Some(R)), NOW);
    let later = R + 5 * 3600;
    h.raise("a", &at(3.0, 14.0, Some(later)), R - 10);
    assert_eq!(peak(&h, "a"), Some(3.0));
    assert_eq!(kept(&h)["a"]["session"]["resets_at"], json!(later));
}

#[test]
fn nothing_is_floored_once_the_window_has_reset() {
    let mut h = HighWater::default();
    h.raise("a", &at(91.0, 14.0, Some(R)), NOW);
    let got = h.floor("a", &at(5.0, 14.0, Some(R + 5 * 3600)), R + 1);
    assert_eq!(figures(&got), (5.0, 14.0));
    let same_reset = h.floor("a", &at(5.0, 14.0, Some(R)), R + 1);
    assert_eq!(figures(&same_reset).0, 5.0, "reset has passed");
}

#[test]
fn a_reading_without_a_reset_time_never_raises_or_lowers_the_peak() {
    let mut h = HighWater::default();
    h.raise("a", &at(91.0, 14.0, Some(R)), NOW);
    h.raise("a", &at(99.0, 14.0, None), NOW);
    assert_eq!(peak(&h, "a"), Some(91.0));
    // It cannot show room for a login whose peak is high.
    assert_eq!(figures(&h.floor("a", &at(40.0, 14.0, None), NOW)).0, 91.0);
    assert!(h.below("a", &at(40.0, 14.0, None), NOW));
}

#[test]
fn a_reading_with_a_passed_reset_is_not_kept() {
    let mut h = HighWater::default();
    h.raise("a", &at(91.0, 14.0, Some(NOW - 5)), NOW);
    assert!(h.is_empty());
}

#[test]
fn figures_are_clamped_and_non_finite_ones_are_rejected() {
    let mut h = HighWater::default();
    h.raise("a", &at(120.0, 14.0, Some(R)), NOW);
    assert_eq!(peak(&h, "a"), Some(100.0));
    let mut bad = HighWater::default();
    bad.raise("b", &at(f64::NAN, 14.0, Some(R)), NOW);
    bad.raise("b", &at(f64::INFINITY, 14.0, Some(R)), NOW);
    assert_eq!(peak(&bad, "b"), None);
    let floored = bad.floor("b", &at(f64::NAN, 1.0, Some(R)), NOW);
    assert!(!figures(&floored).0.is_nan());
}

#[test]
fn extreme_times_do_not_panic() {
    let mut h = HighWater::default();
    for t in [i64::MAX, i64::MIN, 0] {
        h.raise("a", &at(50.0, 5.0, Some(t)), NOW);
        h.raise("a", &at(50.0, 5.0, Some(R)), t);
        let _ = h.floor("a", &at(10.0, 5.0, Some(t)), t);
        let _ = h.below("a", &at(10.0, 5.0, Some(R)), t);
    }
}

#[test]
fn week_peaks_are_kept_apart_from_session_peaks() {
    let mut h = HighWater::default();
    h.raise("a", &at(10.0, 80.0, Some(R)), NOW);
    let got = h.floor("a", &at(20.0, 30.0, Some(R)), NOW);
    assert_eq!(figures(&got), (20.0, 80.0));
}

#[test]
fn an_exhausted_reading_is_left_as_it_was_read() {
    let mut h = HighWater::default();
    h.raise("a", &at(91.0, 14.0, Some(R)), NOW);
    let mut p = at(100.0, 100.0, Some(R));
    p.reading = Reading::EXHAUSTED;
    assert_eq!(h.floor("a", &p, NOW).reading, Reading::EXHAUSTED);
}

#[test]
fn only_documented_keys_are_serialised() {
    let mut h = HighWater::default();
    h.raise("a", &at(91.0, 14.0, Some(R)), NOW);
    let v = kept(&h);
    let obj = v.as_object().unwrap();
    assert_eq!(obj.keys().collect::<Vec<_>>(), ["a"]);
    let mut keys: Vec<_> = v["a"].as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["session", "week"]);
    let mut inner: Vec<_> = v["a"]["session"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    inner.sort();
    assert_eq!(inner, ["resets_at", "used"]);
    assert!(kept(&HighWater::default()).as_object().unwrap().is_empty());
}

fn load(v: Value) -> HighWater {
    validate_on_load(serde_json::from_value(v).unwrap(), NOW)
}

#[test]
fn load_drops_bad_entries_and_keeps_good_ones() {
    let ok = json!({"session": {"used": 91.0, "resets_at": R}});
    let h = load(json!({
        "good": ok,
        "../x": ok,
        "a/b": ok,
        "": ok,
        "nan": {"session": {"used": -1.0, "resets_at": R}},
        "over": {"session": {"used": 101.0, "resets_at": R}},
        "passed": {"session": {"used": 50.0, "resets_at": NOW - 1}},
        "far": {"session": {"used": 50.0, "resets_at": NOW + 9 * 86_400}},
        "empty": {},
    }));
    let v = kept(&h);
    assert_eq!(v.as_object().unwrap().keys().collect::<Vec<_>>(), ["good"]);
}

#[test]
fn load_caps_the_entries_at_sixteen_keeping_the_earliest_resets() {
    let mut m = serde_json::Map::new();
    for i in 0..20 {
        m.insert(
            format!("p{i:02}"),
            json!({"session": {"used": 50.0, "resets_at": R + i}}),
        );
    }
    let v = kept(&load(Value::Object(m)));
    let keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys.len(), 16);
    assert!(keys.contains(&"p00".to_string()) && !keys.contains(&"p19".to_string()));
}

#[test]
fn a_ledger_value_that_is_not_a_map_does_not_fail_the_load() {
    // A bad shape is dropped by the caller's serde default; an unknown
    // field inside an entry is ignored, not fatal.
    let h = load(json!({"a": {"session": {"used": 5.0, "resets_at": R, "extra": 1}}}));
    assert_eq!(peak(&h, "a"), Some(5.0));
}

#[test]
fn load_keeps_only_valid_entries_and_counts_the_rest_by_reason() {
    let ok = json!({"used": 40.0, "resets_at": R});
    let bad = json!({"used": 101.0, "resets_at": R});
    let mut m = serde_json::Map::new();
    m.insert("good".into(), json!({"session": ok}));
    m.insert("half".into(), json!({"session": ok, "week": bad}));
    m.insert("../x".into(), json!({"session": ok}));
    m.insert("over".into(), json!({"session": bad}));
    m.insert("empty".into(), json!({}));
    m.insert("garbled".into(), json!("not a map of peaks"));
    for i in 0..16 {
        m.insert(
            format!("p{i:02}"),
            json!({"session": {"used": 1.0, "resets_at": R - 1}}),
        );
    }
    let h: HighWater = serde_json::from_value(Value::Object(m)).unwrap();
    let (h, d) = validate(h, NOW);
    assert_eq!(
        d,
        Dropped {
            bad_name: 1,
            bad_peak: 2,
            empty: 2,
            capped: 2,
        }
    );
    // The two with the latest resets, "good" and "half", go to the cap.
    let v = kept(&h);
    let keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys.len(), 16);
    assert!(keys.iter().all(|k| k.starts_with('p')));
}

#[test]
fn a_partly_bad_entry_keeps_its_good_peak() {
    let h = load(json!({"half": {
        "session": {"used": 40.0, "resets_at": R},
        "week": {"used": f64::MAX, "resets_at": NOW - 1},
    }}));
    assert_eq!(peak(&h, "half"), Some(40.0));
    assert_eq!(kept(&h)["half"].get("week"), None);
}
