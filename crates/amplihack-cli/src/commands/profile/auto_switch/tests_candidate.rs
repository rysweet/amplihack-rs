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

// --- Overflow profiles (issue #1606, D7) -----------------------------------

#[test]
fn auto_switch_never_picks_an_overflow_profile_even_as_the_only_option() {
    use super::config::Config;
    let cfg = Config {
        enabled: true,
        threshold: 90,
        exclude: Vec::new(),
        overflow: vec![n("cred")],
    };
    let mut c = Case::new(&["work", "spare", "cred"])
        .with("work", reading(95.0, 50.0))
        .with("spare", reading(99.0, 99.0))
        .with("cred", reading(0.0, 0.0));
    c.exclude = cfg.selection_exclude();
    assert_eq!(c.pick("work"), None, "only the rotator spends credits");
    c.readings.insert("spare".into(), reading(10.0, 10.0));
    assert_eq!(c.pick("work").as_deref(), Some("spare"));
}

/// AU4: every `Pool` built outside tests takes its exclusions from
/// `Config::selection_exclude()`, so a new call site cannot let auto-switch
/// spend credits by using `exclude` alone.
#[test]
fn every_pool_construction_uses_selection_exclude() {
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands/profile/auto_switch");
    let mut sites = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        if !file.ends_with(".rs") || file.starts_with("tests_") || file == "candidate.rs" {
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap();
        for (i, _) in src.match_indices("Pool {") {
            sites += 1;
            // The Pool literal and the few lines that build its inputs.
            let start = src[..i].char_indices().rev().nth(400).map_or(0, |(j, _)| j);
            let end = src[i..].find("};").map_or(src.len(), |e| i + e);
            let body = &src[start..end];
            assert!(
                body.contains("selection_exclude()") && !body.contains("exclude: &cfg.exclude"),
                "{file}: a Pool built without selection_exclude():\n{body}"
            );
        }
    }
    assert!(
        sites >= 2,
        "expected the rerun and swap Pool sites, found {sites}"
    );
}
