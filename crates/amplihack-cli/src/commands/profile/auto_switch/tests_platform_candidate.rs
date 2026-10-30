//! Issue #1700: auto-switch and the API-key tier. Pure: profiles, readings,
//! the overflow order and the gate.
//!
//! Contract:
//! - `OverflowPool` gains `platform: Option<&ProfileName>`: the API-key
//!   profile, which `tiers()` also put last in `order`. Its block, when
//!   active, is in `blocked` like a spend block.
//! - The API-key profile is never probed: it has no reading, and that never
//!   makes it plan capacity (it is in `order`, so never in the plan pool).
//! - Onto it only as the last overflow entry, when the gate allows and no
//!   credit profile can be used.
//! - A session on it moves, ignoring holds, to a plan profile with capacity
//!   by the gate's test, else to the first usable credit profile; else it
//!   stays (`None`).
//! - A session on a credit profile never moves to the API key while it can
//!   stay (no spend block).

use super::candidate::{self, OverflowPool, Pool};
use super::usage::{Reading, ReadingSource, Window};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::Gate;
use std::collections::BTreeMap;

const NOW: i64 = 1_760_000_000;
const GATE: Gate = Gate {
    wait: 1800,
    unknown_reset: false,
    session_at: 85,
    week_at: 90,
};

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn five_hour(five: f64, resets_in: i64) -> Reading {
    Reading {
        five_hour: Some(Window {
            used_percentage: five,
            resets_at: NOW + resets_in,
        }),
        seven_day: Some(Window {
            used_percentage: 10.0,
            resets_at: NOW + 86_400,
        }),
        observed_at: NOW - 10,
        source: ReadingSource::Statusline,
    }
}

/// Plan: `work`, `spare`. Credits: `cred`. API key: `console` (no reading).
struct Case {
    profiles: Vec<ProfileName>,
    readings: BTreeMap<String, Reading>,
    order: Vec<ProfileName>,
    blocked: Vec<String>,
    console: ProfileName,
}

impl Case {
    /// Every plan profile exhausted, resetting beyond the wait.
    fn all_plan_out() -> Self {
        let mut readings = BTreeMap::new();
        readings.insert("work".into(), five_hour(95.0, 4 * 3600));
        readings.insert("spare".into(), five_hour(99.0, 4 * 3600));
        readings.insert("cred".into(), five_hour(100.0, 4 * 3600));
        Self {
            profiles: ["work", "spare", "cred", "console"]
                .iter()
                .map(|p| n(p))
                .collect(),
            readings,
            order: vec![n("cred"), n("console")],
            blocked: Vec::new(),
            console: n("console"),
        }
    }

    fn with(mut self, name: &str, r: Reading) -> Self {
        self.readings.insert(name.into(), r);
        self
    }

    fn block(mut self, name: &str) -> Self {
        self.blocked.push(name.into());
        self
    }

    fn no_credits(mut self) -> Self {
        self.order.retain(|o| o.as_str() != "cred");
        self.profiles.retain(|o| o.as_str() != "cred");
        self
    }

    fn pick(&self, current: &str) -> Option<String> {
        let exclude = Vec::new();
        let pool = Pool {
            profiles: &self.profiles,
            readings: &self.readings,
            threshold: 90,
            exclude: &exclude,
            now: NOW,
        };
        let ov = OverflowPool {
            order: &self.order,
            blocked: &self.blocked,
            gate: Some(GATE),
            platform: Some(&self.console),
        };
        candidate::select_with_overflow(&pool, &ov, current, |_| true).map(|p| p.to_string())
    }
}

#[test]
fn credits_come_before_the_api_key() {
    assert_eq!(Case::all_plan_out().pick("work").as_deref(), Some("cred"));
}

#[test]
fn the_api_key_follows_a_blocked_credit_profile() {
    let c = Case::all_plan_out().block("cred");
    assert_eq!(c.pick("work").as_deref(), Some("console"));
    assert_eq!(
        Case::all_plan_out().no_credits().pick("work").as_deref(),
        Some("console")
    );
}

#[test]
fn a_blocked_api_key_is_never_chosen() {
    let c = Case::all_plan_out().block("cred").block("console");
    assert_ne!(c.pick("work").as_deref(), Some("console"));
}

#[test]
fn the_api_key_needs_the_gate() {
    // A plan profile resets within the wait.
    let c = Case::all_plan_out()
        .no_credits()
        .with("spare", five_hour(99.0, 600));
    assert_ne!(c.pick("work").as_deref(), Some("console"));
}

#[test]
fn a_plan_profile_with_capacity_wins_and_the_api_keys_missing_reading_is_not_capacity() {
    let c = Case::all_plan_out().with("spare", five_hour(40.0, 4 * 3600));
    assert_eq!(c.pick("work").as_deref(), Some("spare"));
    // No plan capacity: console's absent reading must not count as one,
    // so it is reached only through the overflow order, after credits.
    assert_eq!(Case::all_plan_out().pick("work").as_deref(), Some("cred"));
}

#[test]
fn a_session_on_the_api_key_moves_to_a_plan_profile_with_capacity() {
    let c = Case::all_plan_out().with("spare", five_hour(84.0, 4 * 3600));
    assert_eq!(c.pick("console").as_deref(), Some("spare"));
}

#[test]
fn a_session_on_the_api_key_moves_to_credits_when_no_plan_has_capacity() {
    assert_eq!(
        Case::all_plan_out().pick("console").as_deref(),
        Some("cred")
    );
}

#[test]
fn a_session_on_the_api_key_stays_when_nothing_else_can_be_used() {
    assert_eq!(Case::all_plan_out().block("cred").pick("console"), None);
    assert_eq!(Case::all_plan_out().no_credits().pick("console"), None);
}

#[test]
fn a_session_on_credits_never_moves_to_the_api_key_while_it_can_stay() {
    assert_eq!(Case::all_plan_out().pick("cred"), None);
}

#[test]
fn a_session_on_spend_blocked_credits_moves_to_the_api_key() {
    let c = Case::all_plan_out().block("cred");
    assert_eq!(c.pick("cred").as_deref(), Some("console"));
}
