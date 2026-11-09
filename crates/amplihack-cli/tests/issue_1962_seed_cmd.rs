//! Issue #1962: `amplihack profile seed-record <name> [--write]`, the explicit
//! seeding path. One fresh reading per profile, no moves, no credential writes and
//! no `profile verify --fix`.

#![cfg(unix)]

use crate::support::profile_1962::{Answer, World, assert_refused, printed};
use crate::support::profile_rotate::{private_file, text};
use std::fs;
use std::os::fd::AsRawFd;

fn no_record_for_work() -> World {
    let w = World::new();
    w.no_record("work");
    w
}

#[test]
fn preview_shows_the_account_and_writes_nothing() {
    let w = no_record_for_work();
    let stamp = w.tree_stamp();
    let out = w.seed("work", false);
    assert!(out.status.success(), "{}", printed(&out));
    let stdout = text(&out.stdout);
    let want = "work -> work@example.com, organization org-work: no record; nothing written; run with --write to record it";
    assert!(stdout.contains(want), "{stdout}");
    assert_eq!(w.tree_stamp(), stamp, "the preview writes nothing");
    assert_eq!(w.fake.count("profile:work"), 1, "one fresh reading");
}

#[test]
fn write_records_only_the_account_and_moves_nothing() {
    let w = no_record_for_work();
    let (logins, ledger) = (w.logins_stamp(), fs::read(w.ledger_path()).unwrap());
    let out = w.seed("work", true);
    assert!(out.status.success(), "{}", printed(&out));
    assert!(
        text(&out.stdout).contains("work -> work@example.com, organization org-work: recorded")
    );
    assert_eq!(
        w.record("work").unwrap(),
        serde_json::json!({"account": "uuid-work", "org": "org-work"})
    );
    assert_eq!(w.logins_stamp(), logins, "no login moved or written");
    assert_eq!(fs::read(w.ledger_path()).unwrap(), ledger);
    assert_eq!(w.fake.refreshes(), 0, "seeding never refreshes");
    assert!(
        w.f.log()
            .contains("seed-record: recorded account for profile work")
    );
    assert!(w.f.probe_envs().is_empty(), "seeding never starts claude");
}

#[test]
fn usable_record_is_kept_and_exits_zero() {
    let w = World::new();
    let before = fs::read(w.record_path("work")).unwrap();
    let out = w.seed("work", true);
    assert_eq!(out.status.code(), Some(0), "{}", printed(&out));
    assert!(printed(&out).contains("work: a usable record exists; nothing written"));
    assert_eq!(fs::read(w.record_path("work")).unwrap(), before);
}

#[test]
fn unusable_record_is_refused_and_kept() {
    let w = World::new();
    private_file(&w.record_path("work"), "{}");
    let out = w.seed("work", true);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        printed(&out).contains("refused: profile work has an unusable record; repair it first"),
        "{}",
        printed(&out)
    );
    assert_eq!(fs::read_to_string(w.record_path("work")).unwrap(), "{}");
}

#[test]
fn invalid_name_builds_no_path() {
    let w = World::new();
    for name in ["../evil", "a/b", ".auto-switch.d"] {
        let out = w.seed(name, true);
        assert!(!out.status.success(), "{name}");
        assert_ne!(
            out.status.code(),
            Some(2),
            "{name}: the name is refused, not the command"
        );
    }
    assert!(!w.f.root.join("../evil.json").exists());
    assert_eq!(w.fake.events(), Vec::<String>::new(), "nothing read");
}

#[test]
fn unreadable_ledger_is_refused_with_and_without_write() {
    for write in [false, true] {
        let w = no_record_for_work();
        w.damage_ledger();
        assert_refused(
            &w,
            "work",
            &w.seed("work", write),
            "the rotate ledger is unreadable",
        );
    }
}

#[test]
fn live_ledger_identity_for_another_account_is_refused() {
    let w = no_record_for_work();
    w.fake.answer(
        "work",
        Answer::Account(crate::support::profile_1962::account("other")),
    );
    assert_refused(
        &w,
        "work",
        &w.seed("work", true),
        "the ledger does not match the reading",
    );
}

#[test]
fn older_key_rules_for_the_live_profile() {
    let w = no_record_for_work();
    w.older_ledger("uuid-other");
    assert_refused(
        &w,
        "work",
        &w.seed("work", true),
        "the ledger's older key does not match the reading",
    );

    let w = no_record_for_work();
    w.older_ledger("work@example.com");
    assert_refused(
        &w,
        "work",
        &w.seed("work", true),
        "the ledger's older key is an e-mail, which cannot prove ownership",
    );

    let w = no_record_for_work();
    w.older_ledger("uuid-work");
    assert!(
        w.seed("work", true).status.success(),
        "the operator named it and the uuid matches"
    );
    assert!(w.record("work").is_some());
}

/// S3: the ledger's live profile is read from `~/.claude`, not from a stale login
/// left in its folder.
#[test]
fn live_profile_reads_home_never_its_folder() {
    let w = no_record_for_work();
    private_file(
        &w.f.slot("work"),
        &serde_json::json!({"claudeAiOauth": {"accessToken": "fake-task1962-decoy",
            "expiresAt": chrono::Utc::now().timestamp_millis() + 3_600_000}})
        .to_string(),
    );
    w.fake.answer(
        "decoy",
        Answer::Account(crate::support::profile_1962::account("decoy")),
    );
    assert!(w.seed("work", true).status.success());
    assert_eq!(w.record("work").unwrap()["account"], "uuid-work");
}

#[test]
fn non_live_profile_reads_its_own_folder_once_and_compares_home() {
    let w = World::new();
    w.no_record("spare");
    let out = w.seed("spare", true);
    assert!(out.status.success(), "{}", printed(&out));
    assert_eq!(w.record("spare").unwrap()["account"], "uuid-spare");
    assert_eq!(w.fake.count("profile:spare"), 1, "{:?}", w.fake.events());
    assert_eq!(w.fake.count("profile:work"), 1, "~/.claude is compared");
}

#[test]
fn non_live_profile_holding_the_home_account_is_refused() {
    let w = World::new();
    w.no_record("spare");
    w.login("spare", "work");
    let reason = "~/.claude holds this account, so the login may be live under another name";
    assert_refused(&w, "spare", &w.seed("spare", true), reason);
}

#[test]
fn non_live_profile_with_a_failed_home_read_is_refused() {
    let w = World::new();
    w.no_record("spare");
    w.fake.answer("work", Answer::Status(429));
    let reason = "~/.claude identity read failed: http 429";
    assert_refused(&w, "spare", &w.seed("spare", true), reason);
}

#[test]
fn no_ledger_treats_every_name_as_non_live() {
    let w = World::new();
    fs::remove_file(w.ledger_path()).unwrap();
    let reason = "~/.claude holds this account, so the login may be live under another name";
    w.login("spare", "work");
    w.no_record("spare");
    assert_refused(&w, "spare", &w.seed("spare", true), reason);
}

#[test]
fn expired_token_is_refused_and_not_refreshed() {
    let w = no_record_for_work();
    w.expire("work");
    w.fake.renews("work", "work");
    assert_refused(
        &w,
        "work",
        &w.seed("work", true),
        "identity read failed: token expired",
    );
    assert_eq!(w.fake.refreshes(), 0);
    assert_eq!(w.token("work"), "fake-task1962-work-old");
}

/// S8: either journal counts, and one that cannot be read counts as present.
#[test]
fn any_journal_stops_the_seed() {
    let reason = "a swap journal is pending or unreadable";
    for journal in [
        ".auto-switch.d/rotate/journal.json",
        ".auto-switch.d/journal.json",
    ] {
        for body in ["{not json", "{}"] {
            let w = no_record_for_work();
            private_file(&w.f.root.join(journal), body);
            assert_refused(&w, "work", &w.seed("work", true), reason);
        }
    }
}

#[test]
fn open_hand_suspicion_stops_the_seed() {
    let w = no_record_for_work();
    let mut ledger = w.f.ledger().unwrap();
    ledger["hand_suspect"] = serde_json::json!({"profile": "spare", "first_seen": 1_000_000});
    w.set_ledger(&ledger);
    assert_refused(
        &w,
        "work",
        &w.seed("work", true),
        "a hand-change suspicion is open",
    );
}

#[test]
fn busy_rotate_lock_is_a_named_error_and_writes_nothing() {
    let w = no_record_for_work();
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(w.state().join("lock"))
        .unwrap();
    // SAFETY: flock on a file this test owns; released when `lock` drops.
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
    let out = w.seed("work", true);
    assert!(!out.status.success());
    assert!(
        printed(&out).contains("rotate: lock busy"),
        "{}",
        printed(&out)
    );
    assert_eq!(w.record("work"), None);
}
