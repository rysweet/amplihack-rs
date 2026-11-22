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
use super::{AfterLimit, Resumed, Waiter};
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
///
/// It also means the step is already out of its own time, and the
/// retry-before-wait return finishes on the step timeout rather than
/// retrying. A test that needs that return to be *taken* therefore uses
/// `ROOMY_STEP_TIMEOUT` and ends its waits some other way.
const IMMEDIATE_STEP_TIMEOUT: &str = "1";

/// A step timeout the step is nowhere near, for the tests that need the
/// retry-before-wait return: it is taken only while the step still has
/// time to run the retry it starts.
const ROOMY_STEP_TIMEOUT: &str = "3600";

/// Virtual seconds per real second, so a wait that has to reach its first
/// 30-second poll does so in milliseconds. Only the tests that let a wait
/// run use it; the rest keep the real clock.
const FAST: &str = "1000";

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
            // A prompt in argv: nothing on stdin for the retry to need.
            super::claude_args::ClaudeArgs {
                print: true,
                prompt: true,
                stream_stdin: false,
            },
            Box::new(|| false),
        )
    }
}

fn capacity() -> WaitFor {
    WaitFor::Capacity {
        harness: Harness::Claude,
        reset: None,
        why: super::plan::Waiting::NoReset,
    }
}

fn login_change() -> WaitFor {
    WaitFor::LoginChange {
        harness: Harness::Claude,
    }
}

/// Holds the environment lock, the step timeout and, where a wait is meant
/// to run rather than end at its deadline, the clock scale.
fn guarded_with<T>(timeout: &str, scale: Option<&str>, f: impl FnOnce() -> T) -> T {
    let _lock = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut guard = EnvGuard::set([("AMPLIHACK_STEP_TIMEOUT", timeout)]);
    for name in SILENCED {
        guard.clear(name);
    }
    if let Some(scale) = scale {
        guard.assign("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_SCALE", scale);
    }
    f()
}

/// The common case: every wait ends at once on the step timeout.
fn guarded<T>(f: impl FnOnce() -> T) -> T {
    guarded_with(IMMEDIATE_STEP_TIMEOUT, None, f)
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
        let after = w.after_limit(
            capacity(),
            1,
            &signals(),
            &|_, _| false,
            &mut || None,
            Resumed::Ordinarily,
        );
        assert!(matches!(after, AfterLimit::Finish(_)));
        assert!(
            w.retry_move().is_some(),
            "a capacity wait's retry may move: #1801"
        );
    });
}

/// Issue #2816: a retry is announced only when the step's prompt can be
/// given to it, on both of the paths that announce one.
///
/// `after_limit` asks `replay` the moment before it logs `resume`. A step
/// whose prompt came on a pipe is replayed from the copy the supervisor
/// kept (#2815), and when there is no whole copy — lost, or still arriving
/// on a pipe the caller has not closed — the retry cannot run. Announcing
/// it anyway would put `resume attempt=2` in the run log for an attempt
/// that never happens, which is the false diagnostic this wait already
/// refuses to write elsewhere. The wait ends with `Finish` and the
/// `skipped` event instead, and the caller passes the limited attempt's own
/// result up.
///
/// Both announcing paths are covered: the retry-before-wait return, taken
/// when the login changed while the attempt was failing, and the end of a
/// wait that ran.
#[test]
fn a_retry_is_not_announced_when_the_step_has_no_prompt_to_give_it() {
    // The retry-before-wait return: a roomy step timeout, so it is taken.
    guarded_with(ROOMY_STEP_TIMEOUT, None, || {
        let e = env();
        e.rotator("a");
        let mut w = e.waiter();
        w.note_attempt_login();
        e.rotator("b"); // the login changed while the attempt was failing
        let mut asked = 0;
        let after = w.after_limit(
            login_change(),
            2,
            &signals(),
            &|_, _| false,
            &mut || {
                asked += 1;
                Some("this step's prompt is still arriving".to_string())
            },
            Resumed::Ordinarily,
        );
        assert_eq!(asked, 1, "the question is asked exactly once");
        assert!(
            matches!(after, AfterLimit::Finish(None)),
            "no retry is announced for an attempt that cannot run"
        );
    });
    // A wait that ran to its own deadline and would otherwise retry.
    guarded(|| {
        let e = env();
        e.rotator("a");
        let mut w = e.waiter();
        w.note_attempt_login();
        let after = w.after_limit(
            capacity(),
            1,
            &signals(),
            &|_, _| false,
            &mut || Some("the copy of this step's prompt could not be written".to_string()),
            Resumed::Ordinarily,
        );
        assert!(
            matches!(after, AfterLimit::Finish(None)),
            "no retry is announced after a wait with no prompt to replay"
        );
    });
}

/// The deputy's point 2. The second wait never runs, because the login
/// changed while the attempt was failing, and the step retries at once. The
/// first wait's `RetryMove` must not be what chooses the login that retry
/// runs on.
#[test]
fn the_retry_before_a_wait_carries_no_earlier_waits_retry_move() {
    // The step keeps time in hand throughout, so the return under test is
    // reached on its own merits rather than being pre-empted by the step
    // timeout, and the first wait ends on a reading instead of a deadline.
    guarded_with(ROOMY_STEP_TIMEOUT, Some(FAST), || {
        let e = env();
        e.rotator("a");
        let mut w = e.waiter();

        // First wait: a capacity wait on login `a`, ended at its first poll
        // by a login with room, which is what sets the field.
        w.note_attempt_login();
        let _ = w.after_limit(
            capacity(),
            1,
            &signals(),
            &|_, _| true,
            &mut || None,
            Resumed::Ordinarily,
        );
        assert!(w.retry_move().is_some(), "the state this test starts from");

        // The attempt that follows is launched on `b` and, while it is
        // failing, the rotator moves `~/.claude` to `c`.
        e.rotator("b");
        w.note_attempt_login();
        e.rotator("c");

        let after = w.after_limit(
            login_change(),
            2,
            &signals(),
            &|_, _| false,
            &mut || None,
            Resumed::Ordinarily,
        );
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
        let _ = w.after_limit(
            capacity(),
            1,
            &signals(),
            &|_, _| false,
            &mut || None,
            Resumed::Ordinarily,
        );
        assert!(w.retry_move().is_some(), "the state this test starts from");

        // A second wait on another login, this time a login-change wait that
        // reaches its deadline: no capacity wait, and no reading ended it.
        e.rotator("b");
        w.note_attempt_login();
        let _ = w.after_limit(
            login_change(),
            2,
            &signals(),
            &|_, _| false,
            &mut || None,
            Resumed::Ordinarily,
        );
        assert!(
            w.retry_move().is_none(),
            "nothing about this wait says the retry may move"
        );
    });
}

/// The clear is about `retry_move` alone. The mark is a different record,
/// answering a different question, and the early return records it like any
/// other wait.
///
/// Issue #2911: what the mark answers has changed. It used to refuse a
/// second wait on a login the step had already waited on, which ended the
/// chain the moment the rotator came back round; `reset_too_far` and
/// `already_waited` between them account for most of the runs #2911
/// reports. The refusal is gone, so a login that comes round twice is
/// waited on twice. The record itself stays, because two readers still want
/// it: `used`, which stops auto-switch choosing another candidate once the
/// step has waited, and the `reentered` line, which says whether this step
/// has waited on this login before.
#[test]
fn the_wait_that_returns_early_still_records_its_mark() {
    // No wait runs here at all: both calls take the early return, so the
    // step needs time left for it to be taken.
    guarded_with(ROOMY_STEP_TIMEOUT, None, || {
        let e = env();
        e.rotator("a");
        let mut w = e.waiter();
        w.note_attempt_login();
        e.rotator("b");
        assert!(matches!(
            w.after_limit(
                login_change(),
                1,
                &signals(),
                &|_, _| false,
                &mut || None,
                Resumed::Ordinarily
            ),
            AfterLimit::Retry
        ));
        assert!(
            w.used(),
            "the step has waited, as far as auto-switch reads it"
        );
        assert!(
            w.marks.waited_on(&Some("a".to_string())),
            "the early return recorded the login it returned on"
        );

        // The rotator comes back round to `a`, which this step has already
        // waited on once. Before #2911 that ended the step; now it waits
        // again, and the record of the first wait is what the `reentered`
        // line reads rather than a refusal.
        e.rotator("a");
        w.note_attempt_login();
        e.rotator("b");
        assert!(
            matches!(
                w.after_limit(
                    login_change(),
                    2,
                    &signals(),
                    &|_, _| false,
                    &mut || None,
                    Resumed::Ordinarily
                ),
                AfterLimit::Retry
            ),
            "a login that comes round twice is waited on twice (#2911)"
        );
        assert!(
            w.marks.waited_on(&Some("a".to_string())),
            "and the second wait on it is recorded too"
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

        let after = w.after_limit(
            login_change(),
            1,
            &signals(),
            &|_, _| false,
            &mut || None,
            Resumed::Ordinarily,
        );
        assert!(
            matches!(after, AfterLimit::Finish(_)),
            "the readings are not comparable, so the step waits as it would \
             have: this wait reaches its deadline and ends the step"
        );
    });
}
