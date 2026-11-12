//! Issue #2006: the picker never touches credential files, and what it
//! prints about a candidate is derived and clean.
//!
//! Wired from `account/mod.rs` as `#[cfg(all(test, unix))] mod tests_credentials;`.
//!
//! Contract:
//!
//! - No source file of `commands/account/` names the Codex login file
//!   (`auth` + `.json`) or the Claude login file (`.credentials` + `.json`).
//!   This file builds the names from parts so it does not trip its own
//!   check.
//! - `reading::codex_candidate(&AccountResult, floor) -> Candidate`: live
//!   headroom is `codex_capacity::headroom`; a reading with no room and no
//!   credits above the floor is unusable; `NotSignedIn`, `Timeout` and
//!   `Error` give no reading (`live_headroom: None`) and the notes
//!   `not signed in`, `timed out` and `error: <kind>`. Raw server text
//!   never reaches a note.
//! - `reading::checked(Candidate) -> Candidate`: a home with a byte below
//!   0x20 is unusable with the note `unsafe path`; every note has control
//!   characters removed and is at most 200 characters.

use super::reading::{self, Candidate};
use super::test_fixtures::*;
use amplihack_utils::codex_capacity::{
    AccountResult, Credits, Outcome, Reading, Window, WindowName,
};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

const SOURCES: &[(&str, &str)] = &[
    ("mod.rs", include_str!("mod.rs")),
    ("config.rs", include_str!("config.rs")),
    ("rank.rs", include_str!("rank.rs")),
    ("procscan.rs", include_str!("procscan.rs")),
    ("lease.rs", include_str!("lease.rs")),
    ("reading.rs", include_str!("reading.rs")),
    ("pick.rs", include_str!("pick.rs")),
    ("launch.rs", include_str!("launch.rs")),
    ("test_fixtures.rs", include_str!("test_fixtures.rs")),
    ("tests_rank.rs", include_str!("tests_rank.rs")),
    ("tests_config.rs", include_str!("tests_config.rs")),
    ("tests_procscan.rs", include_str!("tests_procscan.rs")),
    ("tests_lease.rs", include_str!("tests_lease.rs")),
    (
        "tests_pick_concurrency.rs",
        include_str!("tests_pick_concurrency.rs"),
    ),
    ("tests_credentials.rs", include_str!("tests_credentials.rs")),
    ("tests_launch.rs", include_str!("tests_launch.rs")),
];

#[test]
fn no_account_source_names_a_credential_file() {
    let names = [
        ["auth", ".json"].concat(),
        [".credentials", ".json"].concat(),
    ];
    for (file, text) in SOURCES {
        for n in &names {
            assert!(
                !text.contains(n.as_str()),
                "commands/account/{file} names {n}; the picker must never open credentials"
            );
        }
    }
}

#[test]
fn every_account_source_file_is_checked() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands/account");
    let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".rs"))
        .collect();
    on_disk.sort();
    let mut listed: Vec<String> = SOURCES.iter().map(|(n, _)| (*n).to_string()).collect();
    listed.sort();
    assert_eq!(on_disk, listed, "add new account/*.rs files to SOURCES");
}

// ---------------------------------------------------------------------------
// Codex candidates from live readings
// ---------------------------------------------------------------------------

fn used(pcts: &[f64]) -> Reading {
    let mut r = Reading::default();
    r.windows = pcts
        .iter()
        .enumerate()
        .map(|(i, p)| Window {
            name: if i == 0 {
                WindowName::Primary
            } else {
                WindowName::Secondary
            },
            used_percent: Some(*p),
            window_minutes: None,
            resets_at: None,
        })
        .collect();
    r
}

fn row(outcome: Outcome) -> AccountResult {
    AccountResult::new("b", PathBuf::from("/p/b"), outcome, NOW)
}

#[test]
fn a_reading_gives_100_minus_the_fullest_window() {
    let c = reading::codex_candidate(&row(Outcome::Ok(used(&[12.0, 41.0]))), 3000.0);
    assert_eq!(c.account, "b");
    assert_eq!(c.home, PathBuf::from("/p/b"));
    assert_eq!(c.live_headroom, Some(59.0));
    assert!(c.usable);
    assert!(c.note.contains("41%"), "{}", c.note);
}

#[test]
fn credits_above_the_floor_give_half_a_point() {
    let mut r = used(&[100.0]);
    r.credits = Some(Credits {
        has_credits: true,
        balance: Some(4200.0),
    });
    let c = reading::codex_candidate(&row(Outcome::Ok(r.clone())), 3000.0);
    assert_eq!(c.live_headroom, Some(0.5));
    assert!(c.usable);
    let c = reading::codex_candidate(&row(Outcome::Ok(r)), 5000.0);
    assert!(!c.usable, "credits below --floor are not usable");
}

#[test]
fn a_full_account_without_credits_is_unusable() {
    let c = reading::codex_candidate(&row(Outcome::Ok(used(&[100.0]))), 3000.0);
    assert!(!c.usable);
}

#[test]
fn reasons_are_fixed_words_and_no_raw_text() {
    let c = reading::codex_candidate(&row(Outcome::NotSignedIn), 3000.0);
    assert_eq!(
        (c.live_headroom, c.usable, c.note.as_str()),
        (None, false, "not signed in")
    );
    let c = reading::codex_candidate(&row(Outcome::Timeout), 3000.0);
    assert_eq!(
        (c.live_headroom, c.usable, c.note.as_str()),
        (None, false, "timed out")
    );
    let raw = format!("server said {SECRET} user@example.com org-123");
    let c = reading::codex_candidate(&row(Outcome::Error(raw)), 3000.0);
    assert_eq!(c.live_headroom, None);
    assert!(!c.usable);
    assert!(c.note.starts_with("error: "), "{}", c.note);
    assert!(
        !c.note.contains(SECRET),
        "raw server text in note: {}",
        c.note
    );
    assert!(!c.note.contains("example.com"), "{}", c.note);
}

// ---------------------------------------------------------------------------
// Unsafe homes and clean notes
// ---------------------------------------------------------------------------

fn cand(home: PathBuf, note: &str) -> Candidate {
    Candidate {
        account: "x".into(),
        home,
        live_headroom: Some(70.0),
        usable: true,
        note: note.into(),
    }
}

#[test]
fn a_home_with_a_control_byte_is_unusable_with_a_fixed_note() {
    for bad in [
        &b"/p/a\nb"[..],
        b"/p/\x1b[31m",
        b"/p/\x01x",
        b"/p/tab\there",
    ] {
        let home = PathBuf::from(std::ffi::OsStr::from_bytes(bad));
        let c = reading::checked(cand(home, "used 30%"));
        assert!(!c.usable, "{bad:?}");
        assert_eq!(c.note, "unsafe path", "{bad:?}");
    }
    let c = reading::checked(cand(PathBuf::from("/p/ok home"), "used 30%"));
    assert!(c.usable);
    assert_eq!(c.note, "used 30%");
}

#[test]
fn notes_lose_control_characters_and_stop_at_200() {
    let c = reading::checked(cand(PathBuf::from("/p/a"), "used\n30%\x1b[0m\x07"));
    assert!(!c.note.chars().any(char::is_control), "{:?}", c.note);
    let long = "é".repeat(500);
    let c = reading::checked(cand(PathBuf::from("/p/a"), &long));
    assert!(c.note.chars().count() <= 200, "{}", c.note.chars().count());
}
