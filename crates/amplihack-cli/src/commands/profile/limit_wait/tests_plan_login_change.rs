//! Issue #2731: the login-change wait's plan and the promotion that reaches
//! it.
//!
//! An organisation spend limit that states no reset is not lifted by waiting,
//! and another login of the same organisation would not lift it either, so a
//! capacity wait does not apply. On a `$HOME/.claude` step with a rotator
//! running, though, the login that arrives next may belong to another
//! organisation: that step waits for the login to change instead.
//!
//! The promotion is separate from [`upgrade`], which stays as #1801 left it,
//! and the plan keeps [`PlanKind::Reset`] so the wait itself is unchanged and
//! never consults the stored readings.
//!
//! Wired from `limit_wait/mod.rs` with
//! `#[cfg(test)] mod tests_plan_login_change;`.

use super::plan::{
    Bound, CAPACITY_DEFAULT_MAX, NoWait, PlanKind, WaitFor, plan_login_change, upgrade_spend,
};
use crate::commands::recipe::run::limit_patterns::Harness;

/// 2026-10-18 08:00:00 -07:00.
const NOW: i64 = 1_792_335_600;
const LEAD: u64 = 60;

fn p(max: Option<u64>, step: Option<(i64, u64)>) -> super::plan::Plan {
    plan_login_change(NOW, max, step)
}

// --- the deadline ---------------------------------------------------------

#[test]
fn the_default_length_is_the_capacity_waits_six_hours() {
    let plan = p(None, None);
    assert_eq!(plan.until, NOW + CAPACITY_DEFAULT_MAX as i64);
    assert_eq!(plan.bound, Bound::MaxWait);
}

#[test]
fn the_maximum_sets_the_length() {
    let plan = p(Some(600), None);
    assert_eq!(plan.until, NOW + 600);
    assert_eq!(plan.bound, Bound::MaxWait);
}

#[test]
fn a_zero_maximum_expires_at_once() {
    let plan = p(Some(0), None);
    assert_eq!(plan.until, NOW, "no sleep, and no retry");
    assert_eq!(plan.bound, Bound::MaxWait);
}

#[test]
fn an_earlier_step_timeout_wins() {
    let plan = p(Some(600), Some((NOW, 300)));
    assert_eq!(plan.until, NOW + 300 - LEAD as i64);
    assert_eq!(plan.bound, Bound::StepTimeout);
}

#[test]
fn a_tie_is_named_max_wait() {
    let plan = p(Some(240), Some((NOW, 240 + LEAD)));
    assert_eq!(plan.until, NOW + 240);
    assert_eq!(
        plan.bound,
        Bound::MaxWait,
        "the earlier row of the deadline table names a tie"
    );
}

#[test]
fn a_step_timeout_of_zero_does_not_apply() {
    assert_eq!(p(Some(600), Some((NOW, 0))).bound, Bound::MaxWait);
}

#[test]
fn the_deadline_is_never_before_now() {
    // A step timeout already passed: the wait ends at once rather than
    // being planned into the past.
    let plan = p(Some(600), Some((NOW - 10_000, 30)));
    assert_eq!(plan.until, NOW);
    assert_eq!(plan.bound, Bound::StepTimeout);
}

#[test]
fn a_very_large_maximum_saturates_rather_than_overflowing() {
    let plan = p(Some(u64::MAX), None);
    assert!(plan.until > NOW);
    assert_eq!(plan.bound, Bound::MaxWait);
}

// --- the plan's shape -----------------------------------------------------

#[test]
fn there_is_no_reset_to_show() {
    assert_eq!(
        p(Some(600), None).reset,
        None,
        "the limit stated none, so the start line says reset=none"
    );
}

#[test]
fn the_wait_itself_is_a_reset_wait_so_no_reading_is_consulted() {
    // `PlanKind::Capacity` would make the wait probe the stored readings for
    // another login's capacity. No reading says whether the next login's
    // organisation has credit, and the rotator picks what goes into
    // `~/.claude` in any case.
    assert_eq!(p(Some(600), None).kind, PlanKind::Reset);
}

#[test]
fn there_is_no_five_hour_ceiling() {
    let eight_hours = 8 * 3600;
    assert_eq!(p(Some(eight_hours as u64), None).until, NOW + eight_hours);
}

// --- the promotion --------------------------------------------------------

const SPEND: Option<Harness> = Some(Harness::Claude);

#[test]
fn a_home_spend_limit_with_a_rotator_becomes_a_login_change_wait() {
    assert_eq!(
        upgrade_spend(WaitFor::Skip(NoWait::SpendLimit), SPEND, || true),
        WaitFor::LoginChange {
            harness: Harness::Claude
        }
    );
}

#[test]
fn without_a_rotator_the_spend_limit_keeps_its_skip() {
    assert_eq!(
        upgrade_spend(WaitFor::Skip(NoWait::SpendLimit), SPEND, || false),
        WaitFor::Skip(NoWait::SpendLimit),
        "only the rotator moves a login into ~/.claude, so with none \
         running there is nothing to wait for"
    );
}

#[test]
fn a_step_that_is_not_a_home_step_keeps_its_skip() {
    // `spend` is `None` for an inherited profile or an explicit
    // CLAUDE_CONFIG_DIR step: the rotator never changes their login.
    assert_eq!(
        upgrade_spend(WaitFor::Skip(NoWait::SpendLimit), None, || true),
        WaitFor::Skip(NoWait::SpendLimit)
    );
}

#[test]
fn no_other_skip_is_promoted() {
    for why in [
        NoWait::ResetTooFar,
        NoWait::NoReset,
        NoWait::NoLimitMessage,
        NoWait::PipedPrompt,
    ] {
        assert_eq!(
            upgrade_spend(WaitFor::Skip(why), SPEND, || true),
            WaitFor::Skip(why),
            "{why:?} is #1801's territory or no limit at all"
        );
    }
}

#[test]
fn a_reset_or_capacity_wait_is_never_promoted() {
    let reset = WaitFor::Reset {
        harness: Harness::Claude,
        reset: NOW + 1800,
    };
    assert_eq!(upgrade_spend(reset, SPEND, || true), reset);
    let capacity = WaitFor::Capacity {
        harness: Harness::Claude,
        reset: None,
    };
    assert_eq!(upgrade_spend(capacity, SPEND, || true), capacity);
}

#[test]
fn the_rotator_is_checked_only_for_a_spend_limit_that_could_be_promoted() {
    let calls = std::cell::Cell::new(0u32);
    let check = || {
        calls.set(calls.get() + 1);
        true
    };
    upgrade_spend(WaitFor::Skip(NoWait::ResetTooFar), SPEND, check);
    upgrade_spend(WaitFor::Skip(NoWait::SpendLimit), None, check);
    upgrade_spend(
        WaitFor::Reset {
            harness: Harness::Claude,
            reset: NOW,
        },
        SPEND,
        check,
    );
    assert_eq!(calls.get(), 0, "the check reads files; it must stay lazy");
    upgrade_spend(WaitFor::Skip(NoWait::SpendLimit), SPEND, check);
    assert_eq!(calls.get(), 1);
}
