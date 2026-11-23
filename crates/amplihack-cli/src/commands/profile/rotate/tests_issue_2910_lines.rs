//! Issue #2910 (D9 to D11, T6, S5, S6, A5): the text of the stay notes, the
//! `reject:` line, the time shown in a count and the alert keys. Wired from
//! `target_msg.rs`. Issue #3105 withdrew the unconfirmed swap, and with it
//! the swap line's suffix; the two `reject:` forms it added are tested in
//! `tests_issue_3105`.
//!
//! The grammar is the one in `docs/reference/profile-command.md`, "Rotate no
//! target" and "Rotate confirmation". The summary line, with its counts, is
//! checked on whole cycles in `tests_issue_2910_cycle`, where it is built from
//! real readings.
//!
//! Every text here is fixed or built from a profile name, a whole number or a
//! label this binary owns. None carries what a server, a credentials file or a
//! stored reason chose (I5).
//!
//! The API these tests exercise (in `target_msg.rs`):
//!
//! ```text
//! utc_minute(t: i64) -> String                      // %Y-%m-%dT%H:%MZ, fixed fallback
//! reject(who: &ProfileName, why: &Why, week_max: u8) -> String   // "reject: <name> <reason>"
//! stay_note(m: &NoMove, s: &Settings) -> Option<String>   // None for NoTarget
//! ```

use super::*;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::rotate::alert::{alert_key, valid_alerts};
use crate::commands::profile::rotate::decide::target::{NoMove, Unfit, no_move};
use crate::commands::profile::rotate::decide::target_confirm::Why;
use crate::commands::profile::rotate::decide::{Measured, Reading, Windows};
use crate::commands::profile::rotate::measure::ProbeLabel;
use crate::commands::profile::rotate::settings::Settings;
use crate::commands::profile::rotate::tests_swap::env;

const NOW: i64 = 1_795_000_000;

fn settings() -> Settings {
    Settings {
        session_at: 85,
        week_at: 90,
        margin: 3,
        upgrade_margin: 30,
        hold: 3600,
        interval: 60,
        overflow_wait: 1800,
        overflow_unknown_reset: false,
        target_week_max: 95,
    }
}

fn m(who: &str, session: f64, week: f64) -> Measured {
    Measured {
        name: ProfileName::parse(who).unwrap(),
        reading: Reading::Measured { session, week },
        windows: Windows::default(),
        spend_limit: false,
        unknown: false,
    }
}

fn live(session: f64, week: f64) -> Reading {
    Reading::Measured { session, week }
}

// --- The stay notes (D10) ---------------------------------------------------------------

#[test]
fn the_note_for_a_move_that_needs_more_room() {
    let c = [m("llarame", 0.0, 95.0), m("qquinn", 88.0, 20.0)];
    let nm = no_move(&settings(), &live(86.0, 65.0), &c, None, NOW);
    assert_eq!(
        stay_note(&nm, &settings()).as_deref(),
        Some("stay: live room 14, best other login has room 12, a move needs 17")
    );
}

/// The same note at the thresholds the hosts run, 95/98: a live login at 97%
/// (room 3) and the best other login at 95.5% (room 4), short of the margin.
#[test]
fn the_note_for_a_move_that_needs_more_room_at_the_deployed_settings() {
    let s = Settings {
        session_at: 95,
        week_at: 98,
        ..settings()
    };
    let c = [m("edge", 95.5, 10.0)];
    let nm = no_move(&s, &live(97.0, 50.0), &c, None, NOW);
    assert_eq!(
        stay_note(&nm, &s).as_deref(),
        Some("stay: live room 3, best other login has room 4, a move needs 6")
    );
}

/// A figure the cycle did not measure is said in words, never printed as 0
/// (review of PR #3048): a live probe that read nothing has no room figure,
/// and when only a login in another organisation has room, no login the
/// rotator may move to has one either.
#[test]
fn the_note_never_prints_an_unmeasured_figure_as_zero() {
    // The live probe failed: a move needs the margin, and qquinn has 2.
    let c = [m("qquinn", 98.0, 20.0)];
    let nm = no_move(&settings(), &Reading::Failed, &c, None, NOW);
    assert_eq!(
        nm,
        NoMove::LessRoom {
            most: Some(2),
            room: None,
            needs: 3
        }
    );
    assert_eq!(
        stay_note(&nm, &settings()).as_deref(),
        Some("stay: live room unknown, best other login has room 2, a move needs 3")
    );
    // S1: the live login has room 14, and only another organisation's login
    // has room, which a live login with room never crosses to.
    let nm = NoMove::LessRoom {
        most: None,
        room: Some(14),
        needs: 17,
    };
    assert_eq!(
        stay_note(&nm, &settings()).as_deref(),
        Some("stay: live room 14, only a login in another organisation has room, a move needs 17")
    );
}

#[test]
fn the_note_for_a_move_the_hold_stops() {
    let c = [m("qquinn", 88.0, 20.0)];
    let nm = no_move(&settings(), &live(0.0, 95.0), &c, Some(NOW - 600), NOW);
    assert_eq!(
        stay_note(&nm, &settings()).as_deref(),
        Some(
            "stay: held after last swap (600s of 3600s): qquinn is at s=88%, at or above --session-at"
        )
    );
}

#[test]
fn the_held_note_counts_from_zero_and_never_goes_negative() {
    let c = [m("qquinn", 88.0, 20.0)];
    let nm = no_move(&settings(), &live(0.0, 95.0), &c, Some(NOW), NOW);
    let note = stay_note(&nm, &settings()).expect("held");
    assert!(note.contains("(0s of 3600s)"), "{note}");
    // The hold in the note is the setting.
    let long = Settings {
        hold: 7200,
        ..settings()
    };
    let nm = no_move(&long, &live(0.0, 95.0), &c, Some(NOW - 5), NOW);
    let note = stay_note(&nm, &long).expect("held");
    assert!(note.contains("(5s of 7200s)"), "{note}");
}

#[test]
fn the_note_for_a_cycle_where_no_target_was_confirmed() {
    assert_eq!(
        stay_note(&NoMove::Unconfirmed, &settings()).as_deref(),
        Some("stay: no candidate confirmed by a fresh reading")
    );
}

#[test]
fn no_eligible_login_is_a_summary_and_not_a_note() {
    assert_eq!(stay_note(&NoMove::NoTarget, &settings()), None);
}

#[test]
fn a_note_is_one_line_of_plain_text() {
    let c = [m("qquinn", 88.0, 20.0), m("llarame", 0.0, 95.0)];
    for nm in [
        no_move(&settings(), &live(86.0, 65.0), &c, None, NOW),
        no_move(&settings(), &live(0.0, 95.0), &c[..1], Some(NOW - 1), NOW),
        NoMove::Unconfirmed,
    ] {
        let note = stay_note(&nm, &settings()).expect("a note");
        assert!(note.starts_with("stay: "), "{note}");
        assert!(!note.contains('\n') && note.is_ascii(), "{note:?}");
    }
}

// --- reject: (D11) -------------------------------------------------------------------------

fn who(n: &str) -> ProfileName {
    ProfileName::parse(n).unwrap()
}

#[test]
fn each_reason_has_its_fixed_text() {
    let l = who("llarame");
    for (why, text) in [
        (
            Why::Unfit(Unfit::Full),
            "reject: llarame no room on a fresh reading",
        ),
        (
            Why::Unfit(Unfit::AboveCeiling),
            "reject: llarame week above ceiling 95% on a fresh reading",
        ),
        (
            Why::Unfit(Unfit::SpendLimit),
            "reject: llarame spend limit on a fresh reading",
        ),
        (
            Why::Unfit(Unfit::NoReading),
            "reject: llarame no usable reading",
        ),
        (
            Why::Unknown(ProbeLabel::Http429),
            "reject: llarame reading unknown (HTTP 429)",
        ),
        (
            Why::Unknown(ProbeLabel::IdentityUnavailable),
            "reject: llarame reading unknown (identity unavailable)",
        ),
    ] {
        assert_eq!(reject(&l, &why, 95), text);
    }
}

#[test]
fn the_ceiling_in_a_reject_line_is_the_setting() {
    let line = reject(&who("a"), &Why::Unfit(Unfit::AboveCeiling), 80);
    assert_eq!(line, "reject: a week above ceiling 80% on a fresh reading");
}

#[test]
fn a_reject_line_names_the_login_and_nothing_else_it_was_given() {
    for label in [
        ProbeLabel::Http429,
        ProbeLabel::Transport,
        ProbeLabel::IdentityUnavailable,
        ProbeLabel::Timeout,
        ProbeLabel::StillRunning,
        ProbeLabel::ProbeError,
    ] {
        let line = reject(&who("llarame"), &Why::Unknown(label), 95);
        assert!(line.is_ascii() && !line.contains('\n'), "{line:?}");
        assert!(line.contains(label.as_str()), "{line}");
    }
}

// --- The time in a count (A5, S6) ------------------------------------------------------------------

fn expected_minute(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .unwrap()
        .format("%Y-%m-%dT%H:%MZ")
        .to_string()
}

#[test]
fn a_time_is_shown_in_utc_to_the_minute() {
    assert_eq!(utc_minute(0), "1970-01-01T00:00Z");
    assert_eq!(utc_minute(1_795_000_000), expected_minute(1_795_000_000));
    // The seconds are dropped, not rounded.
    let base = 1_795_000_040; // 20 seconds into a minute (1_795_000_020 is on one)
    assert_eq!(utc_minute(base), utc_minute(base - 20));
    assert_eq!(utc_minute(base + 39), utc_minute(base - 20));
    assert_ne!(utc_minute(base + 40), utc_minute(base - 20));
}

#[test]
fn a_time_nobody_can_show_gets_one_fixed_text_and_never_a_panic() {
    let low = utc_minute(i64::MIN);
    let high = utc_minute(i64::MAX);
    assert_eq!(low, high, "one fixed fallback");
    assert!(
        !low.is_empty() && low.is_ascii() && !low.contains('\n'),
        "{low:?}"
    );
    let _ = utc_minute(i64::MIN + 1);
    let _ = utc_minute(i64::MAX - 1);
    let _ = utc_minute(-1);
}

// --- The alert keys (T6) -------------------------------------------------------------------------------

const KINDS: &[&str] = &[
    "no-capacity",
    "no-capacity/known",
    "no-capacity/stay",
    "no-capacity/held",
    "no-capacity/unconfirmed",
];

#[test]
fn a_key_of_each_kind_is_a_key_an_older_rotator_still_loads() {
    let long = ProfileName::parse(&"a".repeat(64)).unwrap();
    for kind in KINDS {
        let key = alert_key(kind, Some(&long));
        assert!(key.len() <= 89, "{key} is {} bytes", key.len());
        assert!(key.bytes().all(|b| (0x20..0x7f).contains(&b)), "{key:?}");
        for advisory in [false, true] {
            let mut l = crate::commands::profile::rotate::ledger::RotateLedger::default();
            if advisory {
                l.advisory_alert = Some(key.clone());
            } else {
                l.alert = Some(key.clone());
            }
            assert!(valid_alerts(&l), "{key}");
        }
    }
}

#[test]
fn a_ledger_holding_a_key_of_each_kind_round_trips() {
    let e = env();
    for kind in KINDS {
        let mut l = e.rs.load_ledger().unwrap().unwrap();
        l.alert = None;
        l.advisory_alert = Some(alert_key(kind, Some(&ProfileName::parse("work").unwrap())));
        e.rs.save_ledger(&l).unwrap();
        let back = e.rs.load_ledger().unwrap().unwrap();
        assert_eq!(back.advisory_alert, l.advisory_alert, "{kind}");
    }
}

#[test]
fn the_kinds_are_distinct_keys_for_one_login() {
    let work = ProfileName::parse("work").unwrap();
    let keys: std::collections::BTreeSet<String> =
        KINDS.iter().map(|k| alert_key(k, Some(&work))).collect();
    assert_eq!(
        keys.len(),
        KINDS.len(),
        "a change of wording writes a new line"
    );
}
