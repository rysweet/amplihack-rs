//! What one step's record of its waits still says (issues #2731, #2911).
//!
//! [`Marks`] used to bound a chain of waits with two refusals: a login the
//! step had already waited on, and a spent `max_wait_secs` budget. Both
//! ended runs that a limit should only have delayed, and both are gone
//! (#2911). The tests that proved them are gone with them; what bounds the
//! chain now is that every wait costs time, which is `plan`'s invariant and
//! is tested there and in `tests_wait_ends.rs`.
//!
//! One reader is left, and this file is its contract: [`Marks::waited_on`],
//! which the `reentered` line reads to tell a second wait on the same login
//! from a first wait on a new one. The other, `Marks::used`, which
//! `auto_switch::rerun` read to stop choosing another login once the step
//! had waited, went with issue #2967: a move to a login that can run the
//! step is not a wait, so a step that has waited may still make one.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_marks;`.

use super::marks::Marks;

fn key(n: &str) -> Option<String> {
    Some(n.to_string())
}

/// No login name could be read: a step with no profile store, or a ledger
/// that could not be read at that moment.
const NONE: Option<String> = None;

// --- `waited_on`: has this step waited on *this* login? ------------------

#[test]
fn waited_on_reports_a_login_this_step_has_waited_on() {
    let mut m = Marks::new();
    assert!(!m.waited_on(&key("a")), "nothing waited on yet");
    m.record(key("a"));
    assert!(m.waited_on(&key("a")));
    assert!(!m.waited_on(&key("b")), "a login not waited on");
}

#[test]
fn an_unreadable_name_is_its_own_mark_and_not_any_login() {
    let mut m = Marks::new();
    m.record(key("a"));
    assert!(!m.waited_on(&NONE), "an absent name is not login a");
    let mut m = Marks::new();
    m.record(NONE);
    assert!(!m.waited_on(&key("a")), "nor is login a an absent name");
    assert!(m.waited_on(&NONE));
}

#[test]
fn every_mark_is_kept_so_a_login_that_comes_round_again_is_recognised() {
    // A rotator oscillating a -> b -> a is the case the record exists for:
    // the step waits on `a` a second time, and the `reentered` line can say
    // so, because the first mark was never dropped.
    let mut m = Marks::new();
    m.record(key("a"));
    m.record(key("b"));
    assert!(m.waited_on(&key("a")));
    assert!(m.waited_on(&key("b")));
    assert!(!m.waited_on(&key("c")));
}

// --- issue #2911: the record refuses nothing ------------------------------

#[test]
fn issue_2911_recording_the_same_login_again_is_allowed_and_kept() {
    // Nothing here can refuse a wait any more. A step meeting limit after
    // limit on one login records a mark for each, and `waited_on` goes on
    // answering its own question.
    let mut m = Marks::new();
    for _ in 0..3 {
        m.record(key("a"));
        assert!(m.waited_on(&key("a")));
    }
    assert!(!m.waited_on(&key("b")), "still only login a");
}

#[test]
fn issue_2911_a_step_with_no_readable_login_name_waits_as_often_as_it_meets_a_limit() {
    // A step with no profile store records one `no login name` mark per
    // wait. Before #2911 the second such wait was refused and the step
    // ended; now there is nothing here to refuse it.
    let mut m = Marks::new();
    m.record(NONE);
    assert!(m.waited_on(&NONE));
    m.record(NONE);
    assert!(m.waited_on(&NONE));
}

// --- Issue #2967: a profile step's mark is its own login ---------------------
//
// `Waiter::note_attempt_login(profile: Option<&ProfileName>)` stores the name
// `attempt_profile(profile)` resolves: the profile the supervisor launched the
// attempt under, else the inherited one, else the rotator's live login for a
// `$HOME/.claude` step. `mark()` returns that name, so the `reentered` line
// tells logins apart, and the wait line's `account=` names it.
// Before #2967 a profile step had no `attempt_login` (that key is read only
// from the rotate ledger of a Home step), so its mark was the shared `None`
// and its wait line said `account=unknown`.
// `attempt_login` keeps its `(Source, String)` pair for the login-change check.

mod profile_marks {
    use super::super::fingerprint::Watch;
    use super::super::settings::WaitSettings;
    use super::super::{Waiter, claude_args::ClaudeArgs};
    use crate::commands::profile::name::ProfileName;
    use crate::commands::profile::store::ProfileStore;

    fn n(s: &str) -> ProfileName {
        ProfileName::parse(s).unwrap()
    }

    fn waiter(tmp: &tempfile::TempDir, inherited: Option<&str>) -> Waiter {
        let store = ProfileStore::new(tmp.path().join("root"));
        for name in ["lab", "work"] {
            store.add(&n(name)).unwrap();
        }
        let inherited = inherited.map(n);
        let watch = Watch::new(Some(&store), inherited.as_ref(), None, None);
        Waiter::new(
            Some(WaitSettings { enabled: true }),
            watch,
            ClaudeArgs {
                print: true,
                prompt: true,
                session: false,
            },
            Box::new(|| false),
        )
    }

    #[test]
    fn an_inherited_profile_step_is_marked_with_its_own_login() {
        let tmp = tempfile::tempdir().unwrap();
        let mut w = waiter(&tmp, Some("lab"));
        w.note_attempt_login(None);
        assert_eq!(w.mark().as_deref(), Some("lab"));
    }

    #[test]
    fn the_profile_the_supervisor_launched_under_wins_over_the_inherited_one() {
        let tmp = tempfile::tempdir().unwrap();
        let mut w = waiter(&tmp, Some("lab"));
        w.note_attempt_login(Some(&n("work")));
        assert_eq!(w.mark().as_deref(), Some("work"));
        // The next attempt runs on the inherited login again.
        w.note_attempt_login(None);
        assert_eq!(w.mark().as_deref(), Some("lab"));
    }

    #[test]
    fn a_step_with_no_profile_and_no_live_login_keeps_the_unnamed_mark() {
        let tmp = tempfile::tempdir().unwrap();
        let mut w = waiter(&tmp, None);
        w.note_attempt_login(None);
        assert_eq!(w.mark(), None);
    }

    #[test]
    fn a_wait_on_one_named_login_is_not_a_wait_on_another() {
        let mut m = super::Marks::new();
        m.record(Some("lab".into()));
        assert!(m.waited_on(&Some("lab".into())));
        assert!(!m.waited_on(&Some("work".into())));
    }
}
