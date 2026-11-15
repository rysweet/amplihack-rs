//! `profile switch NAME` refuses every profile the rotator would not swap to
//! (issue #1691, "Whatever the rotator refuses, the switch refuses as
//! well"): the skip reasons of [Rotate candidates] in
//! docs/reference/profile-command.md. Each end-to-end case builds the real
//! state on the `tests_swap.rs` fixture (a running process, an auto-switch
//! lease, the exclude list, a park marker) and asserts, through
//! `assert_refused`, that nothing changed. A running session is an entry in
//! the fixture's synthetic process table (issue #2196) with
//! `CLAUDE_CONFIG_DIR` set, found by the same scan the rotator uses, so those
//! cases are Linux only. Each one also removes the entry and checks that the
//! refusal goes with it. `not_a_candidate` is also
//! checked for every reason directly, including the ones the fixture cannot
//! reach end to end (`/proc` unreadable, a `hosting` directory without a
//! running session).
//!
//! A refusal moves nothing but still makes the target the default for new
//! launches (issue #1691, "keeps the `.default` pointer behaviour"); the last
//! section runs the whole switch with `/proc` unscannable, as on macOS.

use super::cycle::Ctx;
use super::identify::{LiveId, Skip};
use super::observe::Snap;
use super::pin::{self, PinState};
use super::switch::not_a_candidate;
use super::switch::{Switched, switch_in};
use super::tests_swap::{CREDS, Env, env, ino_at, n, private};
use super::tests_switch::assert_refused;
use super::{TuneFlags, settings};
use crate::commands::profile::auto_switch::kit::Liveness;
use crate::commands::profile::pointer;
#[cfg(target_os = "linux")]
use crate::test_support::proc::FAKE_PID_BASE;
use std::fs;
#[cfg(target_os = "linux")]
use std::os::unix::fs::PermissionsExt;

/// A fake running `claude` whose `CLAUDE_CONFIG_DIR` is profile `name`'s
/// directory, in `e`'s synthetic process table. Returns its pid, for
/// [`end_session`].
#[cfg(target_os = "linux")]
fn session_in(e: &Env, k: u32, name: &str) -> u32 {
    let pid = FAKE_PID_BASE + k;
    e.proc.running_session(pid, &e.root.join(name));
    pid
}

/// The session at `pid` exits: its entry leaves the table.
#[cfg(target_os = "linux")]
fn end_session(e: &Env, pid: u32) {
    e.proc.fake.remove(pid);
}

/// Writes the auto-switch ledger: directory → the account live in it.
#[cfg(target_os = "linux")]
fn hosts(e: &Env, entries: &[(&str, &str)]) {
    let hosts: serde_json::Map<String, serde_json::Value> = entries
        .iter()
        .map(|(dir, live)| {
            (
                (*dir).to_string(),
                serde_json::json!({"live": live, "swapped_at": 1, "pre_swap_fingerprint": null}),
            )
        })
        .collect();
    let path = e.root.join(".auto-switch.d/ledger.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    private(
        &path,
        &serde_json::json!({"version": 1, "hosts": hosts}).to_string(),
    );
}

#[cfg(target_os = "linux")]
fn ledger_bytes(e: &Env) -> Vec<u8> {
    fs::read(e.root.join(".auto-switch.d/ledger.json")).unwrap()
}

// --- end to end -------------------------------------------------------------

#[cfg(target_os = "linux")]
#[test]
fn a_profile_with_a_running_session_is_refused() {
    let e = env();
    let pid = session_in(&e, 1, "spare");
    assert_refused(
        &e,
        "spare",
        "profile spare is not a rotate candidate (session running): \
         a running process has CLAUDE_CONFIG_DIR set to it",
    );
    // Negative check: with the session gone, the same switch goes through.
    end_session(&e, pid);
    super::tests_switch::switch(&e, "spare").unwrap();
    e.assert_finished();
}

#[cfg(target_os = "linux")]
#[test]
fn a_running_session_in_another_profile_does_not_block_the_switch() {
    let e = env();
    e.store.add(&n("other")).unwrap();
    session_in(&e, 1, "other");
    super::tests_switch::switch(&e, "spare").unwrap();
    e.assert_finished();
}

#[cfg(target_os = "linux")]
#[test]
fn a_profile_whose_login_is_leased_is_refused() {
    let e = env();
    // spare's account is live in `host`, whose session is still running, so
    // auto-switch recovery leaves the lease in place.
    e.store.add(&n("host")).unwrap();
    let pid = session_in(&e, 1, "host");
    hosts(&e, &[("host", "spare")]);
    let before = ledger_bytes(&e);
    assert_refused(
        &e,
        "spare",
        "profile spare is not a rotate candidate (leased): \
         auto-switch has lent its login to another profile's session",
    );
    assert_eq!(ledger_bytes(&e), before, "the lease is untouched");
    // Negative check: once host's session has exited, recovery ends the
    // lease and the same switch goes through.
    end_session(&e, pid);
    assert_eq!(
        super::tests_switch::switch(&e, "spare").unwrap(),
        Switched::Moved {
            from: n("work"),
            to: n("spare")
        },
        "the lease held only while host's session ran"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_profile_hosting_another_login_is_refused() {
    let e = env();
    // spare's directory holds another account's login for a session running
    // in it. The rotator checks the running session first, so that is the
    // reason given; `hosting` itself is checked below.
    e.store.add(&n("other")).unwrap();
    let pid = session_in(&e, 1, "spare");
    hosts(&e, &[("spare", "other")]);
    let before = ledger_bytes(&e);
    assert_refused(&e, "spare", "(session running)");
    assert_eq!(ledger_bytes(&e), before, "the hosted login is untouched");
    // Negative check: without the session, recovery gives the hosted login
    // back to `other`, so the refusal is spare's missing login instead of
    // `session running`.
    end_session(&e, pid);
    match super::tests_switch::switch(&e, "spare").unwrap() {
        Switched::DefaultOnly { reason, .. } => {
            assert!(reason.contains("has no stored login"), "{reason}");
        }
        other => panic!("expected the no-stored-login refusal, got {other:?}"),
    }
}

#[cfg(target_os = "linux")]
#[test]
fn an_excluded_profile_is_refused() {
    let e = env();
    private(
        &e.root.join(".auto-switch"),
        "enabled=off\nthreshold=95\nexclude=spare\n",
    );
    assert_refused(
        &e,
        "spare",
        "profile spare is not a rotate candidate (excluded): \
         it is in the auto-switch exclude list",
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_parked_profile_is_refused() {
    let e = env();
    let parked = e.root.join(".auto-switch.d/parked/spare");
    fs::create_dir_all(&parked).unwrap();
    assert_refused(
        &e,
        "spare",
        "profile spare is not a rotate candidate (parked): \
         auto-switch has parked its own login",
    );
    assert!(parked.is_dir(), "the park marker is untouched");
}

#[test]
fn a_profile_without_an_account_is_refused() {
    let e = env();
    private(&e.root.join("spare/.claude.json"), "{}");
    let mut cred = super::tests_swap::json(&e.slot("spare"));
    cred.as_object_mut().unwrap().remove("task1753FakeIdentity");
    private(&e.slot("spare"), &cred.to_string());
    assert_refused(
        &e,
        "spare",
        "profile spare is not a rotate candidate (no account): \
         its trusted login identity is unavailable",
    );
}

#[test]
fn a_hard_linked_stored_login_is_refused_as_no_login() {
    let e = env();
    fs::hard_link(e.slot("spare"), e.root.join("spare-copy")).unwrap();
    assert_refused(
        &e,
        "spare",
        "profile spare is not a rotate candidate (no login): \
         a login file is not a private regular file with one link",
    );
}

// --- every reason -----------------------------------------------------------

fn snap_with(skip: Skip, sees_sessions: bool) -> Snap {
    Snap {
        live: LiveId::Known(n("work")),
        proto: None,
        account: None,
        trusted_live: None,
        identity_refusal: None,
        identity_cause: None,
        candidate_refusals: Vec::new(),
        candidates: Vec::new(),
        skips: vec![(n("work"), Skip::Live), (n("spare"), skip)],
        sees_sessions,
        overflow: Vec::new(),
        pin: PinState::None,
        ignored: Vec::new(),
        unmarked: Vec::new(),
        detected_mark: None,
        home_no_oauth: false,
        candidate_accounts: Vec::new(),
        live_excluded: false,
    }
}

#[test]
fn every_skip_reason_but_live_refuses_with_the_rotator_reason() {
    let e = env();
    for skip in [
        Skip::Refused,
        Skip::NoLogin,
        Skip::Excluded,
        Skip::SessionRunning,
        Skip::Leased,
        Skip::Hosting,
        Skip::Parked,
        Skip::NoAccount,
    ] {
        let why = not_a_candidate(&e.store, &snap_with(skip, true), &n("spare"))
            .unwrap_or_else(|| panic!("{skip:?} must refuse"));
        assert!(
            why.starts_with(&format!(
                "profile spare is not a rotate candidate ({}): ",
                skip.as_str()
            )),
            "{skip:?}: {why}"
        );
    }
    assert_eq!(
        not_a_candidate(&e.store, &snap_with(Skip::Live, true), &n("spare")),
        None
    );
}

#[test]
fn a_hosting_directory_is_refused() {
    let e = env();
    assert_eq!(
        not_a_candidate(&e.store, &snap_with(Skip::Hosting, true), &n("spare")).as_deref(),
        Some(
            "profile spare is not a rotate candidate (hosting): \
             its directory holds another account's login for auto-switch"
        )
    );
}

#[test]
fn unseen_sessions_refuse_rather_than_guess() {
    // `/proc` unreadable: the rotator marks every profile `session running`.
    let e = env();
    assert_eq!(
        not_a_candidate(
            &e.store,
            &snap_with(Skip::SessionRunning, false),
            &n("spare")
        )
        .as_deref(),
        Some(
            "profile spare is not a rotate candidate (session running): \
             running sessions cannot be seen here, so its login may be in use"
        )
    );
}

#[test]
fn a_missing_stored_login_keeps_its_login_hint() {
    let e = env();
    fs::remove_file(e.root.join("spare").join(CREDS)).unwrap();
    assert_eq!(
        not_a_candidate(&e.store, &snap_with(Skip::NoLogin, true), &n("spare")).as_deref(),
        Some("profile spare has no stored login; run `amplihack profile login spare` first")
    );
}

#[test]
fn a_profile_with_no_recorded_skip_is_a_candidate() {
    let e = env();
    let mut s = snap_with(Skip::Parked, true);
    s.skips.retain(|(name, _)| name != &n("spare"));
    assert_eq!(not_a_candidate(&e.store, &s, &n("spare")), None);
}

// --- the default for new launches survives a refused live move ---------------

/// The whole switch with `/proc` unscannable: `Liveness::Unsupported`, what
/// `scan_system` returns on macOS and every other non-Linux system.
fn switch_without_liveness(e: &Env, name: &str) -> Switched {
    let s = settings::resolve(&TuneFlags::default(), None, &|_| None).unwrap();
    let mut ctx = Ctx::new(e.store.clone(), e.home.clone(), s);
    ctx.scan = |_| Liveness::Unsupported;
    match switch_in(ctx, &n(name)) {
        Ok(Ok(s)) => s,
        Ok(Err(f)) => panic!("unexpected partial failure: {}", f.what),
        Err(err) => panic!("expected a default-only refusal, got {err:?}"),
    }
}

#[test]
fn without_liveness_the_live_login_stays_and_the_default_is_still_set() {
    // macOS: every profile counts as having a running session, so no live
    // login can be moved; `profile switch spare` must still make spare the
    // default for new launches, as it did before #1691, and hold nothing.
    let e = env();
    let live = ino_at(&e.live());
    match switch_without_liveness(&e, "spare") {
        Switched::DefaultOnly { reason, held } => {
            assert!(
                reason.contains("running sessions cannot be seen here"),
                "{reason}"
            );
            assert_eq!(held, None);
        }
        other => panic!("expected DefaultOnly, got {other:?}"),
    }
    assert_eq!(pointer::read(&e.root).unwrap(), Some(n("spare")));
    assert_eq!(pin::read(&e.rs), PinState::None, "nothing is held");
    assert_eq!(ino_at(&e.live()), live, "the live login stays");
    assert!(!e.rs.journal.exists(), "no swap was started");
}

#[test]
fn without_liveness_the_already_live_profile_is_still_switched_to() {
    // The live profile needs no move, so `/proc` does not matter: pin,
    // default and ledger record are written as on Linux.
    let e = env();
    let live = match super::tests_switch::switch(&e, "spare") {
        Ok(Switched::Moved { to, .. }) => to,
        other => panic!("{other:?}"),
    };
    pin::clear(&e.rs).unwrap();
    pointer::clear(&e.root).unwrap();
    assert_eq!(
        switch_without_liveness(&e, live.as_str()),
        Switched::AlreadyLive(live.clone())
    );
    assert_eq!(pointer::read(&e.root).unwrap(), Some(live.clone()));
    assert_eq!(pin::read(&e.rs), PinState::Pinned(live));
}

#[cfg(target_os = "linux")]
#[test]
fn a_refused_switch_keeps_an_earlier_hold_and_says_so() {
    // work is live and held by an earlier switch; spare is open in a session.
    // The default moves to spare, the hold stays on work, and the result
    // names work so the command can print it.
    let e = env();
    assert_eq!(
        super::tests_switch::switch(&e, "work").unwrap(),
        Switched::AlreadyLive(n("work"))
    );
    let pid = session_in(&e, 1, "spare");
    let held = assert_refused(&e, "spare", "(session running)");
    assert_eq!(held, Some(n("work")));
    assert_eq!(pin::read(&e.rs), PinState::Pinned(n("work")));
    assert_eq!(pointer::read(&e.root).unwrap(), Some(n("spare")));
    // Negative check: with the session gone, spare is switched to.
    end_session(&e, pid);
    assert!(
        matches!(
            super::tests_switch::switch(&e, "spare"),
            Ok(Switched::Moved { .. })
        ),
        "the refusal came from the running session"
    );
}
