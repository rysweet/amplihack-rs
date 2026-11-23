//! Issue #2967: a step whose other logins cannot be considered says so. The
//! supervisor's own glue is tested here: `rerun::earliest_other` before a wait
//! and `rerun::bound_profile` after an earliest-reset wait. They are run
//! against a temporary store, with a real `Waiter` writing to a temporary
//! recipe log.
//!
//! Before this, `with_reads` turned an unreadable ledger, unreadable readings
//! and auto-switch turned off during the step into a bare `None`. The wait
//! then went on to the step's own reset, which is the quiet two-hour wait of
//! #2967, and nothing in either log told that apart from "no other login had
//! a reset ahead". The retry after an earliest-reset wait fell back to the
//! step's own login just as quietly. Each case now writes its cause to the
//! run's recipe log as well as to stderr. The stderr sentence itself is
//! pinned in `tests_limit_choice`, and the whole path in
//! `tests/issue_2967_other_logins.rs`. This file asserts what the step
//! decides and the recipe-log line, which a test in this process can read
//! back.
//!
//! Wired from `rerun.rs` as a child module, so it reaches the private
//! functions it tests.

use super::{Switch, bound_profile, earliest_other};
use crate::commands::profile::auto_switch::config::TOGGLE_ENV;
use crate::commands::profile::auto_switch::state::State;
use crate::commands::profile::limit_wait::claude_args::ClaudeArgs;
use crate::commands::profile::limit_wait::fingerprint::Watch;
use crate::commands::profile::limit_wait::{RECIPE_LOG_ENV, WaitSettings, Waiter};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

/// A store of `names`, each holding its own login, and a recipe log.
struct World {
    _lock: std::sync::MutexGuard<'static, ()>,
    env: crate::test_support::EnvGuard,
    tmp: tempfile::TempDir,
    store: ProfileStore,
    log: PathBuf,
}

impl World {
    fn new(names: &[&str]) -> Self {
        let lock = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let root = tmp.path().join("root");
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let home = tmp.path().join("home");
        fs::create_dir(&home).unwrap();
        let log = tmp.path().join("recipe.log");
        let mut env = crate::test_support::EnvGuard::capture(&[]);
        env.assign("HOME", &home);
        env.assign("AMPLIHACK_CLAUDE_PROFILE_ROOT", &root);
        env.assign(TOGGLE_ENV, "1");
        env.assign(RECIPE_LOG_ENV, &log);
        for key in [
            "AMPLIHACK_ROTATE_OVERFLOW_WAIT",
            "AMPLIHACK_ROTATE_OVERFLOW_UNKNOWN_RESET",
            "AMPLIHACK_ROTATE_SESSION_AT",
            "AMPLIHACK_ROTATE_WEEK_AT",
        ] {
            env.clear(key);
        }
        let store = ProfileStore::new(root.clone());
        let state = State::new(&root).unwrap();
        for name in names {
            let dir = root.join(name);
            fs::create_dir(&dir).unwrap();
            fs::write(dir.join(".credentials.json"), "synthetic-login").unwrap();
            fs::write(
                dir.join(".claude.json"),
                serde_json::json!({
                    "oauthAccount": {
                        "accountUuid": format!("u-{name}"),
                        "organizationUuid": "org-a"
                    }
                })
                .to_string(),
            )
            .unwrap();
            state
                .write_sub_json(
                    "expected",
                    name,
                    &serde_json::json!({"account": format!("u-{name}"), "org": "org-a"}),
                )
                .unwrap();
        }
        Self {
            _lock: lock,
            env,
            tmp,
            store,
            log,
        }
    }

    fn root(&self) -> PathBuf {
        self.tmp.path().join("root")
    }

    /// Writes `body` as the auto-switch state file `name`, private as the
    /// state requires, so only its content can make it unreadable.
    fn state_file(&self, name: &str, body: &str) {
        let path = self.root().join(".auto-switch.d").join(name);
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn auto_switch_off(&mut self) {
        self.env.assign(TOGGLE_ENV, "0");
    }

    fn switch(&self, inherited: &str) -> Switch<'_> {
        Switch {
            store: &self.store,
            state: State::new(&self.root()).unwrap(),
            inherited: n(inherited),
        }
    }

    /// The waiter of a step that inherited `inherited` and whose limited
    /// attempt ran under `from` (`None`: the inherited profile).
    fn waiter(&self, inherited: &str, from: Option<&str>, wait_on: bool) -> Waiter {
        let inherited = n(inherited);
        let watch = Watch::new(Some(&self.store), Some(&inherited), None, None);
        let mut w = Waiter::new(
            Some(WaitSettings { enabled: wait_on }),
            watch,
            ClaudeArgs {
                print: true,
                prompt: true,
            },
            Box::new(|| false),
        );
        w.note_attempt_login(from.map(n).as_ref());
        w
    }

    /// The lines the step wrote to the run's recipe log.
    fn log_lines(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

const UNREAD: &str = "amplihack.recipe.usage_limit_wait other_logins_unread";
const NOT_MOVED: &str = "amplihack.recipe.usage_limit_wait retry_not_moved";

// --- before the wait: earliest_other -------------------------------------------

#[test]
fn an_unreadable_ledger_before_a_wait_is_written_to_the_recipe_log() {
    let w = World::new(&["cur", "a"]);
    w.state_file("ledger.json", "{");
    let sw = w.switch("cur");
    let waiter = w.waiter("cur", None, true);
    assert_eq!(earliest_other(Some(&sw), &waiter, None, &[]), None);
    assert_eq!(
        w.log_lines(),
        [format!("{UNREAD} reason=ledger_unreadable account=cur")]
    );
}

#[test]
fn unreadable_readings_before_a_wait_are_written_to_the_recipe_log() {
    let w = World::new(&["cur", "a"]);
    w.state_file("usage.json", "{");
    let sw = w.switch("cur");
    let waiter = w.waiter("cur", None, true);
    assert_eq!(earliest_other(Some(&sw), &waiter, None, &[]), None);
    assert_eq!(
        w.log_lines(),
        [format!("{UNREAD} reason=readings_unreadable account=cur")]
    );
}

#[test]
fn auto_switch_turned_off_before_a_wait_is_written_to_the_recipe_log() {
    // Turned off after the step started: the `Switch` was made with it on,
    // and the choice reads the setting again when it is made.
    let mut w = World::new(&["cur", "a"]);
    w.auto_switch_off();
    let sw = w.switch("cur");
    // The account is the login the limited attempt ran on.
    let waiter = w.waiter("cur", Some("a"), true);
    assert_eq!(earliest_other(Some(&sw), &waiter, Some(&n("a")), &[]), None);
    assert_eq!(
        w.log_lines(),
        [format!("{UNREAD} reason=auto_switch_off account=a")]
    );
}

#[test]
fn readable_logins_with_no_reset_ahead_write_nothing() {
    // The one quiet `None`: nothing to report, and now the log tells it
    // apart from the three above.
    let w = World::new(&["cur", "a"]);
    let sw = w.switch("cur");
    let waiter = w.waiter("cur", None, true);
    assert_eq!(earliest_other(Some(&sw), &waiter, None, &[]), None);
    assert_eq!(w.log_lines(), Vec::<String>::new());
}

#[test]
fn a_step_whose_wait_is_off_reads_nothing_and_writes_nothing() {
    // No wait, so nothing to bound. The limit's own ending is written by
    // `after_limit` (`skipped reason=disabled`).
    let w = World::new(&["cur", "a"]);
    w.state_file("ledger.json", "{");
    let sw = w.switch("cur");
    let waiter = w.waiter("cur", None, false);
    assert_eq!(earliest_other(Some(&sw), &waiter, None, &[]), None);
    assert_eq!(w.log_lines(), Vec::<String>::new());
}

#[test]
fn a_step_that_started_without_auto_switch_writes_nothing() {
    // No `Switch`: the operator's standing instruction, and no other login
    // was ever in play.
    let w = World::new(&["cur", "a"]);
    let waiter = w.waiter("cur", None, true);
    assert_eq!(earliest_other(None, &waiter, None, &[]), None);
    assert_eq!(w.log_lines(), Vec::<String>::new());
}

// --- after an earliest-reset wait: bound_profile --------------------------------

#[test]
fn a_retry_onto_an_eligible_login_moves_and_writes_nothing_to_the_recipe_log() {
    let w = World::new(&["cur", "a"]);
    let sw = w.switch("cur");
    let waiter = w.waiter("cur", None, true);
    assert_eq!(
        bound_profile(&sw, &waiter, &n("cur"), &n("a")),
        Some(n("a"))
    );
    assert_eq!(w.log_lines(), Vec::<String>::new());
}

#[test]
fn a_retry_onto_a_login_logged_out_during_the_wait_stays_and_says_why() {
    let w = World::new(&["cur", "a"]);
    let sw = w.switch("cur");
    let waiter = w.waiter("cur", None, true);
    fs::remove_file(w.root().join("a").join(".credentials.json")).unwrap();
    assert_eq!(bound_profile(&sw, &waiter, &n("cur"), &n("a")), None);
    assert_eq!(
        w.log_lines(),
        [format!("{NOT_MOVED} reason=login_not_eligible account=cur")]
    );
}

#[test]
fn a_retry_onto_a_login_removed_during_the_wait_stays_and_says_why() {
    let w = World::new(&["cur", "a"]);
    let sw = w.switch("cur");
    let waiter = w.waiter("cur", None, true);
    fs::remove_dir_all(w.root().join("a")).unwrap();
    assert_eq!(bound_profile(&sw, &waiter, &n("cur"), &n("a")), None);
    assert_eq!(
        w.log_lines(),
        [format!("{NOT_MOVED} reason=login_not_eligible account=cur")]
    );
}

#[test]
fn a_retry_whose_ledger_became_unreadable_stays_and_says_why() {
    let w = World::new(&["cur", "a"]);
    let sw = w.switch("cur");
    let waiter = w.waiter("cur", None, true);
    w.state_file("ledger.json", "{");
    assert_eq!(bound_profile(&sw, &waiter, &n("cur"), &n("a")), None);
    assert_eq!(
        w.log_lines(),
        [format!("{NOT_MOVED} reason=ledger_unreadable account=cur")]
    );
}

#[test]
fn a_retry_whose_readings_became_unreadable_stays_and_says_why() {
    let w = World::new(&["cur", "a"]);
    let sw = w.switch("cur");
    let waiter = w.waiter("cur", None, true);
    w.state_file("usage.json", "{");
    assert_eq!(bound_profile(&sw, &waiter, &n("cur"), &n("a")), None);
    assert_eq!(
        w.log_lines(),
        [format!(
            "{NOT_MOVED} reason=readings_unreadable account=cur"
        )]
    );
}

#[test]
fn a_retry_after_auto_switch_was_turned_off_stays_and_says_why() {
    let mut w = World::new(&["cur", "a"]);
    w.auto_switch_off();
    let sw = w.switch("cur");
    let waiter = w.waiter("cur", None, true);
    assert_eq!(bound_profile(&sw, &waiter, &n("cur"), &n("a")), None);
    assert_eq!(
        w.log_lines(),
        [format!("{NOT_MOVED} reason=auto_switch_off account=cur")]
    );
}

#[test]
fn a_retry_back_to_the_inherited_profile_is_a_move_not_a_fallback() {
    // The step inherited `cur`, was moved to `sub` before it waited, and
    // `cur`'s reset ended the wait. `None` is the inherited environment,
    // which is `cur`: the step goes back to it, so nothing is written to the
    // recipe log, and the note on stderr names `cur` as the profile it moves
    // to. Before this change, the eligibility was asked from `cur` itself,
    // which refused `cur`, so the same `None` came back with no line at all.
    let w = World::new(&["cur", "sub"]);
    let sw = w.switch("cur");
    let waiter = w.waiter("cur", Some("sub"), true);
    assert_eq!(bound_profile(&sw, &waiter, &n("sub"), &n("cur")), None);
    assert_eq!(w.log_lines(), Vec::<String>::new());
}
