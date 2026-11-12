//! Issue #1700 (D8): `profile switch` onto an API-key profile, and the two
//! texts it adds. The end-to-end behaviour (the `{}` login moved by the
//! journalled rename swap, the pin, the refusal while the key is blocked,
//! the held cycle that never probes the key) is in
//! `tests/profile_platform.rs`; this file holds the texts to the reference.
//!
//! Contract:
//! - `log::msg::switch_platform_note(&ProfileName) -> String`: the one line
//!   a successful switch onto an API-key profile prints, naming the profile
//!   and `amplihack profile switch --none`.
//! - `ProfileError::PlatformBlocked { name: String, reason: String, until:
//!   i64 }`: `switch` onto an API-key profile while the API-key block is
//!   active. Displays `profile <name>: the API key is blocked (<reason>)
//!   until <time>`, `<time>` as `log::timestamp`; its hint is
//!   `amplihack profile overflow clear --platform`.

use super::log::{msg, timestamp};
use crate::commands::profile::ProfileError;
use crate::commands::profile::name::ProfileName;

const REFERENCE: &str = include_str!("../../../../../../docs/reference/profile-command.md");

/// 2026-12-29T10:14:03Z, the time in the reference's example.
const UNTIL: i64 = 1_798_539_243;

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

#[test]
fn the_switch_note_names_the_profile_and_the_release_command() {
    let note = msg::switch_platform_note(&n("console"));
    assert_eq!(
        note,
        "note: console is an API-key profile; work in ~/.claude is billed per token \
         until you run: amplihack profile switch --none"
    );
    assert!(!note.contains('\n'), "one line");
}

#[test]
fn the_switch_note_is_quoted_word_for_word() {
    let note = msg::switch_platform_note(&n("api-key"));
    assert!(REFERENCE.contains(&note), "reference lacks: {note}");
}

#[test]
fn platform_blocked_names_the_profile_reason_and_end() {
    let e = ProfileError::PlatformBlocked {
        name: "console".into(),
        reason: "key_revoked".into(),
        until: UNTIL,
    };
    assert_eq!(
        e.to_string(),
        format!(
            "profile console: the API key is blocked (key_revoked) until {}",
            timestamp(UNTIL)
        )
    );
    assert_eq!(e.hint(), "amplihack profile overflow clear --platform");
}

#[test]
fn platform_blocked_is_quoted_word_for_word() {
    let e = ProfileError::PlatformBlocked {
        name: "api-key".into(),
        reason: "key_exhausted".into(),
        until: UNTIL,
    };
    for line in [format!("error: {e}"), format!("hint: {}", e.hint())] {
        assert!(REFERENCE.contains(&line), "reference lacks: {line}");
    }
}
