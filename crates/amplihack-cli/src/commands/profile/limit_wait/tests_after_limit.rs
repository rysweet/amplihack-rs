//! Issue #1761: what `after_limit` decides before the budget is read
//! ([`check`]), in the documented order: a stop signal, the wait disabled
//! (or off in CI), the rotator check (made once, and never
//! for a step stopped earlier), the per-step rule, and a limit no wait can
//! outlast. Without a rotator a step waits once (`already_waited`); with
//! one it waits up to `max_waits_per_step` times (`step_waits_spent`).
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_after_limit;`.

use super::after_limit::{Check, check};
use super::events::Skip;
use super::plan::{NoWait, WaitFor};
use super::settings::WaitSettings;
use crate::commands::recipe::run::limit_patterns::Harness;
use std::cell::Cell;

const RESET: i64 = 1_792_337_400;

const ON: WaitSettings = WaitSettings {
    enabled: true,
    max_wait_secs: None,
    off_in_ci: false,
    budget_secs: 21_600,
    max_waits_per_step: 5,
};

fn reset() -> WaitFor {
    WaitFor::Reset {
        harness: Harness::Claude,
        reset: RESET,
    }
}

fn wait(rotator: bool) -> Check {
    Check::Wait {
        harness: Harness::Claude,
        reset: RESET,
        rotator,
    }
}

fn finish(code: Option<i32>, skip: Option<Skip>) -> Check {
    Check::Finish { code, skip }
}

/// `check` with a rotator check that answers `rotator`, and how many times
/// it was asked.
fn run(
    settings: WaitSettings,
    signal: Option<i32>,
    waits: u32,
    for_: WaitFor,
    rotator: bool,
) -> (Check, u32) {
    let asked = Cell::new(0);
    let got = check(settings, signal, waits, for_, || {
        asked.set(asked.get() + 1);
        rotator
    });
    (got, asked.get())
}

#[test]
fn a_stop_signal_ends_the_step_before_any_check() {
    for s in [
        ON,
        WaitSettings {
            enabled: false,
            ..ON
        },
    ] {
        assert_eq!(
            run(s, Some(libc::SIGTERM), 0, reset(), true),
            (finish(Some(128 + libc::SIGTERM), None), 0)
        );
    }
}

#[test]
fn a_disabled_wait_never_asks_for_the_rotator() {
    let off = WaitSettings {
        enabled: false,
        ..ON
    };
    let ci = WaitSettings {
        enabled: false,
        off_in_ci: true,
        ..ON
    };
    for waits in [0, 1, 5] {
        assert_eq!(
            run(off, None, waits, reset(), true),
            (finish(None, Some(Skip::Disabled)), 0)
        );
        assert_eq!(
            run(ci, None, waits, reset(), true),
            (finish(None, Some(Skip::Ci)), 0)
        );
    }
}

#[test]
fn the_first_limit_waits_either_way_and_reports_the_rotator() {
    assert_eq!(run(ON, None, 0, reset(), false), (wait(false), 1));
    assert_eq!(run(ON, None, 0, reset(), true), (wait(true), 1));
}

#[test]
fn without_a_rotator_a_step_waits_once() {
    for waits in [1, 2, 5, u32::MAX] {
        assert_eq!(
            run(ON, None, waits, reset(), false),
            (finish(None, Some(Skip::AlreadyWaited)), 1),
            "{waits}"
        );
    }
}

#[test]
fn with_a_rotator_a_step_waits_up_to_its_cap() {
    for waits in 1..5 {
        assert_eq!(
            run(ON, None, waits, reset(), true),
            (wait(true), 1),
            "{waits}"
        );
    }
    for waits in [5, 6, u32::MAX] {
        assert_eq!(
            run(ON, None, waits, reset(), true),
            (finish(None, Some(Skip::StepWaitsSpent)), 1),
            "{waits}"
        );
    }
}

#[test]
fn a_cap_of_one_waits_once_either_way() {
    let one = WaitSettings {
        max_waits_per_step: 1,
        ..ON
    };
    assert_eq!(run(one, None, 0, reset(), true), (wait(true), 1));
    assert_eq!(
        run(one, None, 1, reset(), true),
        (finish(None, Some(Skip::StepWaitsSpent)), 1)
    );
    assert_eq!(
        run(one, None, 1, reset(), false),
        (finish(None, Some(Skip::AlreadyWaited)), 1)
    );
}

#[test]
fn each_limit_is_judged_on_its_own_rotator_check() {
    // Waited once with no rotator; one has started since.
    assert_eq!(run(ON, None, 1, reset(), true), (wait(true), 1));
    // Waited with a rotator; none is seen now.
    assert_eq!(
        run(ON, None, 3, reset(), false),
        (finish(None, Some(Skip::AlreadyWaited)), 1)
    );
}

#[test]
fn a_limit_no_wait_can_outlast_is_passed_up_after_the_per_step_rule() {
    let spend = WaitFor::Skip(NoWait::SpendLimit);
    for rotator in [false, true] {
        assert_eq!(
            run(ON, None, 0, spend, rotator),
            (finish(None, Some(Skip::Cannot(NoWait::SpendLimit))), 1)
        );
    }
    assert_eq!(
        run(ON, None, 1, spend, false),
        (finish(None, Some(Skip::AlreadyWaited)), 1)
    );
    assert_eq!(
        run(ON, None, 5, spend, true),
        (finish(None, Some(Skip::StepWaitsSpent)), 1)
    );
    assert_eq!(
        run(ON, None, 1, WaitFor::Skip(NoWait::PipedPrompt), true),
        (finish(None, Some(Skip::Cannot(NoWait::PipedPrompt))), 1)
    );
}

#[test]
fn a_weekly_or_unreadable_reset_is_judged_after_the_rotator_and_per_step_rule() {
    // #1796 scope: such a limit ends the step even with a rotator, but the
    // rotator check and the per-step rule come first, so a step that has
    // used its waits logs `already_waited` or `step_waits_spent`, not the
    // limit's own reason. docs/reference/profile-command.md states this.
    for why in [NoWait::ResetTooFar, NoWait::NoReset] {
        for rotator in [false, true] {
            assert_eq!(
                run(ON, None, 0, WaitFor::Skip(why), rotator),
                (finish(None, Some(Skip::Cannot(why))), 1)
            );
        }
        assert_eq!(
            run(ON, None, 1, WaitFor::Skip(why), true),
            (finish(None, Some(Skip::Cannot(why))), 1)
        );
        assert_eq!(
            run(ON, None, 1, WaitFor::Skip(why), false),
            (finish(None, Some(Skip::AlreadyWaited)), 1)
        );
        assert_eq!(
            run(ON, None, 5, WaitFor::Skip(why), true),
            (finish(None, Some(Skip::StepWaitsSpent)), 1)
        );
    }
}
