//! Issue #2967: the logins whose resets bound a step's wait. Temporary
//! metadata only; no real profile root, no network and no Claude process.
//!
//! Contract (`auto_switch/limit_choice.rs`):
//!
//! - `limit_choice::cross_login_reset(reads, current, tried, wait_now)
//!   -> Vec<(ProfileName, i64)>`: for each plan profile that passes the
//!   candidate rule's eligibility (not excluded, not overflow, not leased,
//!   not hosting another account, `login_ready`), is not `current`, is not in
//!   `tried`, has a reading and is exhausted by the candidate rule (a window
//!   at or above the auto-switch threshold), the instant
//!   `candidate::regains_room` gives, moved onto the wait's clock
//!   (`wait_now + (instant - fsx::now())`, saturating). Only instants strictly
//!   after `wait_now` are returned. A login under the threshold, a login with
//!   no reading or no instant, and a login barred by another rule contribute
//!   nothing.
//! - `limit_choice::may_run_on(reads, current, login) -> bool`: the same
//!   eligibility, with no reading asked for.
//! - The note the retry after an earliest-reset wait prints reaches the
//!   step's stderr, which the recipe runner's classifier reads, so it must
//!   never read as a usage limit.

use super::config::Config;
use super::limit_choice::{cross_login_reset, may_run_on};
use super::rerun::Reads;
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
        let lock = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let fake_home = tmp.path().join("home");
        fs::create_dir(&fake_home).unwrap();
        let mut env = crate::test_support::EnvGuard::capture(&[]);
        env.assign("HOME", &fake_home);
        env.assign("AMPLIHACK_CLAUDE_PROFILE_ROOT", tmp.path());
        for key in [
            "AMPLIHACK_ROTATE_OVERFLOW_WAIT",
            "AMPLIHACK_ROTATE_OVERFLOW_UNKNOWN_RESET",
            "AMPLIHACK_ROTATE_SESSION_AT",
            "AMPLIHACK_ROTATE_WEEK_AT",
        ] {
            env.clear(key);
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

    fn threshold(mut self, at: u8) -> Self {
        self.cfg.threshold = at;
        self
    }

    fn may_run_on(&self, current: &str, login: &str) -> bool {
        may_run_on(self.reads(), current, &n(login))
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

// --- cross_login_reset -------------------------------------------------------

#[test]
fn an_exhausted_login_contributes_its_reset_on_the_waits_clock() {
    let w = World::new(&["cur", "a"])
        .read("cur", read(100.0, 10.0))
        .read("a", reading_with(100.0, 1800, 10.0, 86_400));
    let got = w.resets("cur", &[], WAIT_NOW);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].0, "a");
    assert!(near(got[0].1, WAIT_NOW + 1800), "{got:?}");
}

#[test]
fn a_login_under_the_threshold_contributes_nothing() {
    // The candidate rule takes such a login before any wait starts.
    let w = World::new(&["cur", "a", "b"])
        .read("cur", read(100.0, 10.0))
        .read("a", read(89.0, 10.0))
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
    // A window that has already reset counts as empty, not as a reset to
    // wait for.
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
fn the_auto_switch_threshold_decides_which_logins_are_waited_for() {
    // One bound: the candidate rule's. A login at 92% is exhausted at the
    // default threshold of 90 and waited for, and under a threshold of 95 it
    // is the candidate rule's to take, so nothing waits for it.
    let base = |w: World| {
        w.read("cur", read(100.0, 10.0))
            .read("a", reading_with(92.0, 1800, 10.0, 86_400))
    };
    let at_90 = base(World::new(&["cur", "a"]));
    let got = at_90.resets("cur", &[], WAIT_NOW);
    assert_eq!(got.len(), 1, "92% is exhausted at 90: {got:?}");
    drop(at_90);
    let at_95 = base(World::new(&["cur", "a"]).threshold(95));
    assert_eq!(at_95.resets("cur", &[], WAIT_NOW), vec![]);
}

// --- may_run_on ---------------------------------------------------------------

#[test]
fn a_stored_login_of_its_own_may_be_run_on_with_no_reading() {
    let w = World::new(&["cur", "a"]);
    assert!(w.may_run_on("cur", "a"));
}

#[test]
fn a_login_barred_by_another_rule_may_not_be_run_on() {
    let w = World::new(&["cur", "x", "h", "l"])
        .exclude("x")
        .hosts("h", "cur")
        .logged_out("l");
    for login in ["x", "h", "l", "cur"] {
        assert!(!w.may_run_on("cur", login), "{login}");
    }
}

#[test]
fn a_login_no_longer_in_the_store_may_not_be_run_on() {
    let w = World::new(&["cur", "a"]);
    assert!(!w.may_run_on("cur", "gone"));
}

// --- the note a retry prints --------------------------------------------------

#[test]
fn the_earliest_reset_note_carries_no_usage_limit_marker_and_names_no_failed_login() {
    // It reaches the step's stderr, which the recipe runner's classifier
    // reads: a note that named a usage limit would be read as one.
    use super::rerun::NOTE_EARLIEST;
    use crate::commands::recipe::run::limit_patterns::all_markers;
    use crate::commands::recipe::run::{FailureClass, classify_failure_text};
    let line = format!("{NOTE_EARLIEST} \"personal\" (auto-switch)");
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
