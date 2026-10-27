//! `profile switch NAME` refuses every profile the rotator would not swap to
//! (issue #1691, "Whatever the rotator refuses, the switch refuses as
//! well"): the skip reasons of [Rotate candidates] in
//! docs/reference/profile-command.md. Each end-to-end case builds the real
//! state on the `tests_swap.rs` fixture (a running process, an auto-switch
//! lease, the exclude list, a park marker) and asserts, through
//! `assert_refused`, that nothing changed. A running session is a real child
//! process with `CLAUDE_CONFIG_DIR` set, found by the same `/proc` scan the
//! rotator uses, so those cases are Linux only. `not_a_candidate` is also
//! checked for every reason directly, including the ones the fixture cannot
//! reach end to end (`/proc` unreadable, a `hosting` directory without a
//! running session).

use super::identify::{LiveId, Skip};
use super::observe::Snap;
use super::pin::PinState;
use super::switch::not_a_candidate;
use super::tests_swap::{CREDS, Env, env, n, private};
use super::tests_switch::assert_refused;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// A child process whose environment names `dir` as its Claude config
/// directory; killed when dropped.
struct Session(std::process::Child);

impl Session {
    fn in_dir(dir: &Path) -> Self {
        Self(
            std::process::Command::new("sleep")
                .arg("600")
                .env("CLAUDE_CONFIG_DIR", dir)
                .spawn()
                .expect("spawn sleep"),
        )
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Writes the auto-switch ledger: directory → the account live in it.
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

fn ledger_bytes(e: &Env) -> Vec<u8> {
    fs::read(e.root.join(".auto-switch.d/ledger.json")).unwrap()
}

// --- end to end -------------------------------------------------------------

#[cfg(target_os = "linux")]
#[test]
fn a_profile_with_a_running_session_is_refused() {
    let e = env();
    let _s = Session::in_dir(&e.root.join("spare"));
    assert_refused(
        &e,
        "spare",
        "profile spare is not a rotate candidate (session running): \
         a running process has CLAUDE_CONFIG_DIR set to it",
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_running_session_in_another_profile_does_not_block_the_switch() {
    let e = env();
    e.store.add(&n("other")).unwrap();
    let _s = Session::in_dir(&e.root.join("other"));
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
    let _s = Session::in_dir(&e.root.join("host"));
    hosts(&e, &[("host", "spare")]);
    let before = ledger_bytes(&e);
    assert_refused(
        &e,
        "spare",
        "profile spare is not a rotate candidate (leased): \
         auto-switch has lent its login to another profile's session",
    );
    assert_eq!(ledger_bytes(&e), before, "the lease is untouched");
}

#[cfg(target_os = "linux")]
#[test]
fn a_profile_hosting_another_login_is_refused() {
    let e = env();
    // spare's directory holds another account's login for a session running
    // in it. The rotator checks the running session first, so that is the
    // reason given; `hosting` itself is checked below.
    e.store.add(&n("other")).unwrap();
    let _s = Session::in_dir(&e.root.join("spare"));
    hosts(&e, &[("spare", "other")]);
    let before = ledger_bytes(&e);
    assert_refused(&e, "spare", "(session running)");
    assert_eq!(ledger_bytes(&e), before, "the hosted login is untouched");
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
    assert_refused(
        &e,
        "spare",
        "profile spare is not a rotate candidate (no account): \
         its .claude.json has no usable oauthAccount",
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
        candidates: Vec::new(),
        skips: vec![(n("work"), Skip::Live), (n("spare"), skip)],
        sees_sessions,
        pin: PinState::None,
    }
}

#[test]
fn every_skip_reason_but_live_refuses_with_the_rotator_reason() {
    let e = env();
    for skip in [
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
