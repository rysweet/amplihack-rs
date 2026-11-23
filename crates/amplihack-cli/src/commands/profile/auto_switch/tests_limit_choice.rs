//! Issue #2967: the move a step makes before it waits, and the logins whose
//! resets bound a wait. Temporary metadata only; no real profile root, no
//! network and no Claude process.
//!
//! Contract (`auto_switch/limit_choice.rs`, wired with
//! `#[cfg(all(test, unix))] mod tests_limit_choice;` in `auto_switch/mod.rs`):
//!
//! - `rerun::Reads` and its five fields become `pub(super)`, so the sibling
//!   module and these tests can build one. `rerun::Pick` gains
//!   `usable_only: bool`.
//! - `limit_choice::limit_candidate(reads, current, skip, alert) -> Option<Pick>`
//!   runs rule 1 (the unchanged `candidate`, 90% threshold) and, only when it
//!   yields nothing, rule 2: the `profile usage` "usable now" test (session
//!   below `session_at`, default 95 and read once per call from
//!   `capacity_weight::usable_threshold(None, env)`; week below 100) over
//!   `adjusted` windows, ranked by the existing ordering, under the same
//!   eligibility and affinity closures as rule 1. A pick by rule 2 has
//!   `usable_only == true`; a pick by rule 1 has `false`. A login with no
//!   reading is never picked by rule 2. `current` and every name in `skip`
//!   are never picked by either rule.
//! - `limit_choice::cross_login_reset(reads, current, tried, wait_now)
//!   -> Vec<(ProfileName, i64)>`: for each profile that passes the same
//!   eligibility rules as rule 1 (not excluded, not overflow, not leased, not
//!   hosting another account, within affinity, `login_ready`), is not
//!   `current`, is not in `tried`, has a reading and is not usable by rule 2,
//!   the instant (`candidate::regains_usable`) moved onto the wait's clock
//!   (`wait_now + (instant - fsx::now())`, saturating). Only instants strictly
//!   after `wait_now` are returned. A usable login, a login with no reading or
//!   no instant, and a login barred by another rule contribute nothing.

use super::config::Config;
use super::limit_choice::{cross_login_reset, limit_candidate};
use super::rerun::{KeyAlert, Reads};
use super::state::{HostEntry, Ledger, State};
use super::usage::{Reading, ReadingSource, Window};
use crate::commands::profile::name::ProfileName;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn now() -> i64 {
    super::fsx::now()
}

/// A reading with both windows, the session one resetting in an hour and
/// the week one in a day.
fn read(five: f64, seven: f64) -> Reading {
    reading_with(five, 3600, seven, 86_400)
}

fn reading_with(five: f64, five_in: i64, seven: f64, seven_in: i64) -> Reading {
    let at = now();
    Reading {
        five_hour: Some(Window {
            used_percentage: five,
            resets_at: at.saturating_add(five_in),
        }),
        seven_day: Some(Window {
            used_percentage: seven,
            resets_at: at.saturating_add(seven_in),
        }),
        observed_at: at,
        source: ReadingSource::Statusline,
    }
}

/// A store of `names`, each holding its own login on its own account.
struct World {
    _tmp: tempfile::TempDir,
    _lock: std::sync::MutexGuard<'static, ()>,
    _env: crate::test_support::EnvGuard,
    state: State,
    profiles: Vec<ProfileName>,
    readings: BTreeMap<String, Reading>,
    ledger: Ledger,
    cfg: Config,
}

impl World {
    fn new(names: &[&str]) -> Self {
        Self::with_env(names, &[])
    }

    fn with_env(names: &[&str], extra: &[(&str, &str)]) -> Self {
        let lock = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let fake_home = tmp.path().join("home");
        fs::create_dir(&fake_home).unwrap();
        let mut env = crate::test_support::EnvGuard::capture(&[]);
        env.assign("HOME", &fake_home);
        env.assign("AMPLIHACK_CLAUDE_PROFILE_ROOT", tmp.path());
        for (key, value) in extra {
            env.assign(key, value);
        }
        for key in [
            "AMPLIHACK_ROTATE_OVERFLOW_WAIT",
            "AMPLIHACK_ROTATE_OVERFLOW_UNKNOWN_RESET",
            "AMPLIHACK_ROTATE_SESSION_AT",
            "AMPLIHACK_ROTATE_WEEK_AT",
            "AMPLIHACK_PROFILE_USAGE_USABLE_SESSION_PCT",
        ] {
            if !extra.iter().any(|(set, _)| *set == key) {
                env.clear(key);
            }
        }
        fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let state = State::new(tmp.path()).unwrap();
        for name in names {
            Self::login(&tmp, &state, name);
        }
        Self {
            _tmp: tmp,
            _lock: lock,
            _env: env,
            state,
            profiles: names.iter().map(|p| n(p)).collect(),
            readings: BTreeMap::new(),
            ledger: Ledger::default(),
            cfg: Config {
                enabled: true,
                threshold: 90,
                exclude: vec![],
                overflow: vec![],
                kind: Default::default(),
            },
        }
    }

    fn login(tmp: &tempfile::TempDir, state: &State, name: &str) {
        fs::create_dir(tmp.path().join(name)).unwrap();
        fs::write(
            tmp.path().join(name).join(".credentials.json"),
            "synthetic-login",
        )
        .unwrap();
        state
            .write_sub_json(
                "expected",
                name,
                &serde_json::json!({"account": format!("u-{name}"), "org": "org-a"}),
            )
            .unwrap();
        fs::write(
            tmp.path().join(name).join(".claude.json"),
            serde_json::json!({
                "oauthAccount": {
                    "accountUuid": format!("u-{name}"),
                    "organizationUuid": "org-a"
                }
            })
            .to_string(),
        )
        .unwrap();
    }

    fn read(mut self, name: &str, r: Reading) -> Self {
        self.readings.insert(name.to_string(), r);
        self
    }

    fn exclude(mut self, name: &str) -> Self {
        self.cfg.exclude.push(n(name));
        self
    }

    /// Directory `dir` hosts the account of profile `live`.
    fn hosts(mut self, dir: &str, live: &str) -> Self {
        self.ledger.hosts.insert(
            dir.to_string(),
            HostEntry {
                live: live.to_string(),
                swapped_at: now(),
                pre_swap_fingerprint: None,
            },
        );
        self
    }

    /// `name` holds no login of its own.
    fn logged_out(self, name: &str) -> Self {
        fs::remove_file(self.state.root.join(name).join(".credentials.json")).unwrap();
        self
    }

    fn reads(&self) -> Reads<'_> {
        Reads {
            state: &self.state,
            ledger: &self.ledger,
            profiles: &self.profiles,
            readings: &self.readings,
            cfg: &self.cfg,
        }
    }

    fn choose(&self, current: &str, skip: &[&str]) -> Option<(String, bool)> {
        let skip: Vec<String> = skip.iter().map(|s| s.to_string()).collect();
        limit_candidate(self.reads(), current, &skip, KeyAlert::Silent)
            .map(|p| (p.profile.to_string(), p.usable_only))
    }

    fn resets(&self, current: &str, tried: &[&str], wait_now: i64) -> Vec<(String, i64)> {
        let tried: Vec<String> = tried.iter().map(|s| s.to_string()).collect();
        let mut got: Vec<(String, i64)> =
            cross_login_reset(self.reads(), current, &tried, wait_now)
                .into_iter()
                .map(|(p, at)| (p.to_string(), at))
                .collect();
        got.sort();
        got
    }
}

/// An instant on the wait's clock, within the seconds the test itself takes.
fn near(got: i64, want: i64) -> bool {
    (got - want).abs() <= 3
}

const WAIT_NOW: i64 = 1_000_000;

// --- limit_candidate: rule 1, then rule 2 ------------------------------------

#[test]
fn rule_one_runs_first_and_its_pick_is_not_marked_usable_only() {
    let w = World::new(&["cur", "a", "b"])
        .read("cur", read(100.0, 10.0))
        .read("a", read(50.0, 10.0))
        .read("b", read(92.0, 10.0));
    assert_eq!(w.choose("cur", &[]), Some(("a".into(), false)));
}

#[test]
fn rule_two_takes_a_login_over_the_threshold_and_under_the_usable_bound() {
    // Observed on 21 November 2026: four logins were usable by `profile
    // usage` and the step moved to none of them.
    let w = World::new(&["cur", "b"])
        .read("cur", read(100.0, 10.0))
        .read("b", read(92.0, 10.0));
    assert_eq!(w.choose("cur", &[]), Some(("b".into(), true)));
}

#[test]
fn rule_two_ranks_as_the_existing_chooser_does() {
    let w = World::new(&["cur", "a", "b", "c"])
        .read("cur", read(100.0, 10.0))
        .read("a", read(94.0, 10.0))
        .read("b", read(92.0, 10.0))
        .read("c", read(93.0, 10.0));
    assert_eq!(w.choose("cur", &[]), Some(("b".into(), true)));
}

#[test]
fn nothing_usable_gives_no_pick() {
    let w = World::new(&["cur", "a", "b"])
        .read("cur", read(100.0, 10.0))
        .read("a", read(95.0, 10.0))
        .read("b", read(10.0, 100.0));
    assert_eq!(w.choose("cur", &[]), None);
}

#[test]
fn the_login_that_reported_the_limit_is_never_chosen_whatever_it_reads() {
    // Its stored reading is stale and says it is nearly empty.
    let w = World::new(&["cur", "a"])
        .read("cur", read(1.0, 1.0))
        .read("a", read(100.0, 100.0));
    assert_eq!(w.choose("cur", &[]), None);
    // One world at a time: each holds the process-wide environment lock.
    drop(w);
    let w = World::new(&["cur"]).read("cur", read(1.0, 1.0));
    assert_eq!(w.choose("cur", &[]), None);
}

#[test]
fn a_login_the_step_already_tried_is_never_chosen_by_either_rule() {
    let w = World::new(&["cur", "a", "b"])
        .read("cur", read(100.0, 10.0))
        .read("a", read(10.0, 10.0))
        .read("b", read(92.0, 10.0));
    assert_eq!(w.choose("cur", &["a"]), Some(("b".into(), true)));
    assert_eq!(w.choose("cur", &["a", "b"]), None);
}

#[test]
fn an_excluded_login_is_never_chosen_by_rule_two() {
    let w = World::new(&["cur", "a"])
        .read("cur", read(100.0, 10.0))
        .read("a", read(92.0, 10.0))
        .exclude("a");
    assert_eq!(w.choose("cur", &[]), None);
}

#[test]
fn a_login_hosting_another_account_is_rejected_by_rule_two() {
    // SR-22: rule 2 shares rule 1's eligibility, so a leased directory is
    // never moved onto because its reading is usable.
    let w = World::new(&["cur", "a"])
        .read("cur", read(100.0, 10.0))
        .read("a", read(92.0, 10.0))
        .hosts("a", "cur");
    assert_eq!(w.choose("cur", &[]), None);
}

#[test]
fn a_login_leased_to_another_step_is_rejected_by_rule_two() {
    let w = World::new(&["cur", "a", "b"])
        .read("cur", read(100.0, 10.0))
        .read("a", read(92.0, 10.0))
        .read("b", read(93.0, 10.0))
        .hosts("b", "a");
    // `b` hosts `a`'s account, so neither directory is free for a move.
    let got = w.choose("cur", &[]);
    assert!(got.as_ref().is_none_or(|(p, _)| p != "b"), "{got:?}");
}

#[test]
fn a_login_that_holds_no_login_of_its_own_is_rejected_by_rule_two() {
    let w = World::new(&["cur", "a"])
        .read("cur", read(100.0, 10.0))
        .read("a", read(92.0, 10.0))
        .logged_out("a");
    assert_eq!(w.choose("cur", &[]), None);
}

#[test]
fn a_login_with_no_reading_is_taken_by_rule_one_and_never_by_rule_two() {
    let w = World::new(&["cur", "unread"]).read("cur", read(100.0, 10.0));
    assert_eq!(w.choose("cur", &[]), Some(("unread".into(), false)));
    // Rule 1 refuses it once it is tried; rule 2 has nothing to add.
    assert_eq!(w.choose("cur", &["unread"]), None);
}

#[test]
fn the_usable_bound_is_read_from_the_environment_once_per_call() {
    let w = World::with_env(
        &["cur", "b"],
        &[("AMPLIHACK_PROFILE_USAGE_USABLE_SESSION_PCT", "90")],
    )
    .read("cur", read(100.0, 10.0))
    .read("b", read(92.0, 10.0));
    assert_eq!(w.choose("cur", &[]), None, "92% is not under a bound of 90");
    // One world at a time: each holds the process-wide environment lock.
    drop(w);
    let w = World::with_env(
        &["cur", "b"],
        &[("AMPLIHACK_PROFILE_USAGE_USABLE_SESSION_PCT", "99")],
    )
    .read("cur", read(100.0, 10.0))
    .read("b", read(98.0, 10.0));
    assert_eq!(w.choose("cur", &[]), Some(("b".into(), true)));
}

#[test]
fn a_garbled_usable_bound_falls_back_to_ninety_five() {
    let w = World::with_env(
        &["cur", "b"],
        &[("AMPLIHACK_PROFILE_USAGE_USABLE_SESSION_PCT", "lots")],
    )
    .read("cur", read(100.0, 10.0))
    .read("b", read(94.0, 10.0));
    assert_eq!(w.choose("cur", &[]), Some(("b".into(), true)));
}

#[test]
fn a_window_that_has_reset_counts_as_empty_for_rule_two() {
    let w = World::new(&["cur", "b"])
        .read("cur", read(100.0, 10.0))
        .read("b", reading_with(100.0, -60, 10.0, 86_400));
    // Rule 1 also sees `b` at 10%, so it is the unmarked pick.
    assert_eq!(w.choose("cur", &[]), Some(("b".into(), false)));
}

#[test]
fn termination_every_pick_is_new_so_the_choices_end() {
    // SR-23: with `tried` growing by each pick, two logins that both report
    // a limit end after two moves, whichever rule picked them.
    let w = World::new(&["cur", "a", "b"])
        .read("cur", read(100.0, 10.0))
        .read("a", read(92.0, 10.0))
        .read("b", read(93.0, 10.0));
    let mut tried: Vec<&str> = vec![];
    let mut moves = 0;
    while let Some((next, _)) = w.choose("cur", &tried) {
        let name: &'static str = match next.as_str() {
            "a" => "a",
            "b" => "b",
            other => panic!("unexpected pick {other}"),
        };
        tried.push(name);
        moves += 1;
        assert!(moves <= 2, "more moves than stored logins: {tried:?}");
    }
    assert_eq!(moves, 2);
}

// --- cross_login_reset -------------------------------------------------------

#[test]
fn an_unusable_login_contributes_its_reset_on_the_waits_clock() {
    let w = World::new(&["cur", "a"])
        .read("cur", read(100.0, 10.0))
        .read("a", reading_with(100.0, 1800, 10.0, 86_400));
    let got = w.resets("cur", &[], WAIT_NOW);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].0, "a");
    assert!(near(got[0].1, WAIT_NOW + 1800), "{got:?}");
}

#[test]
fn a_usable_login_contributes_nothing() {
    let w = World::new(&["cur", "a", "b"])
        .read("cur", read(100.0, 10.0))
        .read("a", read(92.0, 10.0))
        .read("b", reading_with(100.0, 1800, 10.0, 86_400));
    let got = w.resets("cur", &[], WAIT_NOW);
    assert_eq!(got.iter().map(|g| g.0.as_str()).collect::<Vec<_>>(), ["b"]);
}

#[test]
fn the_failed_login_and_the_tried_logins_contribute_nothing() {
    let w = World::new(&["cur", "a", "b"])
        .read("cur", reading_with(100.0, 600, 10.0, 86_400))
        .read("a", reading_with(100.0, 900, 10.0, 86_400))
        .read("b", reading_with(100.0, 1200, 10.0, 86_400));
    let got = w.resets("cur", &["a"], WAIT_NOW);
    assert_eq!(got.iter().map(|g| g.0.as_str()).collect::<Vec<_>>(), ["b"]);
}

#[test]
fn a_login_with_no_reading_contributes_nothing() {
    let w = World::new(&["cur", "a"]).read("cur", read(100.0, 10.0));
    assert_eq!(w.resets("cur", &[], WAIT_NOW), vec![]);
}

#[test]
fn a_login_barred_by_another_rule_contributes_nothing() {
    let w = World::new(&["cur", "x", "h", "l"])
        .read("cur", read(100.0, 10.0))
        .read("x", reading_with(100.0, 600, 10.0, 86_400))
        .read("h", reading_with(100.0, 700, 10.0, 86_400))
        .read("l", reading_with(100.0, 800, 10.0, 86_400))
        .exclude("x")
        .hosts("h", "cur")
        .logged_out("l");
    assert_eq!(w.resets("cur", &[], WAIT_NOW), vec![]);
}

#[test]
fn the_instant_is_the_latest_reset_among_the_windows_that_bar_it() {
    let w = World::new(&["cur", "a", "b"])
        .read("cur", read(100.0, 10.0))
        // Both windows out: the later reset.
        .read("a", reading_with(100.0, 600, 100.0, 7200))
        // Only the week out: the session reset is not one of the bars.
        .read("b", reading_with(10.0, 20_000, 100.0, 900));
    let got = w.resets("cur", &[], WAIT_NOW);
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(near(got[0].1, WAIT_NOW + 7200), "{got:?}");
    assert!(near(got[1].1, WAIT_NOW + 900), "{got:?}");
}

#[test]
fn an_instant_at_or_before_the_waits_clock_is_left_out() {
    // A window that has already reset is usable, not a reset to wait for.
    let w = World::new(&["cur", "a"])
        .read("cur", read(100.0, 10.0))
        .read("a", reading_with(100.0, -5, 100.0, 86_400));
    let got = w.resets("cur", &[], WAIT_NOW);
    assert!(got.iter().all(|(_, at)| *at > WAIT_NOW), "{got:?}");
}

#[test]
fn a_reset_a_year_ahead_is_returned_unclamped_and_does_not_overflow() {
    // Holding the instant to a window belongs to the plan; this only reports.
    let year = 365 * 86_400;
    let w = World::new(&["cur", "a"])
        .read("cur", read(100.0, 10.0))
        .read("a", reading_with(100.0, year, 10.0, year));
    let got = w.resets("cur", &[], WAIT_NOW);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(near(got[0].1, WAIT_NOW + year), "{got:?}");
}

#[test]
fn extreme_stored_resets_do_not_panic() {
    let mut max = read(100.0, 10.0);
    max.five_hour.as_mut().unwrap().resets_at = i64::MAX;
    let mut min = read(100.0, 10.0);
    min.five_hour.as_mut().unwrap().resets_at = i64::MIN;
    let w = World::new(&["cur", "a", "b"])
        .read("cur", read(100.0, 10.0))
        .read("a", max)
        .read("b", min);
    let got = w.resets("cur", &[], i64::MAX - 10);
    assert!(got.iter().all(|(_, at)| *at > i64::MAX - 10), "{got:?}");
    let got = w.resets("cur", &[], i64::MIN);
    assert!(got.iter().all(|(_, at)| *at > i64::MIN), "{got:?}");
}

#[test]
fn the_usable_bound_decides_which_logins_are_waited_for() {
    let w = World::with_env(
        &["cur", "a"],
        &[("AMPLIHACK_PROFILE_USAGE_USABLE_SESSION_PCT", "90")],
    )
    .read("cur", read(100.0, 10.0))
    .read("a", reading_with(92.0, 1800, 10.0, 86_400));
    let got = w.resets("cur", &[], WAIT_NOW);
    assert_eq!(got.len(), 1, "92% is unusable at a bound of 90: {got:?}");
}

// --- the notes a retry prints -------------------------------------------------

#[test]
fn the_retry_notes_carry_no_usage_limit_marker_and_name_no_failed_login() {
    // They reach the step's stderr, which the recipe runner's classifier
    // reads: a note that named a usage limit would be read as one.
    use super::rerun::{NOTE_CAPACITY, NOTE_EARLIEST, NOTE_NO_ROOM};
    use crate::commands::recipe::run::limit_patterns::all_markers;
    use crate::commands::recipe::run::{FailureClass, classify_failure_text};
    for note in [NOTE_CAPACITY, NOTE_EARLIEST, NOTE_NO_ROOM] {
        let line = format!("{note} \"personal\" (auto-switch)");
        let lowered = line.to_ascii_lowercase();
        for marker in all_markers() {
            assert!(!lowered.contains(marker), "{marker:?} in {line:?}");
        }
        assert_ne!(
            classify_failure_text(&line).0,
            FailureClass::UsageLimit,
            "{line:?}"
        );
        assert!(line.starts_with("amplihack: "), "{line:?}");
        assert_eq!(line.lines().count(), 1, "{line:?}");
    }
}
