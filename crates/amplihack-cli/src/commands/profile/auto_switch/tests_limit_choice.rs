//! Issue #2967: the logins whose resets bound a step's wait. Temporary
//! metadata only; no real profile root, no network and no Claude process.
//!
//! Contract (`auto_switch/limit_choice.rs`):
//!
//! - `limit_choice::cross_login_reset(reads, current, tried, wait_now)
//!   -> Vec<(ProfileName, i64)>`: for each plan profile that passes the
//!   candidate rule's eligibility (not excluded, not overflow, not leased,
//!   not hosting another account, `login_ready`), is not `current`, is not in
//!   `tried`, has a reading no older than `MAX_READING_AGE_SECS` (one
//!   session window, the rotator's own bound) and is exhausted by the
//!   candidate rule (a window at or above the auto-switch threshold), the
//!   instant `candidate::regains_room` gives, or the end of a standing block
//!   in the rotate ledger when that is later (`rotate::decide::standing_block`),
//!   moved onto the wait's clock (`wait_now + (instant - fsx::now())`,
//!   saturating). Only instants strictly after `wait_now` are returned. A
//!   login under the threshold, a login with no reading, an older one or no
//!   instant, and a login barred by another rule contribute nothing.
//! - `limit_choice::may_run_on(reads, current, login) -> bool`: the same
//!   eligibility, with no reading asked for.
//! - `limit_choice::held_since(reads, login, since) -> bool`: a block
//!   recorded, or a reading taken, after `since` that still holds the login
//!   out. The retry after an earliest-reset wait is refused on it
//!   (`reason=login_still_limited`).
//! - The note the retry after an earliest-reset wait prints reaches the
//!   step's stderr, which the recipe runner's classifier reads, so it must
//!   never read as a usage limit, and it is printed only for a move that
//!   happens (`rerun::earliest_note`).
//! - A choice among the logins that cannot be made says why
//!   (`rerun::with_reads`, `rerun::may_retry_on`): auto-switch turned off,
//!   the ledger or the readings unreadable, or the login whose reset ended
//!   the wait no longer eligible. Each cause has its own stderr line, which
//!   names it and the error, and never reads as a usage limit.

use super::config::Config;
use super::limit_choice::{
    MAX_READING_AGE_SECS, blocks_unread_line, cross_login_reset, may_run_on,
};
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
    env: crate::test_support::EnvGuard,
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
            env,
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

    /// Writes `body` as the rotate ledger, private as its reader requires.
    fn rotate_ledger(self, body: &str) -> Self {
        let path = amplihack_utils::rotator_status::ledger_path(&self.state.root);
        let dir = path.parent().unwrap();
        fs::create_dir_all(dir).unwrap();
        for d in [dir, dir.parent().unwrap()] {
            fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        self
    }

    /// Records a limit block on each `(login, at, until)`, on the real
    /// clock, as a failed step records one (`rotate::step_block`).
    fn blocked(self, blocks: &[(&str, i64, i64)]) -> Self {
        let map: serde_json::Map<String, serde_json::Value> = blocks
            .iter()
            .map(|(login, at, until)| {
                let b = serde_json::json!({"at": at, "until": until, "reason": "spend_limit"});
                (login.to_string(), b)
            })
            .collect();
        let body = serde_json::json!({"version": 1, "overflow_blocked": map});
        self.rotate_ledger(&body.to_string())
    }

    /// Writes the readings to the usage file, where a choice made through
    /// `with_reads` reads them.
    fn saved(self) -> Self {
        self.state
            .save_usage(&super::state::UsageFile {
                version: 1,
                accounts: self.readings.clone(),
            })
            .unwrap();
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

// --- what the rotator would refuse (issues #3105, #3111) ---------------------------
//
// The rotator ranks a login on no stored reading older than one session
// window, never on a reading older than one it already holds, and never on a
// login while a recorded block stands on it. The earliest-reset deadline
// reads the same data by the same rules, so the two cannot disagree on which
// logins come back.

/// `r`, as if taken `ago` seconds before now. A negative `ago` stamps it
/// ahead of now.
fn taken(mut r: Reading, ago: i64) -> Reading {
    r.observed_at = now().saturating_sub(ago);
    r
}

#[test]
fn the_reading_age_bound_is_the_rotators_one_session_window() {
    assert_eq!(MAX_READING_AGE_SECS, 18_000);
    assert_eq!(
        MAX_READING_AGE_SECS,
        crate::commands::profile::rotate::decide::LAST_GOOD_MAX_AGE_SECS
    );
}

#[test]
fn a_reading_older_than_one_session_window_contributes_nothing() {
    // Only the week window is out: a session window this old has reset.
    let week_out = || reading_with(10.0, 600, 100.0, 1800);
    let w = World::new(&["cur", "old", "inside", "ahead"])
        .read("cur", read(100.0, 10.0))
        .read("old", taken(week_out(), MAX_READING_AGE_SECS + 60))
        .read("inside", taken(week_out(), MAX_READING_AGE_SECS - 60))
        .read("ahead", taken(week_out(), -3600));
    let got = w.resets("cur", &[], WAIT_NOW);
    assert_eq!(
        got.iter().map(|g| g.0.as_str()).collect::<Vec<_>>(),
        ["inside"],
        "only a reading taken within one session window before now counts: {got:?}"
    );
}

#[test]
fn a_standing_block_holds_the_instant_to_its_end() {
    let t = now();
    let exhausted = || reading_with(100.0, 1800, 10.0, 86_400);
    let w = World::new(&["cur", "later", "sooner", "ended", "roomy"])
        .read("cur", read(100.0, 10.0))
        .read("later", exhausted())
        .read("sooner", exhausted())
        .read("ended", exhausted())
        // Under the threshold, so the block is all that holds it out.
        .read("roomy", read(10.0, 10.0))
        .blocked(&[
            ("later", t - 60, t + 3600),
            ("sooner", t - 60, t + 600),
            ("ended", t - 7200, t - 60),
            ("roomy", t - 60, t + 900),
        ]);
    let got = w.resets("cur", &[], WAIT_NOW);
    assert_eq!(got.len(), 3, "{got:?}");
    let at = |login: &str| got.iter().find(|g| g.0 == login).map(|g| g.1);
    // A block that ends after the reset holds the login to the block's end.
    assert!(near(at("later").unwrap(), WAIT_NOW + 3600), "{got:?}");
    // One that ends first, and one that has ended, leave the reset.
    assert!(near(at("sooner").unwrap(), WAIT_NOW + 1800), "{got:?}");
    assert!(near(at("ended").unwrap(), WAIT_NOW + 1800), "{got:?}");
    // A block never adds a login the readings do not already wait for.
    assert_eq!(at("roomy"), None, "{got:?}");
}

#[test]
fn an_unreadable_rotate_ledger_holds_no_login_out_and_is_named() {
    let w = World::new(&["cur", "a"])
        .read("cur", read(100.0, 10.0))
        .read("a", reading_with(100.0, 1800, 10.0, 86_400))
        .rotate_ledger("{");
    let got = w.resets("cur", &[], WAIT_NOW);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(near(got[0].1, WAIT_NOW + 1800), "{got:?}");
    let line = blocks_unread_line("rotate ledger: invalid JSON at line 1 column 1");
    assert_eq!(
        line,
        "amplihack: the rotate ledger cannot be read: rotate ledger: invalid JSON at line 1 \
         column 1; no block it records on another login is known to this step's wait \
         (auto-switch)"
    );
    assert_not_a_limit(&line);
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

// --- a choice that cannot be made says why (issue #2967) -----------------------
//
// `with_reads` used to turn each of these into a bare `None`, so the wait
// went on to the step's own reset, the two-hour wait of #2967, with nothing
// in either log to tell "the logins could not be read" from "no other login
// had a reset ahead". Each is now an error naming its cause, and each cause
// has a stderr line. The wait line beside it (`other_logins_unread`,
// `retry_not_moved`) is pinned in `limit_wait/tests_log_recipe.rs`, and the
// whole path in `tests/issue_2967_other_logins.rs`.

use super::config::TOGGLE_ENV;
use super::rerun::{NOTE_EARLIEST, NotConsidered, earliest_note, may_retry_on, with_reads};
use crate::commands::profile::store::ProfileStore;

impl World {
    /// Auto-switch on or off through its environment toggle, which is how
    /// `config::effective` is read on every call.
    fn auto_switch(mut self, on: bool) -> Self {
        self.env.assign(TOGGLE_ENV, if on { "1" } else { "0" });
        self
    }

    /// Writes `body` as the auto-switch state file `name`, private as the
    /// state requires, so only its content can make it unreadable.
    fn state_file(self, name: &str, body: &str) -> Self {
        let path = self.state.dir.join(name);
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        self
    }

    /// `name` is no longer in the store.
    fn removed(self, name: &str) -> Self {
        fs::remove_dir_all(self.state.root.join(name)).unwrap();
        self
    }

    fn store(&self) -> ProfileStore {
        ProfileStore::new(self.state.root.clone())
    }

    /// Why the readings for a choice cannot be loaded, or `None` when they
    /// can.
    fn unread(&self) -> Option<NotConsidered> {
        with_reads(&self.store(), &self.state, |_| ()).err()
    }

    /// The retry after a wait whose deadline was set 60 seconds ago.
    fn retry_on(&self, from: &str, bound: &str) -> Result<(), NotConsidered> {
        self.retry_since(from, bound, now() - 60)
    }

    fn retry_since(&self, from: &str, bound: &str, since: i64) -> Result<(), NotConsidered> {
        may_retry_on(&self.store(), &self.state, &n(from), &n(bound), since)
    }
}

/// One line, opening as the supervisor's other notes do, that the recipe
/// runner's classifier does not take for a usage limit.
fn assert_not_a_limit(line: &str) {
    use crate::commands::recipe::run::limit_patterns::all_markers;
    use crate::commands::recipe::run::{FailureClass, classify_failure_text};
    let lowered = line.to_ascii_lowercase();
    for marker in all_markers() {
        assert!(!lowered.contains(marker), "{marker:?} in {line:?}");
    }
    assert_ne!(
        classify_failure_text(line).0,
        FailureClass::UsageLimit,
        "{line:?}"
    );
    assert!(line.starts_with("amplihack: "), "{line:?}");
    assert_eq!(line.lines().count(), 1, "{line:?}");
}

#[test]
fn readable_logins_are_read() {
    let w = World::new(&["cur", "a"])
        .auto_switch(true)
        .state_file("ledger.json", r#"{"version":1,"hosts":{}}"#)
        .state_file("usage.json", r#"{"version":1,"accounts":{}}"#);
    assert_eq!(w.unread(), None);
}

#[test]
fn an_unreadable_ledger_is_named_with_its_error() {
    let w = World::new(&["cur", "a"])
        .auto_switch(true)
        .state_file("ledger.json", "{");
    let why = w.unread().expect("an unreadable ledger is not swallowed");
    assert!(matches!(why, NotConsidered::LedgerUnreadable(_)), "{why:?}");
    let line = why.wait_line();
    assert!(
        line.starts_with(
            "amplihack: this step's other logins were not read for its wait \
             (reason=ledger_unreadable): the auto-switch ledger cannot be read: "
        ),
        "{line}"
    );
    assert!(
        line.contains("ledger.json"),
        "the error names the file: {line}"
    );
    assert!(
        line.ends_with("; no other login's reset can end this wait early (auto-switch)"),
        "{line}"
    );
    assert_not_a_limit(&line);
}

#[test]
fn unreadable_readings_are_named_with_their_error() {
    let w = World::new(&["cur", "a"])
        .auto_switch(true)
        .state_file("usage.json", "{");
    let why = w.unread().expect("unreadable readings are not swallowed");
    assert!(
        matches!(why, NotConsidered::ReadingsUnreadable(_)),
        "{why:?}"
    );
    let line = why.wait_line();
    assert!(
        line.starts_with(
            "amplihack: this step's other logins were not read for its wait \
             (reason=readings_unreadable): the auto-switch usage readings cannot be read: "
        ),
        "{line}"
    );
    assert!(
        line.contains("usage.json"),
        "the error names the file: {line}"
    );
    assert_not_a_limit(&line);
}

#[test]
fn auto_switch_turned_off_during_the_step_is_named() {
    let w = World::new(&["cur", "a"]).auto_switch(false);
    let why = w.unread().expect("auto-switch off is not swallowed");
    assert_eq!(why, NotConsidered::AutoSwitchOff);
    let line = why.wait_line();
    assert_eq!(
        line,
        "amplihack: this step's other logins were not read for its wait \
         (reason=auto_switch_off): auto-switch was turned off while this step ran; \
         no other login's reset can end this wait early (auto-switch)"
    );
    assert_not_a_limit(&line);
}

#[test]
fn the_ledger_is_named_before_the_readings_when_both_are_unreadable() {
    // One cause per line: the first file a choice needs is the one named.
    let w = World::new(&["cur", "a"])
        .auto_switch(true)
        .state_file("ledger.json", "{")
        .state_file("usage.json", "{");
    assert!(matches!(
        w.unread(),
        Some(NotConsidered::LedgerUnreadable(_))
    ));
}

// --- the retry after an earliest-reset wait --------------------------------------

#[test]
fn a_retry_onto_an_eligible_login_is_allowed() {
    let w = World::new(&["cur", "a"]).auto_switch(true);
    assert_eq!(w.retry_on("cur", "a"), Ok(()));
}

#[test]
fn a_retry_onto_a_login_no_longer_eligible_is_named() {
    // Logged out, or removed from the store, while the step waited; and the
    // step's own login, which `cross_login_reset` never chooses.
    let w = World::new(&["cur", "a", "l", "gone"])
        .auto_switch(true)
        .logged_out("l")
        .removed("gone");
    for bound in ["l", "gone", "cur"] {
        assert_eq!(
            w.retry_on("cur", bound),
            Err(NotConsidered::LoginNotEligible),
            "{bound}"
        );
    }
    let line = NotConsidered::LoginNotEligible.retry_line(&n("l"), &n("cur"));
    assert_eq!(
        line,
        "amplihack: the wait ended at the reset of profile \"l\", but this step reruns \
         under its own profile \"cur\" (reason=login_not_eligible): that profile was \
         excluded, leased, given another account, logged out or removed while this step \
         waited (auto-switch)"
    );
    assert_not_a_limit(&line);
}

#[test]
fn a_retry_whose_logins_cannot_be_read_is_named() {
    let ledger = World::new(&["cur", "a"])
        .auto_switch(true)
        .state_file("ledger.json", "{");
    let why = ledger.retry_on("cur", "a").unwrap_err();
    assert!(matches!(why, NotConsidered::LedgerUnreadable(_)), "{why:?}");
    let line = why.retry_line(&n("a"), &n("cur"));
    assert!(
        line.starts_with(
            "amplihack: the wait ended at the reset of profile \"a\", but this step reruns \
             under its own profile \"cur\" (reason=ledger_unreadable): the auto-switch \
             ledger cannot be read: "
        ),
        "{line}"
    );
    assert_not_a_limit(&line);
    drop(ledger);

    let readings = World::new(&["cur", "a"])
        .auto_switch(true)
        .state_file("usage.json", "{");
    let why = readings.retry_on("cur", "a").unwrap_err();
    assert!(
        matches!(why, NotConsidered::ReadingsUnreadable(_)),
        "{why:?}"
    );
    let line = why.retry_line(&n("a"), &n("cur"));
    assert!(line.contains("(reason=readings_unreadable)"), "{line}");
    assert_not_a_limit(&line);
    drop(readings);

    let off = World::new(&["cur", "a"]).auto_switch(false);
    assert_eq!(off.retry_on("cur", "a"), Err(NotConsidered::AutoSwitchOff));
    let line = NotConsidered::AutoSwitchOff.retry_line(&n("a"), &n("cur"));
    assert!(line.contains("(reason=auto_switch_off)"), "{line}");
    assert_not_a_limit(&line);
}

#[test]
fn a_retry_onto_a_login_a_reading_taken_during_the_wait_still_holds_out_is_named() {
    // The week window filled while the step waited: the reset that set the
    // deadline does not bring the login under the threshold. A reading of
    // the same figures taken before the deadline was set is what the
    // deadline was set from, so it refuses nothing.
    let t = now();
    let w = World::new(&["cur", "a"])
        .auto_switch(true)
        .read("a", taken(reading_with(10.0, 1800, 95.0, 86_400), 0))
        .saved();
    assert_eq!(
        w.retry_since("cur", "a", t - 60),
        Err(NotConsidered::LoginStillLimited)
    );
    assert_eq!(w.retry_since("cur", "a", t + 60), Ok(()));
    let line = NotConsidered::LoginStillLimited.retry_line(&n("a"), &n("cur"));
    assert_eq!(
        line,
        "amplihack: the wait ended at the reset of profile \"a\", but this step reruns \
         under its own profile \"cur\" (reason=login_still_limited): a limit block on \
         that profile, or a reading of it at or above the auto-switch threshold, came in \
         while this step waited and still holds it out (auto-switch)"
    );
    assert_not_a_limit(&line);
}

#[test]
fn a_retry_onto_a_login_a_newer_reading_shows_under_the_threshold_is_allowed() {
    let w = World::new(&["cur", "a"])
        .auto_switch(true)
        .read("a", taken(reading_with(10.0, 1800, 50.0, 86_400), 0))
        .saved();
    assert_eq!(w.retry_on("cur", "a"), Ok(()));
}

#[test]
fn a_retry_onto_a_login_blocked_during_the_wait_is_named() {
    // A block recorded after the deadline was set that still stands keeps
    // the retry off the login. One recorded before had already set the
    // deadline, and one that has ended holds nothing.
    let t = now();
    let standing = World::new(&["cur", "a"])
        .auto_switch(true)
        .blocked(&[("a", t - 30, t + 3600)]);
    assert_eq!(
        standing.retry_since("cur", "a", t - 60),
        Err(NotConsidered::LoginStillLimited)
    );
    assert_eq!(standing.retry_since("cur", "a", t - 10), Ok(()));
    drop(standing);
    let ended = World::new(&["cur", "a"])
        .auto_switch(true)
        .blocked(&[("a", t - 30, t - 5)]);
    assert_eq!(ended.retry_since("cur", "a", t - 60), Ok(()));
}

#[test]
fn a_retry_back_to_the_inherited_profile_is_a_move_and_is_noted() {
    // The step was moved off its inherited profile `inh` onto `sub` before
    // it waited, and `inh`'s reset ended the wait. The eligibility is asked
    // from `sub`, the login the limited attempt ran on, as
    // `cross_login_reset` was when it chose `inh`, so `inh` is allowed. The
    // retry's `None` is the inherited environment, which is `inh`: the step
    // does change login, and the note says so.
    let w = World::new(&["inh", "sub"]).auto_switch(true);
    assert_eq!(w.retry_on("sub", "inh"), Ok(()));
    assert_eq!(
        earliest_note(&n("sub"), &n("inh")),
        Some(format!("{NOTE_EARLIEST} \"inh\" (auto-switch)"))
    );
}

#[test]
fn the_earliest_reset_note_is_printed_only_for_a_move() {
    assert_eq!(earliest_note(&n("a"), &n("a")), None);
    let note = earliest_note(&n("cur"), &n("a")).expect("a move is noted");
    assert_eq!(
        note,
        "amplihack: the earliest reset among this step's logins has arrived; rerunning \
         this step under profile \"a\" (auto-switch)"
    );
    assert_not_a_limit(&note);
}
