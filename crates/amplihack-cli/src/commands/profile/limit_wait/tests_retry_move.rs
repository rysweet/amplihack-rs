//! Issue #2731, the deputy's point 2: `retry_move` is reset at the entry of
//! every wait, the retry-before-wait return included.
//!
//! `RetryMove` (#1801, #2694) says whether the retry after a wait may run on
//! another login, and from which instant a capacity reading is fresh enough
//! to choose one. Before #2731 a step waited at most once, so the field was
//! written exactly once and the question of a stale value could not arise.
//! #2731 lets a `$HOME/.claude` step wait once per login it runs on, so the
//! same `Waiter` now serves several waits, and one of the new paths returns
//! `Retry` without running a wait at all: the login changed while the
//! attempt was failing, so there is nothing left to wait for.
//!
//! A value left behind by an earlier wait would then pick the login of a
//! retry that earned no move. That is not a cosmetic error. `retry_move`
//! carries the freshness floor a reading must clear, and the earlier wait's
//! floor is older, so it admits readings taken before the current limit was
//! hit and can put the step onto an account the user did not select.
//!
//! The field is therefore cleared as soon as the plan is chosen and the mark
//! recorded, which is above the early return, and is written again only by a
//! wait that actually ran.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_retry_move;`.

use super::fingerprint::Watch;
use super::plan::WaitFor;
use super::settings::WaitSettings;
use super::{AfterLimit, Waiter};
use crate::commands::profile::attempt::Signals;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use crate::commands::recipe::run::limit_patterns::Harness;
use crate::test_support::{EnvGuard, env_lock};
use std::fs;
use std::os::unix::fs::PermissionsExt;

/// A step timeout of one second puts every plan's deadline at `now`, so each
/// wait here ends the instant it begins and nothing sleeps. What is under
/// test is the order of the writes to `retry_move`, not the waiting.
const IMMEDIATE_STEP_TIMEOUT: &str = "1";

/// The variables that would send a wait's own records outside the test's
/// temporary tree, or put it on a virtual clock. Cleared for the test.
const SILENCED: [&str; 4] = [
    "AMPLIHACK_USAGE_WAIT_LEDGER",
    "AMPLIHACK_RECIPE_LOG",
    "AMPLIHACK_USAGE_LIMIT_WAIT_TEST_NOW",
    "AMPLIHACK_USAGE_LIMIT_WAIT_TEST_SCALE",
];

struct Env {
    _tmp: tempfile::TempDir,
    home: std::path::PathBuf,
    store: ProfileStore,
}

/// A `$HOME/.claude` step's world: a profile store with two logins and a
/// rotate ledger the test moves between them. Everything is inside one
/// tempfile root; nothing here reads or writes the real `~/.claude` or
/// `~/.amplihack`.
fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().to_path_buf();
    let home = base.join("home");
    fs::create_dir_all(home.join(".claude")).unwrap();
    let store = ProfileStore::new(base.join("root"));
    for n in ["a", "b", "c"] {
        store.add(&ProfileName::parse(n).unwrap()).unwrap();
    }
    Env {
        _tmp: tmp,
        home,
        store,
    }
}

impl Env {
    /// Writes the rotate ledger naming `live`, as the rotator does.
    fn rotator(&self, live: &str) {
        let dir = self.store.root().join(".auto-switch.d").join("rotate");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ledger.json");
        let tmp = path.with_extension("new");
        fs::write(
            &tmp,
            format!(
                r#"{{"version":1,"live":"{live}","alert":null,{}}}"#,
                r#""last_cycle":{"at":1,"outcome":"stayed"}"#
            ),
        )
        .unwrap();
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600)).unwrap();
        fs::rename(&tmp, &path).unwrap();
    }

    fn waiter(&self) -> Waiter {
        let watch = Watch::new(Some(&self.store), None, None, Some(self.home.as_path()));
        assert!(watch.is_home(), "the test needs a $HOME/.claude step");
        Waiter::new(
            Some(WaitSettings {
                enabled: true,
                // No budget cap, so a second wait is never refused for one.
                max_wait_secs: None,
                off_in_ci: false,
            }),
            watch,
            true,
            Box::new(|| false),
            Box::new(|| true),
        )
    }
}

fn capacity() -> WaitFor {
    WaitFor::Capacity {
        harness: Harness::Claude,
        reset: None,
    }
}

fn login_change() -> WaitFor {
    WaitFor::LoginChange {
        harness: Harness::Claude,
    }
}

/// Holds the environment lock and the step timeout for one test.
fn guarded<T>(f: impl FnOnce() -> T) -> T {
    let _lock = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut guard = EnvGuard::set([("AMPLIHACK_STEP_TIMEOUT", IMMEDIATE_STEP_TIMEOUT)]);
    for name in SILENCED {
        guard.clear(name);
    }
    f()
}

fn signals() -> Signals {
    Signals::register().expect("the test process can register stop signals")
}

/// A capacity wait leaves a `RetryMove` behind, which is #1801's and #2717's
/// behaviour and the starting state for the cases below. Without it the two
/// tests that follow would pass on a `Waiter` that never set the field at
/// all.
#[test]
fn a_capacity_wait_still_leaves_a_retry_move() {
    guarded(|| {
        let e = env();
        e.rotator("a");
        let mut w = e.waiter();
        w.note_attempt_login();
        let after = w.after_limit(capacity(), 1, &signals(), &|_, _| false);
        assert!(matches!(after, AfterLimit::Finish(_)));
        assert!(
            w.retry_move().is_some(),
            "a capacity wait's retry may move: #1801"
        );
    });
}

/// The deputy's point 2. The second wait never runs, because the login
/// changed while the attempt was failing, and the step retries at once. The
/// first wait's `RetryMove` must not be what chooses the login that retry
/// runs on.
#[test]
fn the_retry_before_a_wait_carries_no_earlier_waits_retry_move() {
    guarded(|| {
        let e = env();
        e.rotator("a");
        let mut w = e.waiter();

        // First wait: a capacity wait on login `a`, which sets the field.
        w.note_attempt_login();
        let _ = w.after_limit(capacity(), 1, &signals(), &|_, _| false);
        assert!(w.retry_move().is_some(), "the state this test starts from");

        // The attempt that follows is launched on `b` and, while it is
        // failing, the rotator moves `~/.claude` to `c`.
        e.rotator("b");
        w.note_attempt_login();
        e.rotator("c");

        let after = w.after_limit(login_change(), 2, &signals(), &|_, _| false);
        assert!(
            matches!(after, AfterLimit::Retry),
            "the login already changed: there is nothing left to wait for"
        );
        assert!(
            w.retry_move().is_none(),
            "this retry runs on the login the rotator put in place, and the \
             earlier wait's freshness floor must not choose one for it"
        );
    });
}

/// The same rule for the wait that does run: a second wait that ends without
/// earning a move clears what the first left, rather than inheriting it.
#[test]
fn a_later_wait_that_earns_no_move_clears_the_earlier_ones() {
    guarded(|| {
        let e = env();
        e.rotator("a");
        let mut w = e.waiter();
        w.note_attempt_login();
        let _ = w.after_limit(capacity(), 1, &signals(), &|_, _| false);
        assert!(w.retry_move().is_some(), "the state this test starts from");

        // A second wait on another login, this time a login-change wait that
        // reaches its deadline: no capacity wait, and no reading ended it.
        e.rotator("b");
        w.note_attempt_login();
        let _ = w.after_limit(login_change(), 2, &signals(), &|_, _| false);
        assert!(
            w.retry_move().is_none(),
            "nothing about this wait says the retry may move"
        );
    });
}

/// The clear is about `retry_move` alone. The mark is a different record,
/// answering a different question (*may this step wait again*), and the
/// early return records it like any other wait: a login the step has
/// already waited on must not earn a second wait later in the chain.
#[test]
fn the_wait_that_returns_early_still_records_its_mark() {
    guarded(|| {
        let e = env();
        e.rotator("a");
        let mut w = e.waiter();
        w.note_attempt_login();
        e.rotator("b");
        assert!(matches!(
            w.after_limit(login_change(), 1, &signals(), &|_, _| false),
            AfterLimit::Retry
        ));
        assert!(
            w.used(),
            "the step has waited, as far as auto-switch reads it"
        );

        // The rotator comes back round to `a`, which this step has already
        // waited on once.
        e.rotator("a");
        w.note_attempt_login();
        e.rotator("b");
        assert!(
            matches!(
                w.after_limit(login_change(), 2, &signals(), &|_, _| false),
                AfterLimit::Finish(_)
            ),
            "a login that comes round twice ends the chain"
        );
    });
}

/// `Watch::login` is read twice, once before the attempt and once after its
/// limit, and a change counts only when the two readings came from the same
/// file. A swap record left over from an older swap must not end the wait
/// before it starts.
#[test]
fn a_fallback_reading_does_not_start_the_retry_before_the_wait() {
    guarded(|| {
        let e = env();
        e.rotator("a");
        // A swap record naming a different login, which is what the reader
        // falls back to once the ledger cannot be read.
        let dir = e.store.root().join(".auto-switch.d").join("rotate");
        fs::create_dir_all(&dir).unwrap();
        let swap = dir.join("swap.json");
        fs::write(
            &swap,
            r#"{"seq":1,"live_login":"b","from_login":"a","swapped_at":1,
                "reading":{"session":12,"week":40,"measured_at":1}}"#,
        )
        .unwrap();
        fs::set_permissions(&swap, fs::Permissions::from_mode(0o600)).unwrap();

        let mut w = e.waiter();
        w.note_attempt_login();
        // The ledger stops being readable: the second reading falls back to
        // the swap record and names `b`, with no swap in between.
        fs::set_permissions(dir.join("ledger.json"), fs::Permissions::from_mode(0o622)).unwrap();

        let after = w.after_limit(login_change(), 1, &signals(), &|_, _| false);
        assert!(
            matches!(after, AfterLimit::Finish(_)),
            "the readings are not comparable, so the step waits as it would \
             have: this wait reaches its deadline and ends the step"
        );
    });
}
