//! Candidate selection: a pure function of profiles, readings, threshold,
//! exclusions and an eligibility filter (issue #1569).

use super::candidate::{self, Pool};
use super::usage::{Reading, ReadingSource, Window};
use crate::commands::profile::name::ProfileName;
use std::collections::BTreeMap;

const NOW: i64 = 1_760_000_000;

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn w(pct: f64, resets_in: i64) -> Option<Window> {
    Some(Window {
        used_percentage: pct,
        resets_at: NOW + resets_in,
    })
}

fn reading(five: f64, seven: f64) -> Reading {
    Reading {
        five_hour: w(five, 3600),
        seven_day: w(seven, 86_400),
        observed_at: NOW - 10,
        source: ReadingSource::Statusline,
    }
}

struct Case {
    profiles: Vec<ProfileName>,
    readings: BTreeMap<String, Reading>,
    threshold: u8,
    exclude: Vec<ProfileName>,
}

impl Case {
    fn new(profiles: &[&str]) -> Self {
        Self {
            profiles: profiles.iter().map(|p| n(p)).collect(),
            readings: BTreeMap::new(),
            threshold: 90,
            exclude: Vec::new(),
        }
    }

    fn with(mut self, name: &str, r: Reading) -> Self {
        self.readings.insert(name.to_string(), r);
        self
    }

    fn pool(&self) -> Pool<'_> {
        Pool {
            profiles: &self.profiles,
            readings: &self.readings,
            threshold: self.threshold,
            exclude: &self.exclude,
            now: NOW,
        }
    }

    fn pick(&self, current: &str) -> Option<String> {
        candidate::select(&self.pool(), current, |_| true).map(|p| p.to_string())
    }
}

#[test]
fn expired_window_counts_as_zero() {
    let expired = Window {
        used_percentage: 100.0,
        resets_at: NOW,
    };
    assert_eq!(
        candidate::adjusted(Some(&expired), NOW),
        0.0,
        "at reset time"
    );
    let past = Window {
        used_percentage: 100.0,
        resets_at: NOW - 1,
    };
    assert_eq!(candidate::adjusted(Some(&past), NOW), 0.0);
    let live = Window {
        used_percentage: 42.5,
        resets_at: NOW + 1,
    };
    assert_eq!(candidate::adjusted(Some(&live), NOW), 42.5);
    assert_eq!(candidate::adjusted(None, NOW), 0.0);
}

#[test]
fn exhausted_when_either_window_reaches_threshold() {
    assert!(candidate::is_exhausted(Some(&reading(90.0, 0.0)), 90, NOW));
    assert!(candidate::is_exhausted(Some(&reading(0.0, 90.0)), 90, NOW));
    assert!(!candidate::is_exhausted(
        Some(&reading(89.9, 89.9)),
        90,
        NOW
    ));
    assert!(candidate::is_exhausted(Some(&reading(50.0, 0.0)), 50, NOW));
    assert!(
        !candidate::is_exhausted(None, 90, NOW),
        "no reading is not exhausted"
    );
}

#[test]
fn exhausted_window_that_has_reset_is_not_exhausted() {
    let r = Reading {
        five_hour: w(100.0, -5),
        seven_day: w(10.0, 86_400),
        observed_at: NOW - 7200,
        source: ReadingSource::UsageLimitFailure,
    };
    assert!(!candidate::is_exhausted(Some(&r), 90, NOW));
}

#[test]
fn picks_the_most_quota_left_by_worst_window() {
    let c = Case::new(&["a", "b", "c", "d"])
        .with("b", reading(10.0, 80.0)) // worst 80
        .with("c", reading(50.0, 20.0)) // worst 50
        .with("d", reading(60.0, 60.0)); // worst 60
    assert_eq!(c.pick("a").as_deref(), Some("c"));
}

#[test]
fn ties_break_by_name() {
    let c = Case::new(&["cur", "zed", "amy"])
        .with("zed", reading(30.0, 0.0))
        .with("amy", reading(30.0, 0.0));
    assert_eq!(c.pick("cur").as_deref(), Some("amy"));
}

#[test]
fn profiles_with_no_reading_come_after_known_ones_by_name() {
    let c =
        Case::new(&["cur", "b-unknown", "a-unknown", "known"]).with("known", reading(85.0, 0.0));
    assert_eq!(c.pick("cur").as_deref(), Some("known"));
    let c = Case::new(&["cur", "b-unknown", "a-unknown"]);
    assert_eq!(c.pick("cur").as_deref(), Some("a-unknown"));
}

#[test]
fn never_picks_current_exhausted_excluded_or_ineligible() {
    let mut c = Case::new(&["cur", "full", "skip", "busy", "ok"])
        .with("cur", reading(1.0, 1.0))
        .with("full", reading(95.0, 0.0))
        .with("skip", reading(1.0, 1.0))
        .with("busy", reading(2.0, 2.0))
        .with("ok", reading(70.0, 0.0));
    c.exclude = vec![n("skip")];
    let picked = candidate::select(&c.pool(), "cur", |p| p.as_str() != "busy");
    assert_eq!(picked.map(|p| p.to_string()).as_deref(), Some("ok"));
}

#[test]
fn no_eligible_candidate_is_none() {
    let c = Case::new(&["cur", "x", "y"])
        .with("x", reading(99.0, 0.0))
        .with("y", reading(0.0, 95.0));
    assert_eq!(c.pick("cur"), None);
    assert_eq!(Case::new(&["only"]).pick("only"), None);
}

#[test]
fn excluded_current_account_can_still_switch_away() {
    let mut c = Case::new(&["cur", "next"]).with("cur", reading(99.0, 0.0));
    c.exclude = vec![n("cur")];
    assert_eq!(c.pick("cur").as_deref(), Some("next"));
}

#[test]
fn threshold_changes_who_is_exhausted() {
    let mut c = Case::new(&["cur", "a", "b"])
        .with("a", reading(60.0, 0.0))
        .with("b", reading(75.0, 0.0));
    c.threshold = 70;
    assert_eq!(c.pick("cur").as_deref(), Some("a"));
    c.threshold = 50;
    assert_eq!(c.pick("cur"), None);
}

#[test]
fn reset_window_ranks_as_full_quota() {
    let c = Case::new(&["cur", "reset", "fresh"])
        .with(
            "reset",
            Reading {
                five_hour: w(100.0, -1),
                seven_day: w(5.0, 86_400),
                observed_at: NOW - 9000,
                source: ReadingSource::Statusline,
            },
        )
        .with("fresh", reading(20.0, 20.0));
    assert_eq!(c.pick("cur").as_deref(), Some("reset"));
}

#[test]
fn eligibility_filter_is_only_asked_about_other_profiles() {
    let c = Case::new(&["cur", "a"]);
    let picked = candidate::select(&c.pool(), "cur", |p| {
        assert_ne!(
            p.as_str(),
            "cur",
            "current account must be skipped before the filter"
        );
        true
    });
    assert_eq!(picked.map(|p| p.to_string()).as_deref(), Some("a"));
}

// --- Overflow profiles (issue #1606) ---------------------------------------
//
// Auto-switch follows the rotator's rule, through the same `Gate`:
// - never onto an overflow profile while any plan profile reads under the
//   threshold, eligible for this session or not;
// - onto the first usable overflow profile (tier, then file order) when every
//   plan profile is exhausted and the earliest plan reset is beyond the wait;
// - back to a plan profile as soon as one has capacity, whether or not the
//   overflow profile is exhausted; otherwise it stays on the overflow profile;
// - an overflow profile's own plan reading never counts: only a spend block
//   makes it unusable (issue #1694).

use super::candidate::OverflowPool;
use crate::commands::profile::overflow::Gate;

const GATE: Gate = Gate {
    wait: 1800,
    unknown_reset: false,
    session_at: 85,
    week_at: 90,
};

/// A reading whose five-hour window is at `five`% and resets in `resets_in`.
fn five_hour(five: f64, resets_in: i64) -> Reading {
    Reading {
        five_hour: w(five, resets_in),
        seven_day: w(10.0, 86_400),
        observed_at: NOW - 10,
        source: ReadingSource::Statusline,
    }
}

struct Over {
    order: Vec<ProfileName>,
    blocked: Vec<String>,
    gate: Option<Gate>,
}

#[test]
fn issue_2136_room_fallback_uses_its_own_eligible_same_organisation_pool() {
    let c = Case::new(&["work", "same", "other"])
        .with("work", reading(100.0, 100.0))
        .with("same", reading(95.0, 95.0))
        .with("other", reading(91.0, 91.0));
    let o = Over::new(&[]);
    let picked = candidate::select_restricted(
        &c.pool(),
        &o.pool(),
        "work",
        |_| false,
        |p| p.as_str() == "same",
    );
    assert_eq!(picked.map(ProfileName::as_str), Some("same"));
    assert!(
        candidate::select_restricted(&c.pool(), &o.pool(), "work", |_| false, |_| false).is_none()
    );
}

impl Over {
    fn new(order: &[&str]) -> Self {
        Self {
            order: order.iter().map(|o| n(o)).collect(),
            blocked: Vec::new(),
            gate: Some(GATE),
        }
    }

    fn pool(&self) -> OverflowPool<'_> {
        OverflowPool {
            order: &self.order,
            blocked: &self.blocked,
            gate: self.gate,
            platform: None,
            not_plan: &[],
            no_login: &[],
            kinds_unknown: false,
        }
    }

    fn pick(&self, c: &Case, current: &str) -> Option<String> {
        self.pick_if(c, current, |_| true)
    }

    fn pick_if(
        &self,
        c: &Case,
        current: &str,
        ok: impl Fn(&ProfileName) -> bool,
    ) -> Option<String> {
        candidate::select_with_overflow(&c.pool(), &self.pool(), current, ok).map(|p| p.to_string())
    }
}

/// work (live) and spare exhausted, spare resetting in `spare_reset`; cred
/// is overflow with nothing used.
fn all_plan_out(spare_reset: i64) -> Case {
    Case::new(&["work", "spare", "cred"])
        .with("work", five_hour(100.0, 4 * 3600))
        .with("spare", five_hour(100.0, spare_reset))
        .with("cred", reading(0.0, 0.0))
}

#[test]
fn no_move_onto_overflow_while_a_plan_profile_has_capacity() {
    let o = Over::new(&["cred"]);
    let c = all_plan_out(4 * 3600).with("spare", reading(40.0, 40.0));
    assert_eq!(o.pick(&c, "work").as_deref(), Some("spare"));
    // Even when that plan profile cannot take this session (leased, no
    // login file): it is not exhausted, so no money is spent.
    assert_eq!(o.pick_if(&c, "work", |p| p.as_str() != "spare"), None);
}

#[test]
fn moves_onto_overflow_when_every_plan_profile_is_exhausted_beyond_the_wait() {
    let o = Over::new(&["cred"]);
    let c = all_plan_out(1801);
    assert_eq!(o.pick(&c, "work").as_deref(), Some("cred"));
}

#[test]
fn a_plan_reset_inside_the_wait_keeps_overflow_unused() {
    let o = Over::new(&["cred"]);
    assert_eq!(
        o.pick(&all_plan_out(1800), "work"),
        None,
        "exactly at the wait"
    );
    assert_eq!(o.pick(&all_plan_out(60), "work"), None);
    // The live profile's own reset counts too.
    let c = all_plan_out(4 * 3600).with("work", five_hour(100.0, 600));
    assert_eq!(o.pick(&c, "work"), None);
}

#[test]
fn moves_back_off_overflow_once_a_plan_profile_recovers() {
    let o = Over::new(&["cred"]);
    // On cred, no plan capacity: stay.
    let c = all_plan_out(4 * 3600);
    assert_eq!(o.pick(&c, "cred"), None, "stays on the overflow profile");
    // spare recovers: move back, though cred is far from exhausted.
    let c = c.with("spare", reading(30.0, 30.0));
    assert_eq!(o.pick(&c, "cred").as_deref(), Some("spare"));
    // A reset that passed counts as recovered as well.
    let c = all_plan_out(4 * 3600).with("spare", five_hour(99.0, 0));
    assert_eq!(o.pick(&c, "cred").as_deref(), Some("spare"));
}

/// A credit account at its plan limit: the account issue #1694 is about.
/// Its own plan reading does not make it unusable.
fn credit_used_up() -> Reading {
    reading(100.0, 100.0)
}

#[test]
fn a_credit_profile_at_its_plan_limit_is_moved_onto() {
    let o = Over::new(&["cred"]);
    let c = all_plan_out(4 * 3600).with("cred", credit_used_up());
    assert_eq!(o.pick(&c, "work").as_deref(), Some("cred"));
    // A plan reset inside the wait still keeps it unused.
    let c = all_plan_out(60).with("cred", credit_used_up());
    assert_eq!(o.pick(&c, "work"), None);
}

#[test]
fn a_session_on_a_credit_profile_at_its_plan_limit_stays() {
    let mut o = Over::new(&["cred", "cred2"]);
    let mut c = all_plan_out(4 * 3600)
        .with("cred", credit_used_up())
        .with("cred2", reading(0.0, 0.0));
    c.profiles.push(n("cred2"));
    assert_eq!(o.pick(&c, "cred"), None, "no move to the next overflow");
    // Without a second overflow profile, no move to a plan profile either.
    o.order = vec![n("cred")];
    assert_eq!(
        o.pick(
            &all_plan_out(4 * 3600).with("cred", credit_used_up()),
            "cred"
        ),
        None
    );
}

#[test]
fn only_a_spend_block_moves_a_session_to_the_next_overflow_profile() {
    let mut o = Over::new(&["cred", "cred2"]);
    let mut c = all_plan_out(4 * 3600)
        .with("cred", credit_used_up())
        .with("cred2", credit_used_up());
    c.profiles.push(n("cred2"));
    assert_eq!(o.pick(&c, "cred"), None);
    o.blocked = vec!["cred".into()];
    assert_eq!(o.pick(&c, "cred").as_deref(), Some("cred2"));
    assert_eq!(
        o.pick(&c, "work").as_deref(),
        Some("cred2"),
        "never onto a blocked one"
    );
}

#[test]
fn a_plan_exhausted_tier_1_profile_beats_a_fresh_tier_2_profile() {
    // Tier order is the order given (`Config::overflow_order`): tier 1 credits
    // first, then a tier 2 API key that reads 0%.
    let o = Over::new(&["credits", "api-key"]);
    let c = Case::new(&["work", "credits", "api-key"])
        .with("work", five_hour(100.0, 4 * 3600))
        .with("credits", credit_used_up())
        .with("api-key", reading(0.0, 0.0));
    assert_eq!(o.pick(&c, "work").as_deref(), Some("credits"));
    assert_eq!(o.pick(&c, "credits"), None, "stays on tier 1");
}

#[test]
fn overflow_profiles_are_tried_in_the_order_given() {
    // The order passed in is tier order (`Config::overflow_order`).
    let o = Over::new(&["low", "high"]);
    let c = Case::new(&["work", "high", "low"])
        .with("work", five_hour(100.0, 4 * 3600))
        .with("high", reading(0.0, 0.0))
        .with("low", reading(0.0, 0.0));
    assert_eq!(o.pick(&c, "work").as_deref(), Some("low"));
}

#[test]
fn a_closed_gate_or_an_excluded_overflow_profile_spends_nothing() {
    let mut o = Over::new(&["cred"]);
    o.gate = None;
    assert_eq!(
        o.pick(&all_plan_out(4 * 3600), "work"),
        None,
        "gate unreadable"
    );
    let o = Over::new(&["cred"]);
    let mut c = all_plan_out(4 * 3600);
    c.exclude = vec![n("cred")];
    assert_eq!(o.pick(&c, "work"), None, "excluded");
}

#[test]
fn an_overflow_profile_is_never_picked_as_a_plan_profile() {
    // cred has the most room of all, but spare still has capacity (below
    // the gate's 85% session and 90% week).
    let o = Over::new(&["cred"]);
    let c = all_plan_out(4 * 3600).with("spare", reading(80.0, 85.0));
    assert_eq!(o.pick(&c, "work").as_deref(), Some("spare"));
}

/// The rerun path (`nested_candidate`, used by `substitute` and the agent
/// step's rerun) with the gate from the environment and no rotate ledger.
#[test]
fn the_rerun_candidate_follows_the_overflow_rule() {
    use super::config::Config;
    use super::fsx;
    use super::rerun::nested_candidate;
    use super::state::{CREDS, Ledger, State};
    use crate::commands::profile::overflow::OverflowEntry;
    use crate::test_support::{EnvGuard, env_lock};
    use std::os::unix::fs::PermissionsExt;

    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _env = EnvGuard::unset([
        "AMPLIHACK_ROTATE_OVERFLOW_WAIT",
        "AMPLIHACK_ROTATE_OVERFLOW_UNKNOWN_RESET",
        "AMPLIHACK_ROTATE_SESSION_AT",
        "AMPLIHACK_ROTATE_WEEK_AT",
    ]);
    let tmp = tempfile::tempdir().unwrap();
    std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    for p in ["work", "spare", "cred"] {
        let dir = tmp.path().join(p);
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join(CREDS), "x").unwrap();
    }
    let state = State::new(tmp.path()).unwrap();
    let profiles = vec![n("work"), n("spare"), n("cred")];
    let now = fsx::now();
    let live = |five: f64, resets_in: i64| Reading {
        five_hour: Some(Window {
            used_percentage: five,
            resets_at: now + resets_in,
        }),
        seven_day: Some(Window {
            used_percentage: 10.0,
            resets_at: now + 86_400,
        }),
        observed_at: now - 10,
        source: ReadingSource::Statusline,
    };
    let cfg = Config {
        enabled: true,
        threshold: 90,
        exclude: Vec::new(),
        overflow: vec![OverflowEntry::new(n("cred"), 1)],
        kind: Default::default(),
    };
    let mut readings = BTreeMap::new();
    readings.insert("work".to_string(), live(100.0, 4 * 3600));
    readings.insert("spare".to_string(), live(40.0, 4 * 3600));
    // cred is a credit account at its plan limit: still usable.
    readings.insert("cred".to_string(), live(100.0, 4 * 3600));
    let pick = |readings: &BTreeMap<String, Reading>, current: &str| {
        let skip = [current.to_string()];
        nested_candidate(
            &state,
            &Ledger::default(),
            &profiles,
            readings,
            &cfg,
            current,
            &skip,
        )
        .map(|p| p.profile.to_string())
    };
    assert_eq!(
        pick(&readings, "work").as_deref(),
        Some("spare"),
        "plan first"
    );
    readings.insert("spare".to_string(), live(100.0, 4 * 3600));
    assert_eq!(
        pick(&readings, "work").as_deref(),
        Some("cred"),
        "every plan profile out for longer than the wait"
    );
    readings.insert("spare".to_string(), live(100.0, 600));
    assert_eq!(
        pick(&readings, "work"),
        None,
        "spare is back within the wait"
    );
    readings.insert("spare".to_string(), live(100.0, 4 * 3600));
    assert_eq!(pick(&readings, "cred"), None, "stays on cred");
    readings.insert("spare".to_string(), live(20.0, 4 * 3600));
    assert_eq!(
        pick(&readings, "cred").as_deref(),
        Some("spare"),
        "back to the plan profile once it recovers"
    );
}

// --- Issue #1818: readings with no session limit ----------------------------

/// A reading with a week window and no five-hour window.
fn weekly_only(seven: f64) -> Reading {
    Reading {
        five_hour: None,
        seven_day: w(seven, 86_400),
        observed_at: NOW - 10,
        source: ReadingSource::Statusline,
    }
}

#[test]
fn a_weekly_only_reading_is_judged_on_its_week() {
    assert!(!candidate::is_exhausted(Some(&weekly_only(0.0)), 90, NOW));
    assert!(!candidate::is_exhausted(Some(&weekly_only(89.9)), 90, NOW));
    assert!(candidate::is_exhausted(Some(&weekly_only(90.0)), 90, NOW));
    assert!(candidate::is_exhausted(Some(&weekly_only(100.0)), 90, NOW));
}

#[test]
fn a_weekly_only_profile_is_picked_when_the_rest_are_session_exhausted() {
    let c = Case::new(&["cur", "a", "b", "free"])
        .with("cur", reading(100.0, 10.0))
        .with("a", reading(95.0, 5.0))
        .with("b", reading(100.0, 0.0))
        .with("free", weekly_only(40.0));
    assert_eq!(c.pick("cur").as_deref(), Some("free"));
}

#[test]
fn weekly_only_profiles_rank_by_their_week() {
    let c = Case::new(&["cur", "x", "y", "z"])
        .with("cur", reading(100.0, 0.0))
        .with("x", weekly_only(60.0))
        .with("y", reading(30.0, 30.0))
        .with("z", weekly_only(20.0));
    assert_eq!(c.pick("cur").as_deref(), Some("z"));
    let c = Case::new(&["cur", "x", "y"])
        .with("cur", reading(100.0, 0.0))
        .with("x", weekly_only(60.0))
        .with("y", reading(30.0, 30.0));
    assert_eq!(c.pick("cur").as_deref(), Some("y"));
}

// --- issue #2967: the usable rule ---------------------------------------------
//
// Rule 2 of the move before a wait: the test `profile usage` applies for
// "usable now" (session below `session_at`, default 95, and week below 100),
// read over `adjusted` windows, so a window whose reset has passed counts as
// 0. Rule 1 (`select`, the 90% threshold) is unchanged and runs first.
//
// - `is_usable(r, session_at, now) -> bool`
// - `regains_usable(r, session_at, now) -> Option<i64>`: for an unusable
//   reading, the latest `resets_at` among the windows that make it unusable;
//   `None` when it is usable or no such window has an instant. Never an
//   instant at or before `now`.
// - `select_usable(pool, current, session_at, eligible)`: the usable logins
//   that have a reading, ranked as `select` ranks (lowest worse window, then
//   name), never `current`, an excluded or an ineligible profile. A login
//   with no reading is not usable.

fn is_usable(r: &Reading, session_at: u8) -> bool {
    candidate::is_usable(r, session_at, NOW)
}

fn regains(r: &Reading, session_at: u8) -> Option<i64> {
    candidate::regains_usable(r, session_at, NOW)
}

#[test]
fn a_login_is_usable_below_the_session_bound_and_below_a_full_week() {
    assert!(is_usable(&reading(10.0, 5.0), 95));
    assert!(is_usable(&reading(94.9, 99.9), 95));
    assert!(is_usable(&reading(92.0, 40.0), 95), "over 90, under 95");
    assert!(is_usable(&reading(0.0, 0.0), 95));
}

#[test]
fn the_session_bound_is_strict_and_the_week_bound_is_one_hundred() {
    assert!(
        !is_usable(&reading(95.0, 0.0), 95),
        "at the bound is not under it"
    );
    assert!(!is_usable(&reading(100.0, 0.0), 95));
    assert!(!is_usable(&reading(0.0, 100.0), 95));
    assert!(!is_usable(&reading(100.0, 100.0), 95));
    assert!(is_usable(&reading(89.0, 0.0), 90));
    assert!(!is_usable(&reading(90.0, 0.0), 90));
    assert!(is_usable(&reading(99.9, 0.0), 100));
    assert!(!is_usable(&reading(100.0, 0.0), 100));
}

#[test]
fn a_window_that_has_reset_counts_as_zero() {
    let r = Reading {
        five_hour: w(100.0, -5),
        seven_day: w(100.0, 0),
        observed_at: NOW - 7200,
        source: ReadingSource::UsageLimitFailure,
    };
    assert!(is_usable(&r, 95));
    assert_eq!(regains(&r, 95), None);
}

#[test]
fn an_unreadable_value_is_not_usable() {
    assert!(!is_usable(&reading(f64::NAN, 0.0), 95));
    assert!(!is_usable(&reading(0.0, f64::NAN), 95));
}

#[test]
fn a_usable_login_regains_nothing() {
    assert_eq!(regains(&reading(10.0, 5.0), 95), None);
    assert_eq!(regains(&reading(94.0, 99.0), 95), None);
}

#[test]
fn a_login_out_on_session_regains_at_the_session_reset() {
    assert_eq!(regains(&reading(100.0, 5.0), 95), Some(NOW + 3600));
    assert_eq!(regains(&reading(96.0, 5.0), 95), Some(NOW + 3600));
}

#[test]
fn a_login_out_on_week_alone_regains_at_the_week_reset_not_the_session_one() {
    assert_eq!(regains(&reading(10.0, 100.0), 95), Some(NOW + 86_400));
}

#[test]
fn a_login_out_on_both_windows_regains_at_the_later_reset() {
    let r = Reading {
        five_hour: w(100.0, 3600),
        seven_day: w(100.0, 86_400),
        observed_at: NOW - 10,
        source: ReadingSource::Statusline,
    };
    assert_eq!(regains(&r, 95), Some(NOW + 86_400));
    // The session window resetting last gives the session reset.
    let r = Reading {
        five_hour: w(100.0, 7200),
        seven_day: w(100.0, 600),
        observed_at: NOW - 10,
        source: ReadingSource::Statusline,
    };
    assert_eq!(regains(&r, 95), Some(NOW + 7200));
}

#[test]
fn only_the_windows_that_make_it_unusable_count() {
    // The session window is fine (10%); the week is out. A session reset
    // later than the week's must not be picked.
    let r = Reading {
        five_hour: w(10.0, 20_000),
        seven_day: w(100.0, 600),
        observed_at: NOW - 10,
        source: ReadingSource::Statusline,
    };
    assert_eq!(regains(&r, 95), Some(NOW + 600));
    // A window that has already reset is not one of them.
    let r = Reading {
        five_hour: w(100.0, -30),
        seven_day: w(100.0, 600),
        observed_at: NOW - 10,
        source: ReadingSource::Statusline,
    };
    assert_eq!(regains(&r, 95), Some(NOW + 600));
}

#[test]
fn regains_is_never_at_or_before_now() {
    for resets in [-100, -1, 0] {
        let r = Reading {
            five_hour: w(100.0, resets),
            seven_day: w(10.0, 3600),
            observed_at: NOW - 10,
            source: ReadingSource::Statusline,
        };
        assert!(regains(&r, 95).is_none_or(|t| t > NOW), "resets {resets}");
    }
}

#[test]
fn extreme_resets_do_not_overflow() {
    let far = Reading {
        five_hour: Some(Window {
            used_percentage: 100.0,
            resets_at: i64::MAX,
        }),
        seven_day: w(0.0, 3600),
        observed_at: NOW,
        source: ReadingSource::Statusline,
    };
    assert_eq!(regains(&far, 95), Some(i64::MAX));
    let past = Reading {
        five_hour: Some(Window {
            used_percentage: 100.0,
            resets_at: i64::MIN,
        }),
        seven_day: w(0.0, 3600),
        observed_at: NOW,
        source: ReadingSource::Statusline,
    };
    assert_eq!(regains(&past, 95), None, "reset long ago: usable");
    assert!(is_usable(&past, 95));
}

fn usable_pick(c: &Case, current: &str) -> Option<String> {
    candidate::select_usable(&c.pool(), current, 95, |_| true).map(|p| p.to_string())
}

#[test]
fn the_usable_rule_takes_a_login_the_threshold_rule_refuses() {
    // 92% is over the 90% threshold and under the 95% bound.
    let c = Case::new(&["cur", "y"]).with("y", reading(92.0, 40.0));
    assert_eq!(c.pick("cur"), None, "rule 1 refuses it");
    assert_eq!(
        usable_pick(&c, "cur").as_deref(),
        Some("y"),
        "rule 2 takes it"
    );
}

#[test]
fn the_usable_rule_ignores_the_auto_switch_threshold() {
    let mut c = Case::new(&["cur", "y"]).with("y", reading(40.0, 40.0));
    c.threshold = 30;
    assert_eq!(c.pick("cur"), None);
    assert_eq!(usable_pick(&c, "cur").as_deref(), Some("y"));
}

#[test]
fn the_usable_rule_ranks_by_the_worse_window_then_name() {
    let c = Case::new(&["cur", "a", "b", "c", "d"])
        .with("a", reading(94.0, 10.0)) // worst 94
        .with("b", reading(92.0, 5.0)) // worst 92
        .with("c", reading(20.0, 92.0)) // worst 92, after b by name
        .with("d", reading(93.0, 0.0)); // worst 93
    assert_eq!(usable_pick(&c, "cur").as_deref(), Some("b"));
}

#[test]
fn the_usable_rule_never_takes_an_unusable_login() {
    let c = Case::new(&["cur", "x", "y"])
        .with("x", reading(95.0, 0.0))
        .with("y", reading(0.0, 100.0));
    assert_eq!(usable_pick(&c, "cur"), None);
}

#[test]
fn a_login_with_no_reading_is_not_usable_by_the_second_rule() {
    // Rule 1 takes it (as it always has); rule 2 does not.
    let c = Case::new(&["cur", "unread"]);
    assert_eq!(c.pick("cur").as_deref(), Some("unread"));
    assert_eq!(usable_pick(&c, "cur"), None);
}

#[test]
fn the_usable_rule_never_takes_the_current_an_excluded_or_an_ineligible_login() {
    let mut c = Case::new(&["cur", "skip", "busy", "ok"])
        .with("cur", reading(1.0, 1.0))
        .with("skip", reading(1.0, 1.0))
        .with("busy", reading(2.0, 2.0))
        .with("ok", reading(94.0, 0.0));
    c.exclude = vec![n("skip")];
    let picked = candidate::select_usable(&c.pool(), "cur", 95, |p| p.as_str() != "busy");
    assert_eq!(picked.map(|p| p.to_string()).as_deref(), Some("ok"));
    // The login that just reported the limit is never chosen, whatever its
    // reading says: here it reads as perfectly usable.
    let c = Case::new(&["cur"]).with("cur", reading(1.0, 1.0));
    assert_eq!(usable_pick(&c, "cur"), None);
}

#[test]
fn the_usable_session_bound_is_a_parameter() {
    let c = Case::new(&["cur", "y"]).with("y", reading(92.0, 0.0));
    assert!(candidate::select_usable(&c.pool(), "cur", 95, |_| true).is_some());
    assert!(candidate::select_usable(&c.pool(), "cur", 90, |_| true).is_none());
}
