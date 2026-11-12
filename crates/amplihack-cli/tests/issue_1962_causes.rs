//! Issue #1962, test group B: a refused identity read is reported as itself in the
//! rotate log, the dry run and `profile switch`. It is never "could not be read",
//! and it never blames `~/.claude.json`. Nothing moves and no login file changes.

#![cfg(unix)]

use crate::support::profile_1962::{Answer, World, printed};
use crate::support::profile_rotate::{private_file, text};

const VAGUE: &[&str] = &[
    "could not be read",
    ".claude.json",
    "unavailable or changed",
    "verify it before retrying",
];

fn assert_not_vague(what: &str, text: &str) {
    // Issue #2013 (D10): a refusal's evidence names each source, the
    // `.claude.json=` field included; that field never blames the file.
    let text = &text.replace(".claude.json=", "home-block=");
    for vague in VAGUE {
        assert!(!text.contains(vague), "{what} says {vague:?}: {text}");
    }
}

fn live_read_fails_with(answer: Answer, cause: &str) {
    let w = World::new();
    w.fake.answer("work", answer);
    let logins = w.logins_stamp();
    let held = |w: &World| {
        let l = w.f.ledger().unwrap();
        (
            l["live"].clone(),
            l["account"].clone(),
            l["login_identity"].clone(),
        )
    };
    let ledger = held(&w);

    let _ = w.cycle();
    let log = w.f.log();
    // Issue #2013 (D10): the refusal line, with its kind and evidence.
    let alert = format!(
        "ALERT: swap refused (identity-unavailable, profile work): trusted login identity unavailable: {cause}. Evidence:"
    );
    assert!(log.contains(&alert), "{log}");
    let note = format!("~/.claude identity read failed: {cause}");
    assert!(log.contains(&note), "{log}");
    assert_not_vague("log", &log);

    let stdout = text(&w.dry().stdout);
    let would = format!(
        "would alert: swap refused (identity-unavailable, profile work): trusted login identity unavailable: {cause}. Evidence:"
    );
    assert!(stdout.contains(&would), "{stdout}");
    assert!(stdout.contains(&note), "{stdout}");
    assert_not_vague("dry run", &stdout);

    assert_eq!(w.logins_stamp(), logins, "no login file changed");
    // The cycle records its ALERT and note; whose login it names must not change.
    assert_eq!(held(&w), ledger, "the ledger is not relabelled");
    assert_eq!(w.fake.refreshes(), 0);
}

#[test]
fn live_401_is_named() {
    live_read_fails_with(Answer::Status(401), "http 401");
}

#[test]
fn live_429_is_named() {
    live_read_fails_with(Answer::Status(429), "http 429");
}

#[test]
fn live_timeout_is_named() {
    live_read_fails_with(Answer::Hang, "timeout");
}

#[test]
fn switch_refusal_names_the_cause() {
    let w = World::new();
    w.fake.answer("work", Answer::Status(429));
    let logins = w.logins_stamp();
    let out = w.switch("extra");
    assert!(!out.status.success());
    let all = printed(&out);
    assert!(
        all.contains("trusted login identity unavailable: http 429"),
        "{all}"
    );
    assert_eq!(w.logins_stamp(), logins);
}

#[test]
fn alert_tracks_the_cause_on_every_cycle() {
    let w = World::new();
    w.fake.answer("work", Answer::Status(429));
    let _ = w.cycle();
    let _ = w.cycle();
    w.fake.answer("work", Answer::Status(401));
    let _ = w.cycle();
    let log = w.f.log();
    // Issue #2013 (D11): the same cause within the hour is counted, not
    // written; a new cause is written at once.
    assert_eq!(log.matches("unavailable: http 429").count(), 1, "{log}");
    assert_eq!(log.matches("unavailable: http 401").count(), 1, "{log}");
}

#[test]
fn candidate_refusal_names_the_profile_and_cause_and_leaves_it_in_place() {
    let w = World::new();
    w.fake.answer("spare", Answer::Status(429));
    let _ = w.cycle();
    let log = w.f.log();
    let want = "ALERT: trusted login identity unavailable for profile spare: http 429";
    assert!(log.contains(want), "{log}");
    assert_not_vague("log", &log);
    assert_eq!(
        crate::support::profile_rotate::ino(&w.f.slot("spare")),
        Some(w.f.spare_ino),
        "a candidate with no identity is never swapped in"
    );
}

/// A world-writable directory is refused on every host, whatever its group.
#[test]
fn directory_cause_is_named() {
    let w = World::new();
    w.chmod(&w.f.home.join(".claude"), 0o777);
    let _ = w.cycle();
    w.chmod(&w.f.home.join(".claude"), 0o700);
    let log = w.f.log();
    assert!(
        log.contains("trusted login identity unavailable: directory not private"),
        "{log}"
    );
    assert_eq!(
        w.fake.count("profile:work"),
        0,
        "no token sent: {:?}",
        w.fake.events()
    );
}

/// File checks stay strict, whatever the file's group: no token is sent and
/// nothing moves. The live login's refusal may stay the file-safety ALERT.
#[test]
fn group_writable_live_login_sends_no_token() {
    let w = World::new();
    w.chmod(&w.f.home_login(), 0o660);
    let _ = w.cycle();
    w.chmod(&w.f.home_login(), 0o600);
    let log = w.f.log();
    assert!(log.contains("ALERT: "), "{log}");
    assert_not_vague("log", &log);
    assert_eq!(w.fake.count("profile:work"), 0, "no token sent");
    assert_eq!(w.f.ledger().unwrap()["live"], "work");
}

/// The note names each failed profile with its own cause, sorted.
#[test]
fn unread_profiles_are_each_named_with_a_cause() {
    let w = World::new();
    w.set_record("work", "uuid-other", Some("org-other"));
    w.fake.answer("extra", Answer::Status(503));
    w.fake.answer("spare", Answer::Status(429));
    let _ = w.cycle();
    let log = w.f.log();
    let want = "profile extra not read (http 5xx), spare not read (http 429)";
    assert!(log.contains(want), "{log}");
    assert_not_vague("log", &log);
}

/// S7: a note over the ledger's 1024-byte `guidance` limit must not make the ledger
/// unreadable. If it did, the failed read would stop every later cycle.
#[test]
fn long_note_leaves_the_ledger_readable() {
    let w = World::new();
    w.set_record("work", "uuid-other", Some("org-other"));
    for i in 0..20 {
        let name = format!("a-long-profile-name-for-the-guidance-limit-{i:02}");
        let dir = w.f.root.join(&name);
        private_file(
            &dir.join(".credentials.json"),
            &serde_json::json!({"claudeAiOauth": {
                "accessToken": format!("fake-task1962-long{i}"),
                "expiresAt": chrono::Utc::now().timestamp_millis() + 3_600_000
            }})
            .to_string(),
        );
        w.chmod(&dir, 0o700);
    }
    let _ = w.cycle();
    assert!(w.f.log().contains("not read (http 401)"), "{}", w.f.log());
    let _ = w.cycle();
    let log = w.f.log();
    assert!(!log.contains("rotate ledger unreadable"), "{log}");
    let ledger = w.f.ledger().expect("the ledger still parses");
    assert_eq!(ledger["live"], "work");
    for line in ledger["guidance"].as_array().into_iter().flatten() {
        assert!(line.as_str().unwrap().len() <= 1024, "{line}");
    }
}
