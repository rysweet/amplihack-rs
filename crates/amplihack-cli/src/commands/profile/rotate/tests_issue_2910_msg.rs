//! Issue #2910: the lines that say which login the rotator passed over and
//! why. Fixed text only; a stored block reason is labelled, never quoted.
//!
//! The brief asks for a line per rejected candidate and exactly one line
//! when no candidate has room. What is pinned here is the text itself and
//! which cycles write it: the ALERT cycle, which is the one that found no
//! target, and not a cycle that swapped, where the `swap` line already says
//! what was chosen.

use super::{CONFIRM_SKIPPED, Reject, blocked, no_room, reject, rejections};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::rotate::decide::{Decision, Measured, Reading, Reason, Windows};
use crate::commands::profile::rotate::ledger::OverflowBlock;
use crate::commands::profile::rotate::settings::Settings;
use std::collections::BTreeMap;

const NOW: i64 = 1_793_389_376; // 2026-10-30T19:42:56Z

fn settings() -> Settings {
    Settings {
        session_at: 85,
        week_at: 90,
        margin: 5,
        upgrade_margin: 30,
        hold: 3600,
        interval: 600,
        overflow_wait: 1800,
        overflow_unknown_reset: false,
    }
}

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn m(name: &str, session: f64, week: f64) -> Measured {
    Measured {
        name: n(name),
        reading: Reading::Measured { session, week },
        windows: Windows::default(),
        spend_limit: false,
        unknown: false,
    }
}

fn unreadable(name: &str) -> Measured {
    Measured {
        name: n(name),
        reading: Reading::Failed,
        windows: Windows::default(),
        spend_limit: false,
        unknown: false,
    }
}

fn block(reason: &str, until: i64) -> OverflowBlock {
    OverflowBlock {
        at: NOW - 60,
        until,
        reason: reason.into(),
    }
}

fn blocks(entries: &[(&str, OverflowBlock)]) -> BTreeMap<String, OverflowBlock> {
    entries
        .iter()
        .map(|(n, b)| ((*n).to_string(), b.clone()))
        .collect()
}

#[test]
fn the_week_is_reported_before_the_session() {
    let s = settings();
    // Both windows are past their thresholds. The week is named, because
    // that is the bound the rotator used to ignore and the one the
    // 21 November 2026 07:20:31Z swap on candide turned on.
    let r = Reading::Measured {
        session: 88.0,
        week: 98.0,
    };
    assert_eq!(Reject::of(&s, &r), Some(Reject::Week(98)));
    assert_eq!(
        reject(&s, &n("ddrumpf"), Reject::of(&s, &r).unwrap()),
        "reject: ddrumpf week w=98% at or above --week-at 90"
    );
}

#[test]
fn each_reason_has_its_own_line() {
    let s = settings();
    let session = Reading::Measured {
        session: 88.0,
        week: 10.0,
    };
    assert_eq!(Reject::of(&s, &session), Some(Reject::Session(88)));
    assert_eq!(
        reject(&s, &n("aanderson"), Reject::Session(88)),
        "reject: aanderson session s=88% at or above --session-at 85"
    );
    assert_eq!(
        reject(&s, &n("nnorth"), Reject::Unread),
        "reject: nnorth no usable reading"
    );
    assert_eq!(
        no_room(&n("nnorth")),
        "reject: nnorth fresh reading shows no room"
    );
    assert_eq!(
        CONFIRM_SKIPPED,
        "confirm: skipped, no reading could be taken in this cycle"
    );
}

#[test]
fn a_login_with_room_has_no_reason_to_report() {
    let s = settings();
    for (session, week) in [(0.0, 0.0), (84.9, 89.9), (50.0, 50.0)] {
        let r = Reading::Measured { session, week };
        assert_eq!(Reject::of(&s, &r), None, "{session}/{week}");
    }
}

#[test]
fn a_failed_an_exhausted_and_an_unreadable_login_all_read_as_unread() {
    let s = settings();
    for r in [
        Reading::Failed,
        Reading::EXHAUSTED,
        Reading::Measured {
            session: f64::NAN,
            week: 10.0,
        },
        Reading::Measured {
            session: 10.0,
            week: f64::NAN,
        },
    ] {
        assert_eq!(Reject::of(&s, &r), Some(Reject::Unread), "{r:?}");
    }
}

#[test]
fn a_recorded_block_is_named_by_its_kind_and_its_reset() {
    assert_eq!(
        blocked(&n("ccapstone"), "spend_limit", 1_795_046_400),
        "reject: ccapstone spend limit recorded until 2026-11-19T00:00:00Z"
    );
    assert_eq!(
        blocked(&n("ccapstone"), "session_limit", 1_795_046_400),
        "reject: ccapstone session limit recorded until 2026-11-19T00:00:00Z"
    );
}

/// Issue #2909 widened the stored `reason` to any short token, so a writer
/// this binary does not know about still blocks its login. It must not be
/// able to choose the text of a log line with it: an unknown token is
/// labelled, and the token itself appears nowhere.
#[test]
fn an_unknown_limit_kind_is_labelled_and_never_quoted() {
    let odd = "some_future_limit_kind_0";
    let line = blocked(&n("ccapstone"), odd, 1_795_046_400);
    assert_eq!(
        line,
        "reject: ccapstone a recorded limit recorded until 2026-11-19T00:00:00Z"
    );
    assert!(!line.contains(odd), "{line}");
}

#[test]
fn only_the_alert_cycle_says_why_it_passed_each_candidate_over() {
    let s = settings();
    let measured = [m("work", 50.0, 50.0), m("spare", 10.0, 98.0)];
    let live = n("work");
    let none = BTreeMap::new();
    for d in [
        Decision::Stay,
        Decision::Swap {
            to: n("spare"),
            reason: Reason::Threshold,
        },
    ] {
        assert!(
            rejections(&s, &d, &measured, Some(&live), &none, NOW).is_empty(),
            "{d:?} already says what it did"
        );
    }
    assert_eq!(
        rejections(&s, &Decision::Alert, &measured, Some(&live), &none, NOW),
        ["reject: spare week w=98% at or above --week-at 90"],
        "the live login is left out: the cycle line already names it"
    );
}

/// One line per candidate passed over, in the order the candidates come in,
/// and a recorded block reported ahead of whatever its reading says. The
/// block is the evidence a limit event left, and the reading behind it may
/// be from before the limit was met.
#[test]
fn every_candidate_without_room_gets_one_line_with_its_own_reason() {
    let s = settings();
    let until = NOW + 4 * 3600;
    let measured = [
        m("ccapstone", 1.0, 1.0),
        m("ddrumpf", 10.0, 98.0),
        m("aanderson", 88.0, 10.0),
        unreadable("nnorth"),
    ];
    let b = blocks(&[("ccapstone", block("spend_limit", until))]);
    let lines = rejections(&s, &Decision::Alert, &measured, None, &b, NOW);
    assert_eq!(
        lines,
        [
            "reject: ccapstone spend limit recorded until 2026-10-30T23:42:56Z",
            "reject: ddrumpf week w=98% at or above --week-at 90",
            "reject: aanderson session s=88% at or above --session-at 85",
            "reject: nnorth no usable reading",
        ]
    );
}

/// A block whose reset has passed is not a reason to pass a login over. The
/// rotator wakes at that reset and the login is an ordinary candidate
/// again, on the same reading as before.
#[test]
fn a_lapsed_block_stops_counting_against_its_login() {
    let s = settings();
    let measured = [m("ccapstone", 1.0, 1.0)];
    let live_b = blocks(&[("ccapstone", block("session_limit", NOW + 1))]);
    assert_eq!(
        rejections(&s, &Decision::Alert, &measured, None, &live_b, NOW).len(),
        1,
        "still in force one second before its reset"
    );
    let lapsed = blocks(&[("ccapstone", block("session_limit", NOW))]);
    assert!(
        rejections(&s, &Decision::Alert, &measured, None, &lapsed, NOW).is_empty(),
        "released at its reset, and it has room"
    );
}

/// Every line this module writes begins with its kind and holds nothing but
/// fixed text, a profile name, a whole percentage, a threshold and a
/// timestamp. No value a writer chose reaches a line, and no figure is
/// written with a fraction, which the rotate log grammar does not admit.
#[test]
fn every_line_is_fixed_text_with_no_fractions() {
    let s = settings();
    let measured = [
        m("ddrumpf", 10.0, 98.6),
        m("aanderson", 88.4, 10.0),
        unreadable("nnorth"),
    ];
    let b = blocks(&[("nnorth", block("week_limit", NOW + 60))]);
    let mut lines = rejections(&s, &Decision::Alert, &measured, None, &b, NOW);
    lines.push(no_room(&n("ddrumpf")));
    lines.push(CONFIRM_SKIPPED.to_string());
    for line in &lines {
        assert!(
            line.starts_with("reject: ") || line.starts_with("confirm: "),
            "{line}"
        );
        assert!(!line.contains('.'), "a fraction in {line}");
        assert!(!line.contains('\n') && !line.contains('\r'), "{line}");
    }
    assert!(lines[0].contains("w=98%"), "{:?}", lines[0]);
    assert!(lines[1].contains("s=88%"), "{:?}", lines[1]);
}
