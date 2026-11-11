//! Crusty round 10 on #1831: status must report the hold `policy::decide`
//! applies to ordinary work, at every step that can hold it, not only at the
//! reserve. With every eligible profile session- or weekly-exhausted the
//! pool step 4 reads is empty, and status used to print "ordinary work
//! admitted" while every start waited at step 2 or 3.
//!
//! Each case runs `decide` and `status::view` on the same stored readings
//! and checks that they agree.

use super::config::Settings;
use super::inputs::{budgets, snapshots};
use super::model::{Class, DecisionKind};
use super::policy::decide;
use super::status::{render_table, view};
use super::tests_policy::fixture::inputs;
use super::tests_seed::{NOW, Root};
use serde_json::Value;

const HOUR: i64 = 3600;

/// Seeds for `Root::new`, and the profiles whose readings are made stale.
type Case = (&'static [(&'static str, f64, f64)], &'static [&'static str]);

/// Moves `name`'s stored reading `secs` seconds into the past.
fn age(r: &Root, name: &str, secs: i64) {
    let path = r.root.join(".auto-switch.d").join("usage.json");
    let mut usage: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    usage["accounts"][name]["observed_at"] = (NOW - secs).into();
    std::fs::write(&path, usage.to_string()).unwrap();
}

/// Runs both and asserts they agree; returns the status JSON and table.
fn agree(r: &Root) -> (Value, String) {
    let settings = Settings::on();
    let b = budgets(&snapshots(&r.root), NOW, settings.max_reading_age_secs);
    let d = decide(&inputs(b, Class::Ordinary), NOW);
    let v = view(Some(&r.root), &settings, None, NOW);
    let table = render_table(&v);
    let json = serde_json::to_value(&v).unwrap();
    let held = d.kind != DecisionKind::Admit;
    assert_eq!(json["reserve"]["ordinary_held"], held, "{d:?}\n{json}");
    let reason = if held {
        Value::from(d.reason.code.as_str())
    } else {
        Value::Null
    };
    assert_eq!(json["reserve"]["held_reason"], reason, "{d:?}\n{json}");
    if held {
        assert!(!table.contains("ordinary work admitted"), "{table}");
    } else {
        assert!(table.contains("ordinary work admitted"), "{table}");
    }
    (json, table)
}

/// Crusty's case: the only profile has used its session limit. `decide`
/// waits at step 3; status says so.
#[test]
fn a_session_exhausted_pool_is_reported_as_held() {
    let r = Root::new(&[("alpha", 100.0, 10.0)]);
    let (json, table) = agree(&r);
    assert_eq!(json["reserve"]["ordinary_held"], true, "{json}");
    assert_eq!(
        json["reserve"]["held_reason"], "session-exhausted",
        "{json}"
    );
    assert!(
        table.contains("ordinary work held: session limit used"),
        "{table}"
    );
}

#[test]
fn a_weekly_exhausted_pool_is_reported_as_held() {
    let r = Root::new(&[("alpha", 0.0, 100.0), ("beta", -1.0, 100.0)]);
    let (json, table) = agree(&r);
    assert_eq!(json["reserve"]["held_reason"], "weekly-exhausted", "{json}");
    assert!(
        table.contains("ordinary work held: weekly limit used"),
        "{table}"
    );
}

#[test]
fn a_pool_at_the_reserve_is_reported_as_held_at_the_reserve() {
    let r = Root::new(&[("alpha", 0.0, 95.0)]);
    let (json, table) = agree(&r);
    assert_eq!(json["reserve"]["held_reason"], "reserve", "{json}");
    assert!(
        table.contains("ordinary work held at the reserve"),
        "{table}"
    );
}

/// Step 1 fails open: with no reading `decide` admits, and so does status.
#[test]
fn no_readings_is_reported_as_admitted() {
    let r = Root::new(&[]);
    let (json, _) = agree(&r);
    assert_eq!(json["reserve"]["ordinary_held"], false, "{json}");
    assert_eq!(json["reserve"]["held_reason"], Value::Null, "{json}");
}

/// Further mixes of fresh, stale, session-exhausted, weekly-exhausted and
/// reserve-bound profiles: status and `decide` agree on every one.
#[test]
fn status_and_decide_agree_across_mixed_pools() {
    let cases: &[Case] = &[
        (&[("alpha", 20.0, 30.0)], &[]),
        (&[("alpha", 100.0, 10.0), ("beta", 20.0, 70.0)], &["alpha"]),
        (&[("alpha", 100.0, 10.0), ("beta", 0.0, 100.0)], &[]),
        (&[("alpha", 100.0, 95.0), ("beta", 0.0, 95.0)], &[]),
        (&[("alpha", 0.0, 95.0), ("beta", -1.0, 10.0)], &["beta"]),
        (
            &[("alpha", -1.0, 100.0), ("beta", 100.0, 20.0)],
            &["alpha", "beta"],
        ),
    ];
    for (seeds, stale) in cases {
        let r = Root::new(seeds);
        for name in *stale {
            age(&r, name, 2 * HOUR);
        }
        agree(&r);
    }
}
