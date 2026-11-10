//! Issue #1753, Codex review R1: an expired but refreshable live login is
//! refreshed once, identified again from the refreshed credential, and only
//! then measured. A refresh that fails, or a refreshed login whose identity
//! conflicts with its record, still alerts and moves nothing. Reuses the
//! #1753 fixture: fake credentials, a loopback refresh and identity
//! endpoint, and an isolated home under the task-owned TMPDIR.

#![cfg(unix)]

use crate::issue_1753_login_identity::{Endpoint, LIVE_REFRESH, direct_cycle, fixture};
#[cfg(target_os = "linux")]
use crate::support::fake_proc::{FAKE_PID_BASE, FakeProc};
use crate::support::profile_rotate::{Fixture, ino, private_file, text};
use std::sync::atomic::Ordering;

/// The fixture with the live login in `~/.claude` past its `expiresAt`,
/// holding a refresh token the fake endpoint renews to the same account.
fn expired_live() -> (Fixture, Endpoint) {
    let (f, ep) = fixture();
    let path = f.home_login();
    assert!(path.starts_with(f.tmp.path()));
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["claudeAiOauth"]["expiresAt"] = (chrono::Utc::now().timestamp_millis() - 60_000).into();
    value["claudeAiOauth"]["refreshToken"] = LIVE_REFRESH.into();
    private_file(&path, &value.to_string());
    (f, ep)
}

fn oauth(path: &std::path::Path) -> serde_json::Value {
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    v["claudeAiOauth"].clone()
}

#[test]
fn expired_refreshable_live_login_is_refreshed_identified_then_rotated() {
    let (f, ep) = expired_live();
    ep.refresh_allowed.store(true, Ordering::Relaxed);
    let candidates = [("spare", f.spare_ino), ("extra", f.extra_ino)];
    let out = direct_cycle(&f, &ep);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        ep.refreshes.load(Ordering::Relaxed) > 0,
        "live login refreshed"
    );
    let log = f.log();
    assert!(
        log.contains("expired login in ~/.claude refreshed"),
        "refresh logged: {log}"
    );
    assert!(
        !log.contains("trusted login identity unavailable"),
        "a refreshed live login is identified, not refused: {log}"
    );
    let ledger = f.ledger().unwrap();
    assert_ne!(ledger["live"], "work", "work at 90% rotates away: {log}");
    let to = ledger["live"].as_str().unwrap().to_string();
    assert_eq!(ledger["account"], format!("uuid-{to}"));
    // The refreshed work login moved, still fresh, into its own slot.
    let work = oauth(&f.slot("work"));
    assert_eq!(work["accessToken"], "fake-task1753-work");
    assert!(work["expiresAt"].as_i64().unwrap() > chrono::Utc::now().timestamp_millis());
    // The incoming login is the one that was in `to`'s slot; the other
    // candidate is untouched. The refresh wrote a new file in `~/.claude`,
    // so the outgoing login has a new inode, and there are still three.
    assert!(ino(&f.slot(&to)).is_none());
    for (name, inode) in candidates {
        let want = if name == to {
            &f.home_login()
        } else {
            &f.slot(name)
        };
        assert_eq!(ino(want), Some(inode), "{name}: {:?}", f.logins());
    }
    assert_eq!(f.logins().len(), 3, "{:?}", f.logins());
    assert!(!f.root.join(".rejected-logins").exists());
}

#[test]
fn failed_live_refresh_alerts_and_moves_no_login() {
    let (f, ep) = expired_live();
    let before = f.logins();
    for _ in 0..2 {
        let _ = direct_cycle(&f, &ep);
    }
    assert!(
        ep.refreshes.load(Ordering::Relaxed) > 0,
        "refresh attempted"
    );
    let log = f.log();
    assert!(
        log.contains("expired login in ~/.claude not refreshed"),
        "{log}"
    );
    assert!(
        log.contains("trusted login identity unavailable: token expired"),
        "{log}"
    );
    assert_eq!(f.logins(), before);
    assert_eq!(f.ledger().unwrap()["live"], "work");
    assert_eq!(f.ledger().unwrap()["account"], "uuid-work");
    assert!(!f.root.join(".rejected-logins").exists());
}

#[test]
fn refreshed_live_login_with_conflicting_identity_moves_no_login() {
    let (f, ep) = expired_live();
    ep.refresh_allowed.store(true, Ordering::Relaxed);
    ep.identity(
        "work",
        serde_json::json!({"account": {"uuid":"uuid-work"},
        "organization":{"uuid":"conflicting-organization"}}),
    );
    let spare = ino(&f.slot("spare"));
    let extra = ino(&f.slot("extra"));
    for _ in 0..2 {
        let _ = direct_cycle(&f, &ep);
    }
    assert!(ep.refreshes.load(Ordering::Relaxed) > 0);
    assert_eq!(oauth(&f.home_login())["accessToken"], "fake-task1753-work");
    assert_eq!(ino(&f.slot("spare")), spare);
    assert_eq!(ino(&f.slot("extra")), extra);
    assert_eq!(ino(&f.slot("work")), None);
    assert_eq!(f.ledger().unwrap()["live"], "work");
    assert_eq!(f.ledger().unwrap()["account"], "uuid-work");
    assert!(!f.root.join(".rejected-logins").exists());
}

/// Neither refresh log line: nothing was attempted for `~/.claude`.
fn assert_no_live_refresh(f: &Fixture, ep: &Endpoint) {
    assert_eq!(ep.refreshes.load(Ordering::Relaxed), 0, "no refresh sent");
    let log = f.log();
    assert!(
        !log.contains("expired login in ~/.claude"),
        "no live refresh logged: {log}"
    );
}

/// Issue #1962 (D6): a lease view that bars the live profile no longer stops the
/// refresh of the login in `~/.claude`. Otherwise no identity can be read and
/// rotation stalls. The refresh is not identity evidence, and the lease itself is
/// kept. A barred profile folder is still never refreshed; see
/// `issue_1962_refresh_ledger`.
///
/// The move that follows is pinned exactly. The identity was read from the
/// refreshed login, so #1753 allows a move. `work` at 90% swaps to `extra`, the
/// only candidate the lease leaves: `spare`, hosted by auto-switch, is skipped as
/// leased and its login stays where it was. No other login moves.
#[cfg(target_os = "linux")]
#[test]
fn lease_barred_live_login_is_still_refreshed_and_the_cycle_goes_on() {
    let (f, ep) = expired_live();
    ep.refresh_allowed.store(true, Ordering::Relaxed);
    let state = f.root.join(".auto-switch.d");
    let now = chrono::Utc::now().timestamp();
    private_file(
        &state.join("ledger.json"),
        &serde_json::json!({"version": 1, "hosts": {"work": {"live": "spare",
            "swapped_at": now - 60, "pre_swap_fingerprint": null}}})
        .to_string(),
    );
    // A running session in `work` keeps auto-switch recovery from handing
    // the hosted login back, so the lease is still in force at the refresh.
    // The session is a static entry in a fake process table (#2042).
    let fake = FakeProc::new();
    fake.process(FAKE_PID_BASE)
        .cmdline(&["sleep", "600"])
        .status('S')
        .environ(&[("CLAUDE_CONFIG_DIR", f.root.join("work").to_str().unwrap())]);
    f.with_proc(&fake);
    let out = direct_cycle(&f, &ep);
    assert!(ep.refreshes.load(Ordering::Relaxed) > 0, "refresh sent");
    let log = f.log();
    assert!(
        log.contains("expired login in ~/.claude refreshed"),
        "{log}"
    );
    assert!(!log.contains("trusted login identity unavailable"), "{log}");
    let hosts: serde_json::Value =
        serde_json::from_slice(&std::fs::read(state.join("ledger.json")).unwrap()).unwrap();
    assert_eq!(
        hosts["hosts"]["work"]["live"], "spare",
        "lease kept: {out:?}"
    );
    assert!(log.contains("skip: spare (leased)"), "{log}");
    assert!(log.contains("swap work->extra"), "{log}");
    let ledger = f.ledger().unwrap();
    assert_eq!(ledger["live"], "extra", "{log}");
    assert_eq!(ledger["account"], "uuid-extra");
    // The refreshed work login moved, still fresh, into its own slot; extra's
    // login is the one now in `~/.claude`; spare's is untouched.
    let work = oauth(&f.slot("work"));
    assert_eq!(work["accessToken"], "fake-task1753-work");
    assert!(work["expiresAt"].as_i64().unwrap() > chrono::Utc::now().timestamp_millis());
    assert_eq!(ino(&f.home_login()), Some(f.extra_ino));
    assert_eq!(ino(&f.slot("extra")), None);
    assert_eq!(ino(&f.slot("spare")), Some(f.spare_ino));
    assert_eq!(f.logins().len(), 3, "{:?}", f.logins());
}

/// A live token that has not expired is never sent to the token endpoint,
/// and no refresh line is logged, whether or not the endpoint is usable.
#[test]
fn unexpired_live_login_sends_no_refresh() {
    let (f, ep) = fixture();
    ep.refresh_allowed.store(true, Ordering::Relaxed);
    let out = direct_cycle(&f, &ep);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_no_live_refresh(&f, &ep);
    // With no usable endpoints the cycle must not claim a failed refresh of
    // a token that has not expired.
    let (g, _unused) = fixture();
    let _ = g
        .cmd(&["profile", "rotate"])
        .env_remove("AMPLIHACK_PROFILE_USAGE_API_BASE")
        .env_remove("AMPLIHACK_PROFILE_USAGE_TOKEN_URL")
        .output()
        .unwrap();
    let log = g.log();
    assert!(!log.contains("expired login in ~/.claude"), "{log}");
}

/// Issue #1962 (D6): a damaged rotate ledger no longer stops the refresh. The cycle
/// refreshes, reads the identity, then stops with a named cause and moves nothing.
#[test]
fn unreadable_rotate_ledger_still_refreshes_then_stops() {
    let (f, ep) = expired_live();
    ep.refresh_allowed.store(true, Ordering::Relaxed);
    private_file(&f.rotate_dir().join("ledger.json"), "{not json");
    let _ = direct_cycle(&f, &ep);
    assert!(ep.refreshes.load(Ordering::Relaxed) > 0, "refresh sent");
    let log = f.log();
    assert!(
        log.contains("ALERT: rotate ledger unreadable; live identity: readable"),
        "{log}"
    );
    assert_eq!(
        std::fs::read_to_string(f.rotate_dir().join("ledger.json")).unwrap(),
        "{not json",
        "the damaged ledger stays as it is"
    );
    assert_eq!(ino(&f.slot("spare")), Some(f.spare_ino));
    assert_eq!(ino(&f.slot("extra")), Some(f.extra_ino));
    assert_eq!(ino(&f.slot("work")), None);
}
