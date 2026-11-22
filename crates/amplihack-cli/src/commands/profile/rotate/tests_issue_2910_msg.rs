//! Issue #2910: the lines that say which login the rotator passed over and
//! why. Fixed text only; a stored block reason is labelled, never quoted.
//!
//! The brief asks for a line per rejected candidate and exactly one line
//! when no candidate has room. What is pinned here is the text itself and
//! which cycles write it: a cycle that looked for a target, whether it
//! swapped or found none, and not a cycle that stayed.

use super::{
    CONFIRM_LOCKED, CONFIRM_SKIPPED, Reject, WAKE_REARMED, blocked, cause, no_room, reject,
    rejections, wake_failed, wake_unavailable,
};
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
    assert_eq!(
        CONFIRM_LOCKED,
        "confirm: skipped, no reading is taken under the lock"
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
fn an_exhausted_login_has_no_room_left_and_a_failed_or_unreadable_one_no_reading() {
    let s = settings();
    assert_eq!(Reject::of(&s, &Reading::EXHAUSTED), Some(Reject::Spent));
    assert_eq!(
        reject(&s, &n("nnorth"), Reject::Spent),
        "reject: nnorth no room left"
    );
    for r in [
        Reading::Failed,
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

/// A cycle that stays was not choosing a target, so it names no candidate.
/// A cycle that swapped and the ALERT both did choose, and both say why
/// each other login was passed over. The live login and the swap's own
/// target are left out: the `cycle` and `swap` lines already name them.
#[test]
fn a_cycle_that_chose_a_target_says_why_it_passed_each_other_candidate_over() {
    let s = settings();
    let until = NOW + 4 * 3600;
    let measured = [
        m("work", 50.0, 50.0),
        m("spare", 10.0, 98.0),
        m("extra", 20.0, 30.0),
        m("blocked", 1.0, 1.0),
    ];
    let live = n("work");
    let b = blocks(&[("blocked", block("session_limit", until))]);
    assert!(
        rejections(&s, &Decision::Stay, &measured, Some(&live), &b, NOW).is_empty(),
        "a cycle that stays was not choosing"
    );
    let swap = Decision::Swap {
        to: n("extra"),
        reason: Reason::Threshold,
    };
    let expected = [
        "reject: spare week w=98% at or above --week-at 90".to_string(),
        "reject: blocked session limit recorded until 2026-10-30T23:42:56Z".to_string(),
    ];
    assert_eq!(
        rejections(&s, &swap, &measured, Some(&live), &b, NOW),
        expected,
        "a swap names the logins it passed over, the block included, and not its target"
    );
    // The decision is a stand-in: `rejections` reports each reading against
    // the target rules, so extra, which has room, is not named either way.
    assert_eq!(
        rejections(&s, &Decision::Alert, &measured, Some(&live), &b, NOW),
        expected,
        "the ALERT names every other login without room"
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
    lines.push(CONFIRM_LOCKED.to_string());
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

/// Issue #2909: the lines that say the event intake could not be made, or
/// was lost and rebuilt. The cause is fixed words chosen from the error's
/// kind and number, never the operating system's message, so the line is
/// evidence an operator can act on and carries no text a path or a peer
/// chose. The two inotify limits are named, because they are the likely
/// causes on a busy host.
#[test]
fn a_wake_failure_names_a_fixed_cause_and_never_the_os_message() {
    use std::io::{Error, ErrorKind};
    let cases = [
        (Error::from(ErrorKind::Unsupported), "not supported here"),
        (
            Error::from_raw_os_error(libc::ENOSPC),
            "inotify watch limit reached",
        ),
        (
            Error::from_raw_os_error(libc::EMFILE),
            "descriptor or inotify instance limit reached",
        ),
        (
            Error::from(ErrorKind::PermissionDenied),
            "permission denied",
        ),
        (Error::other("/home/someone/.claude/x"), "I/O error"),
    ];
    for (e, want) in &cases {
        assert_eq!(cause(e), *want, "{e:?}");
        assert_eq!(
            wake_unavailable(e),
            format!("wake: unavailable ({want}), using --interval")
        );
        assert_eq!(
            wake_failed(e),
            format!("wake: lost ({want}), using --interval")
        );
    }
    assert_eq!(WAKE_REARMED, "wake: watch rebuilt");
}
