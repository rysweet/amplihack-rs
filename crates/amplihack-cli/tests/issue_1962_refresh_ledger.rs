//! Issue #1962, test group A: an expired live login is refreshed before its
//! identity is read, even when the ledger is unreadable or the lease view bars the
//! live profile. The fake endpoint never accepts the old token, so a pass proves
//! the refresh came first. The refusals that stay are pinned here too.

#![cfg(unix)]

#[cfg(target_os = "linux")]
use crate::support::fake_proc::{FAKE_PID_BASE, FakeProc};
use crate::support::profile_1962::{Answer, World, account, printed};
use crate::support::profile_rotate::{ino, private_file, text};
use std::fs;

const REFRESHED: &str = "refresh: expired login in ~/.claude refreshed; identity read again";

fn stop_line(cause: &str) -> String {
    format!("rotate ledger unreadable; live identity: {cause}; nothing moved, no record written")
}

fn expired_with_refresh() -> World {
    let w = World::new();
    w.expire("work");
    w.fake.renews("work", "work");
    w
}

fn assert_spares_in_place(w: &World) {
    assert_eq!(ino(&w.f.slot("spare")), Some(w.f.spare_ino));
    assert_eq!(ino(&w.f.slot("extra")), Some(w.f.extra_ino));
    assert_eq!(ino(&w.f.slot("work")), None, "no login moved to the slot");
}

#[test]
fn unreadable_ledger_refreshes_then_reads_identity_then_stops() {
    let w = expired_with_refresh();
    w.damage_ledger();
    let before = fs::read(w.ledger_path()).unwrap();
    let _ = w.cycle();

    let events = w.fake.events();
    let refresh = events.iter().position(|e| e == "refresh:work");
    let read = events.iter().position(|e| e == "profile:work");
    assert!(refresh.is_some(), "the live token is refreshed: {events:?}");
    assert!(read.is_some(), "the identity is read: {events:?}");
    assert!(refresh < read, "refresh comes first: {events:?}");

    let log = w.f.log();
    assert!(log.contains(REFRESHED), "{log}");
    assert!(
        log.contains(&format!("ALERT: {}", stop_line("readable"))),
        "{log}"
    );
    assert_eq!(w.token("work"), "fake-task1962-work", "refreshed on disk");
    assert_eq!(
        fs::read(w.ledger_path()).unwrap(),
        before,
        "ledger byte-identical"
    );
    assert_spares_in_place(&w);
}

#[test]
fn unreadable_ledger_seeds_no_record() {
    let w = expired_with_refresh();
    w.no_record("work");
    w.damage_ledger();
    let _ = w.cycle();
    assert_eq!(w.record("work"), None);
    assert!(!w.f.log().contains("seeded:"), "{}", w.f.log());
}

#[test]
fn unreadable_ledger_with_a_failed_refresh_names_the_cause() {
    let w = World::new();
    w.expire("work"); // No renewal: the fake refuses the refresh.
    w.damage_ledger();
    let _ = w.cycle();
    assert_eq!(w.fake.count("refresh:work"), 1);
    let log = w.f.log();
    assert!(
        log.contains("refresh: expired login in ~/.claude not refreshed ("),
        "{log}"
    );
    assert!(
        log.contains(&format!("ALERT: {}", stop_line("token expired"))),
        "{log}"
    );
    assert!(!log.contains("could not be read"), "{log}");
    assert_spares_in_place(&w);
}

#[test]
fn unreadable_ledger_alert_is_written_on_every_cycle() {
    let w = expired_with_refresh();
    w.damage_ledger();
    let line = stop_line("readable");
    for cycle in 1..=2 {
        let _ = w.cycle();
        assert_eq!(w.f.log().matches(&line).count(), cycle, "{}", w.f.log());
    }
}

#[test]
fn unreadable_ledger_dry_run_never_refreshes_and_names_the_stop() {
    let w = expired_with_refresh();
    w.damage_ledger();
    let stdout = text(&w.dry().stdout);
    assert_eq!(w.fake.refreshes(), 0, "{:?}", w.fake.events());
    assert!(
        stdout.contains("would refresh: expired login in ~/.claude"),
        "{stdout}"
    );
    let want = format!("would alert: {}", stop_line("token expired"));
    assert!(stdout.contains(&want), "{stdout}");
    assert_eq!(w.token("work"), "fake-task1962-work-old");
}

#[test]
fn unreadable_ledger_switch_refuses_with_the_same_text() {
    let w = expired_with_refresh();
    w.damage_ledger();
    let out = w.switch("extra");
    assert!(!out.status.success());
    assert!(
        printed(&out).contains(&stop_line("readable")),
        "{}",
        printed(&out)
    );
    assert_spares_in_place(&w);
}

/// A refresh is not identity evidence: the refreshed login holds another account
/// from the record, and nothing moves.
#[test]
fn refresh_to_a_conflicting_account_stays_fail_closed() {
    let w = expired_with_refresh();
    w.fake.answer("work", Answer::Account(account("stranger")));
    for _ in 0..2 {
        let _ = w.cycle();
    }
    assert_eq!(
        w.fake.count("refresh:work"),
        1,
        "refreshed once, then fresh"
    );
    assert_spares_in_place(&w);
    assert_eq!(w.f.ledger().unwrap()["live"], "work");
}

/// The refreshed login in `~/.claude` goes on even though `spare` is barred, but a
/// barred profile folder is never refreshed. `spare`'s renewal is on offer, so
/// only the bar keeps it from being sent.
#[cfg(target_os = "linux")]
#[test]
fn barred_profile_folder_is_never_refreshed() {
    let w = expired_with_refresh();
    w.expire("spare");
    w.fake.renews("spare", "spare");
    private_file(
        &w.state().join("ledger.json"),
        &serde_json::json!({"version": 1, "hosts": {"spare": {"live": "extra",
            "swapped_at": chrono::Utc::now().timestamp() - 60,
            "pre_swap_fingerprint": null}}})
        .to_string(),
    );
    // A session in `spare`: a static entry in a fake process table (#2042).
    let fake = FakeProc::new();
    fake.process(FAKE_PID_BASE)
        .cmdline(&["sleep", "600"])
        .status('S')
        .environ(&[(
            "CLAUDE_CONFIG_DIR",
            w.f.root.join("spare").to_str().unwrap(),
        )]);
    w.f.with_proc(&fake);
    let _ = w.cycle();
    assert_eq!(w.fake.count("refresh:work"), 1, "{:?}", w.fake.events());
    assert_eq!(w.fake.count("refresh:spare"), 0, "{:?}", w.fake.events());
    assert_eq!(w.token("spare"), "fake-task1962-spare-old");
}

// --- Refusals that stay ------------------------------------------------------

fn assert_no_refresh_sent(w: &World) {
    assert_eq!(w.fake.refreshes(), 0, "{:?}", w.fake.events());
    assert_eq!(w.token("work"), "fake-task1962-work-old");
    assert_spares_in_place(w);
}

/// A damaged journal stops the cycle in recovery, before any refresh.
#[test]
fn pending_journal_blocks_the_refresh() {
    for journal in [
        ".auto-switch.d/rotate/journal.json",
        ".auto-switch.d/journal.json",
    ] {
        let w = expired_with_refresh();
        private_file(&w.f.root.join(journal), "{not json");
        let _ = w.cycle();
        assert_no_refresh_sent(&w);
    }
}

#[test]
fn held_refresh_lock_blocks_the_refresh_and_is_named() {
    let w = expired_with_refresh();
    let lock = w.f.home.join(".claude/.oauth_refresh.lock");
    fs::create_dir(&lock).unwrap();
    let _ = w.cycle();
    assert_no_refresh_sent(&w);
    let log = w.f.log();
    assert!(log.contains("not refreshed (refresh busy)"), "{log}");
    assert!(lock.is_dir(), "another holder's lock stays");
}

#[test]
fn directory_others_can_write_blocks_the_refresh_and_is_named() {
    let w = expired_with_refresh();
    w.chmod(&w.f.home.join(".claude"), 0o777);
    let _ = w.cycle();
    w.chmod(&w.f.home.join(".claude"), 0o700);
    assert_no_refresh_sent(&w);
    let log = w.f.log();
    assert!(log.contains("not refreshed (refresh refused)"), "{log}");
    assert!(
        log.contains("trusted login identity unavailable: directory not private"),
        "{log}"
    );
}
