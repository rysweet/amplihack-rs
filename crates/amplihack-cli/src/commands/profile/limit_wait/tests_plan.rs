//! `plan` (issue #1618): the deadline table. The earliest applicable row
//! wins, a tie is named by the earlier row, a reset at or before now means
//! now, and no input can push the deadline past now + 5 h + 120 s.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_plan;`.

use super::plan::{Bound, Plan, plan};

/// 2026-10-18 08:00:00 -07:00.
const NOW: i64 = 1_792_335_600;
const MARGIN: i64 = 120;
const FIVE_HOURS: i64 = 5 * 3600;
const CEILING: i64 = FIVE_HOURS + MARGIN;

fn p(reset: Option<i64>, max: Option<u64>, step: Option<(i64, u64)>) -> Plan {
    plan(NOW, reset, max, step)
}

#[test]
fn a_future_reset_waits_until_reset_plus_margin() {
    let got = p(Some(NOW + 1800), None, None);
    assert_eq!(got.until, NOW + 1800 + MARGIN);
    assert_eq!(got.bound, Bound::ResetMargin);
    assert_eq!(got.reset, Some(NOW + 1800));
}

#[test]
fn a_reset_at_now_waits_zero_seconds() {
    let got = p(Some(NOW), None, None);
    assert_eq!((got.until, got.bound), (NOW, Bound::ResetMargin));
}

#[test]
fn a_past_reset_waits_zero_seconds_and_keeps_the_reset_as_read() {
    let got = p(Some(NOW - 300), None, None);
    assert_eq!((got.until, got.bound), (NOW, Bound::ResetMargin));
    assert_eq!(
        got.reset,
        Some(NOW - 300),
        "the log prints the reset as read"
    );
}

#[test]
fn a_reset_beyond_five_hours_is_capped() {
    let got = p(Some(NOW + 40_000), None, None);
    assert_eq!((got.until, got.bound), (NOW + CEILING, Bound::ResetMargin));
}

#[test]
fn no_reset_waits_the_thirty_minute_default() {
    let got = p(None, None, None);
    assert_eq!(
        (got.until, got.bound, got.reset),
        (NOW + 1800, Bound::Default, None)
    );
}

#[test]
fn a_smaller_maximum_wins() {
    let got = p(Some(NOW + 1800), Some(600), None);
    assert_eq!((got.until, got.bound), (NOW + 600, Bound::MaxWait));
    let got = p(None, Some(60), None);
    assert_eq!((got.until, got.bound), (NOW + 60, Bound::MaxWait));
}

#[test]
fn a_larger_maximum_changes_nothing() {
    let got = p(Some(NOW + 1800), Some(7200), None);
    assert_eq!((got.until, got.bound), (NOW + 1920, Bound::ResetMargin));
    let got = p(None, Some(u64::MAX), None);
    assert_eq!((got.until, got.bound), (NOW + 1800, Bound::Default));
}

#[test]
fn a_zero_maximum_waits_zero_seconds() {
    let got = p(Some(NOW + 1800), Some(0), None);
    assert_eq!((got.until, got.bound), (NOW, Bound::MaxWait));
    let got = p(None, Some(0), None);
    assert_eq!((got.until, got.bound), (NOW, Bound::MaxWait));
}

#[test]
fn a_tie_is_named_by_the_earlier_row() {
    // reset + margin == now + max.
    let got = p(Some(NOW + 480), Some(600), None);
    assert_eq!((got.until, got.bound), (NOW + 600, Bound::ResetMargin));
    // default == now + max.
    let got = p(None, Some(1800), None);
    assert_eq!((got.until, got.bound), (NOW + 1800, Bound::Default));
    // past reset (now) == max 0 (now).
    let got = p(Some(NOW - 1), Some(0), None);
    assert_eq!((got.until, got.bound), (NOW, Bound::ResetMargin));
    // max == step timeout.
    let got = p(None, Some(600), Some((NOW, 660)));
    assert_eq!((got.until, got.bound), (NOW + 600, Bound::MaxWait));
    // default == step timeout.
    let got = p(None, None, Some((NOW, 1860)));
    assert_eq!((got.until, got.bound), (NOW + 1800, Bound::Default));
}

#[test]
fn a_step_timeout_ends_the_wait_sixty_seconds_before_it() {
    let got = p(Some(NOW + 1800), None, Some((NOW - 100, 1000)));
    assert_eq!((got.until, got.bound), (NOW + 840, Bound::StepTimeout));
}

#[test]
fn a_step_timeout_of_sixty_seconds_or_less_gives_now() {
    for t in [1u64, 30, 59, 60] {
        let got = p(Some(NOW + 1800), None, Some((NOW, t)));
        assert_eq!(
            (got.until, got.bound),
            (NOW, Bound::StepTimeout),
            "timeout {t}"
        );
    }
}

#[test]
fn a_used_up_step_timeout_gives_now() {
    let got = p(Some(NOW + 1800), None, Some((NOW - 5000, 1000)));
    assert_eq!((got.until, got.bound), (NOW, Bound::StepTimeout));
}

#[test]
fn a_zero_step_timeout_does_not_apply() {
    let got = p(Some(NOW + 1800), None, Some((NOW, 0)));
    assert_eq!((got.until, got.bound), (NOW + 1920, Bound::ResetMargin));
}

#[test]
fn a_longer_step_timeout_changes_nothing() {
    let got = p(None, None, Some((NOW, 7200)));
    assert_eq!((got.until, got.bound), (NOW + 1800, Bound::Default));
}

#[test]
fn bound_names_match_the_log_table() {
    assert_eq!(Bound::ResetMargin.as_str(), "reset+margin");
    assert_eq!(Bound::Default.as_str(), "default");
    assert_eq!(Bound::MaxWait.as_str(), "max_wait");
    assert_eq!(Bound::StepTimeout.as_str(), "step_timeout");
}

#[test]
fn extreme_inputs_saturate_without_panicking() {
    let got = p(Some(i64::MAX), Some(u64::MAX), Some((i64::MAX, u64::MAX)));
    assert_eq!(got.until, NOW + CEILING);
    let got = p(Some(i64::MIN), None, Some((i64::MIN, u64::MAX)));
    assert_eq!(got.until, NOW);
    let got = plan(i64::MAX - 10, Some(i64::MAX), Some(u64::MAX), None);
    assert!(got.until >= i64::MAX - 10);
    let got = plan(i64::MIN, None, None, Some((i64::MIN, 1)));
    assert_eq!(got.until, i64::MIN, "never earlier than now");
    let got = plan(i64::MAX, None, None, None);
    assert_eq!(got.until, i64::MAX);
}

#[test]
fn until_is_always_between_now_and_the_ceiling() {
    let resets = [
        None,
        Some(i64::MIN),
        Some(NOW - 86_400),
        Some(NOW - 1),
        Some(NOW),
        Some(NOW + 1),
        Some(NOW + 1799),
        Some(NOW + FIVE_HOURS),
        Some(NOW + FIVE_HOURS + 1),
        Some(i64::MAX),
    ];
    let maxes = [
        None,
        Some(0),
        Some(1),
        Some(1800),
        Some(18_120),
        Some(u64::MAX),
    ];
    let steps = [
        None,
        Some((NOW, 0)),
        Some((NOW, 61)),
        Some((NOW - 10, 3600)),
        Some((i64::MIN, u64::MAX)),
        Some((i64::MAX, u64::MAX)),
    ];
    for r in resets {
        for m in maxes {
            for s in steps {
                let got = p(r, m, s);
                assert!(
                    got.until >= NOW && got.until - NOW <= CEILING,
                    "reset {r:?} max {m:?} step {s:?} gave until-now = {}",
                    got.until.saturating_sub(NOW)
                );
            }
        }
    }
}
