//! Issue #1962, S4: how a seeding scan counts each other profile. One expired spare
//! must not block every seed. A record stands in for a read only on a cause that
//! says nothing against the folder, and a doubtful login always blocks.

#![cfg(unix)]

use crate::support::profile_1962::{Answer, World, assert_refused, printed};
use crate::support::profile_rotate::private_file;
use std::fs;

fn seed_work() -> World {
    let w = World::new();
    w.no_record("work");
    w
}

fn assert_seeds(w: &World) {
    let out = w.seed("work", true);
    assert!(out.status.success(), "{}", printed(&out));
    assert_eq!(w.record("work").unwrap()["account"], "uuid-work");
}

fn assert_blocked(w: &World, reason: &str) {
    assert_refused(w, "work", &w.seed("work", true), reason);
}

#[test]
fn record_stands_in_for_a_read_that_says_nothing_against_the_folder() {
    let w = seed_work();
    w.expire("spare");
    assert_seeds(&w);

    for answer in [Answer::Status(401), Answer::Status(429), Answer::Hang] {
        let w = seed_work();
        w.fake.answer("spare", answer);
        assert_seeds(&w);
    }
}

#[test]
fn record_does_not_stand_in_for_other_causes() {
    for (answer, cause) in [
        (Answer::Status(403), "http 403"),
        (Answer::Status(500), "http 5xx"),
        (
            Answer::Account(serde_json::json!({"account": {}})),
            "unparseable",
        ),
    ] {
        let w = seed_work();
        w.fake.answer("spare", answer);
        assert_blocked(&w, &format!("profile spare not read ({cause})"));
    }
}

#[test]
fn no_record_never_stands_in() {
    let w = seed_work();
    w.no_record("spare");
    w.expire("spare");
    assert_blocked(&w, "profile spare not read (token expired)");

    let w = seed_work();
    w.no_record("spare");
    w.fake.answer("spare", Answer::Status(429));
    assert_blocked(&w, "profile spare not read (http 429)");
}

#[test]
fn unusable_record_blocks_naming_the_profile() {
    let w = seed_work();
    private_file(&w.record_path("spare"), "{}");
    assert_blocked(&w, "profile spare has an unusable record; repair it first");
}

/// A doubtful login blocks even with a record, because the reading cannot be
/// trusted. These causes are never replaced by a record.
#[test]
fn doubtful_login_blocks_even_with_a_record() {
    let w = seed_work();
    w.chmod(&w.f.slot("spare"), 0o660);
    assert_blocked(&w, "profile spare not read (login file not trusted)");

    let w = seed_work();
    let dir = w.f.root.join("spare");
    w.chmod(&dir, 0o777);
    let out = w.seed("work", true);
    w.chmod(&dir, 0o700);
    assert_refused(
        &w,
        "work",
        &out,
        "profile spare not read (directory not private)",
    );
}

#[test]
fn missing_login_makes_no_claim_and_no_read() {
    let w = seed_work();
    w.no_record("spare");
    fs::remove_file(w.f.slot("spare")).unwrap();
    assert_seeds(&w);
    assert_eq!(w.fake.count("profile:spare"), 0);
}

#[test]
fn api_key_login_makes_no_claim_and_no_read() {
    let w = seed_work();
    w.no_record("spare");
    w.api_key_login("spare");
    assert_seeds(&w);
    assert_eq!(w.fake.count("profile:spare"), 0);
}

/// A login that differs from its record counts both accounts as claimed.
#[test]
fn login_differing_from_its_record_claims_both_accounts() {
    let w = seed_work();
    w.login("spare", "work");
    assert_blocked(&w, "another profile claims this account: spare");

    let w = seed_work();
    w.set_record("spare", "uuid-work", Some("org-work"));
    assert_blocked(&w, "another profile claims this account: spare");
}

/// The refusal is about the claim, with names sorted, and an expired login with a
/// record still claims by its record.
#[test]
fn claimers_are_named_sorted() {
    let w = seed_work();
    w.set_record("spare", "uuid-work", Some("org-work"));
    w.set_record("extra", "uuid-work", Some("org-work"));
    w.expire("extra");
    assert_blocked(&w, "another profile claims this account: extra, spare");
}

/// One account in two organizations is two accounts.
#[test]
fn same_uuid_in_another_organization_is_not_a_claim() {
    let w = seed_work();
    w.set_record("spare", "uuid-work", Some("org-other"));
    w.fake.answer(
        "spare",
        Answer::Account(crate::support::profile_1962::account_in(
            "uuid-work",
            "work@example.com",
            "org-other",
        )),
    );
    assert_seeds(&w);
}

/// Swap authorization keeps its strict scan: an expired spare with a record lets a
/// seed through, and a switch still refuses when that read fails.
#[test]
fn swap_authorization_stays_on_the_strict_scan() {
    let w = seed_work();
    w.fake.answer("spare", Answer::Status(429));
    let out = w.switch("extra");
    assert!(!out.status.success(), "a failed scan read refuses the swap");
    assert_eq!(w.record("work"), None, "a switch never seeds");
}
