//! Issue #1962, test groups C and D: a rotate cycle seeds a live profile's record
//! from one fresh reading, only with full ledger ownership, and tells two profiles
//! with one e-mail apart by account uuid and organization.

#![cfg(unix)]

use crate::support::profile_1962::{Answer, World, account_in};
use crate::support::profile_rotate::{ino, text};
use std::fs;

fn note(reason: &str) -> String {
    format!(
        "no record for live profile work: {reason}; nothing moved; run: amplihack profile seed-record work"
    )
}

fn assert_nothing_moved(w: &World) {
    assert_eq!(ino(&w.f.slot("spare")), Some(w.f.spare_ino));
    assert_eq!(ino(&w.f.slot("extra")), Some(w.f.extra_ino));
    assert_eq!(ino(&w.f.slot("work")), None);
    assert_eq!(w.f.ledger().unwrap()["live"], "work");
}

#[test]
fn full_ownership_seeds_from_one_reading_and_rotates() {
    let w = World::new();
    w.no_record("work");
    let out = w.cycle();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let log = w.f.log();
    assert!(
        log.contains("seeded: recorded account for live profile work"),
        "{log}"
    );
    assert_eq!(
        w.record("work").unwrap(),
        serde_json::json!({"account": "uuid-work", "org": "org-work"})
    );
    let ledger = w.f.ledger().unwrap();
    assert_ne!(ledger["live"], "work", "work at 90% rotates away: {log}");
    assert_eq!(
        ino(&w.f.slot("work")),
        Some(w.f.live_ino),
        "login is in its own slot"
    );
    assert!(!log.contains("trusted login identity unavailable"), "{log}");
}

#[test]
fn seeding_is_previewed_by_the_dry_run_and_nothing_is_written() {
    let w = World::new();
    w.no_record("work");
    let stamp = w.tree_stamp();
    let stdout = text(&w.dry().stdout);
    assert!(stdout.contains("live=work"), "{stdout}");
    assert!(
        stdout.contains("would seed: record for live profile work from a fresh reading"),
        "{stdout}"
    );
    assert_eq!(w.record("work"), None);
    assert_eq!(w.tree_stamp(), stamp);
}

/// D2: a legacy key alone cannot prove ownership, even when it matches and no
/// other profile claims the account.
#[test]
fn older_ledger_does_not_seed_or_move_and_names_the_command() {
    let w = World::new();
    w.no_record("work");
    w.older_ledger("uuid-work");
    let logins = w.logins_stamp();
    let _ = w.cycle();
    let log = w.f.log();
    assert!(
        log.contains(&note("the ledger has no login identity")),
        "{log}"
    );
    assert!(
        log.contains("outgoing ownership is not established"),
        "{log}"
    );
    assert_eq!(w.record("work"), None);
    assert_nothing_moved(&w);
    assert_eq!(w.logins_stamp(), logins);
}

/// D3, the candide acceptance wording: the dry run names the live login and the
/// extra line, with neither of the old refusals.
#[test]
fn older_ledger_dry_run_names_the_login_and_the_extra_line() {
    let w = World::new();
    w.no_record("work");
    w.older_ledger("uuid-work");
    let stdout = text(&w.dry().stdout);
    assert!(stdout.contains("live=work"), "{stdout}");
    assert!(
        stdout.contains(&format!(
            "would note: {}",
            note("the ledger has no login identity")
        )),
        "{stdout}"
    );
    assert!(
        !stdout.contains("trusted login identity unavailable"),
        "{stdout}"
    );
    assert!(!stdout.contains("could not be read"), "{stdout}");
}

#[test]
fn ledger_identity_for_another_account_stops_the_seed() {
    let w = World::new();
    w.no_record("work");
    w.fake.answer(
        "work",
        Answer::Account(account_in("uuid-work", "work@example.com", "org-new")),
    );
    let _ = w.cycle();
    assert!(
        w.f.log()
            .contains(&note("the ledger does not match the reading")),
        "{}",
        w.f.log()
    );
    assert_eq!(w.record("work"), None);
    assert_nothing_moved(&w);
}

#[test]
fn reading_with_no_organization_never_seeds() {
    let w = World::new();
    w.no_record("work");
    w.fake.answer(
        "work",
        Answer::Account(serde_json::json!({"account":
            {"uuid": "uuid-work", "email": "work@example.com"}})),
    );
    let _ = w.cycle();
    let want = note("the reading has no account uuid or organization");
    assert!(w.f.log().contains(&want), "{}", w.f.log());
    assert_eq!(w.record("work"), None);
    assert_nothing_moved(&w);
}

#[test]
fn failed_read_names_the_cause_in_the_seed_note() {
    let w = World::new();
    w.no_record("work");
    w.fake.answer("work", Answer::Status(429));
    let _ = w.cycle();
    assert!(
        w.f.log().contains(&note("identity read failed: http 429")),
        "{}",
        w.f.log()
    );
    assert_eq!(w.record("work"), None);
    assert_nothing_moved(&w);
}

#[test]
fn blocked_scan_names_the_profile_and_cause() {
    let w = World::new();
    w.no_record("work");
    w.fake.answer("spare", Answer::Status(403));
    let _ = w.cycle();
    assert!(
        w.f.log()
            .contains(&note("profile spare not read (http 403)")),
        "{}",
        w.f.log()
    );
    assert_eq!(w.record("work"), None);
}

#[test]
fn existing_usable_record_is_never_rewritten_by_a_cycle() {
    let w = World::new();
    let before = fs::read(w.record_path("work")).unwrap();
    let inode = ino(&w.record_path("work"));
    let _ = w.cycle();
    assert_eq!(fs::read(w.record_path("work")).unwrap(), before);
    assert_eq!(ino(&w.record_path("work")), inode);
    assert!(!w.f.log().contains("seeded:"), "{}", w.f.log());
}

// --- Two profiles with one e-mail ---------------------------------------------

/// `work` and `spare` share an e-mail and an account uuid, in two organizations.
fn shared_email(spare_org: &str) -> World {
    let w = World::new();
    w.no_record("work");
    w.no_record("spare");
    let email = "shared@example.com";
    w.fake.answer(
        "work",
        Answer::Account(account_in("uuid-shared", email, "org-a")),
    );
    w.fake.answer(
        "spare",
        Answer::Account(account_in("uuid-shared", email, spare_org)),
    );
    let mut ledger = World::ledger_for(
        "work",
        "uuid-shared",
        serde_json::json!({"accountUuid": "uuid-shared", "emailAddress": email,
            "organizationUuid": "org-a"}),
    );
    // Inside the swap grace, so hand-change recovery reads nothing here.
    let recent = chrono::Utc::now().timestamp() - 60;
    ledger["swapped_at"] = recent.into();
    ledger["since"] = recent.into();
    w.set_ledger(&ledger);
    w
}

#[test]
fn same_email_in_two_organizations_is_told_apart() {
    let w = shared_email("org-b");
    let _ = w.cycle();
    let log = w.f.log();
    assert!(
        log.contains("seeded: recorded account for live profile work"),
        "{log}"
    );
    assert_eq!(
        w.record("work").unwrap(),
        serde_json::json!({"account": "uuid-shared", "org": "org-a"})
    );
    assert!(!log.contains("cannot tell profiles"), "{log}");
}

#[test]
fn same_account_alerts_naming_both_on_every_cycle_and_moves_nothing() {
    let w = shared_email("org-a");
    let want = "ALERT: cannot tell profiles spare, work apart: both claim the account in ~/.claude; no record written, nothing moved";
    for _ in 1..=2 {
        let _ = w.cycle();
        // Issue #2013 (D11): written once an hour; the stop holds every cycle.
        assert_eq!(w.f.log().matches(want).count(), 1, "{}", w.f.log());
        assert_eq!(w.record("work"), None);
        assert_eq!(w.record("spare"), None);
        assert_nothing_moved(&w);
    }
}

/// A record claims its account too, even with no login to read.
#[test]
fn another_profiles_record_of_the_same_account_alerts() {
    let w = shared_email("org-b");
    w.set_record("extra", "uuid-shared", Some("org-a"));
    let _ = w.cycle();
    let want = "cannot tell profiles extra, work apart";
    assert!(w.f.log().contains(want), "{}", w.f.log());
    assert_eq!(w.record("work"), None);
    assert_nothing_moved(&w);
}
