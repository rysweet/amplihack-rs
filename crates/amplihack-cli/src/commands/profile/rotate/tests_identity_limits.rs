//! Issue #1973: the identity budget and the identity read timeout may be
//! raised in debug builds through `AMPLIHACK_TEST_IDENTITY_BUDGET_SECS` and
//! `AMPLIHACK_TEST_IDENTITY_READ_TIMEOUT_SECS`, so that the `profile rotate`
//! integration tests do not fail on a loaded host. Release builds always use
//! the constants, and neither constant changes.
//!
//! These tests never write the process environment: every module of the
//! library's unit tests shares one process. The parser is tested as a pure
//! function, and the env-reading functions are checked against whatever the
//! environment of this process holds.

use crate::commands::profile::rotate::cred_identity::{
    IDENTITY_BUDGET, IDENTITY_READ_TIMEOUT, identity_budget, identity_read_timeout,
};
use std::time::Duration;

const DEFAULT: Duration = Duration::from_secs(30);

#[test]
fn the_constants_keep_their_values() {
    assert_eq!(IDENTITY_BUDGET, Duration::from_secs(30));
    assert_eq!(IDENTITY_READ_TIMEOUT, Duration::from_secs(5));
}

#[cfg(debug_assertions)]
mod debug {
    use super::*;
    use crate::commands::profile::rotate::cred_identity::{BUDGET_ENV, READ_TIMEOUT_ENV};
    use crate::debug_knob::secs_or;

    #[test]
    fn the_knob_names_are_test_only_and_outside_the_rotate_prefix() {
        // daemon.rs removes every AMPLIHACK_ROTATE* variable from the daemon's
        // environment, so a knob with that prefix would never reach it.
        assert_eq!(BUDGET_ENV, "AMPLIHACK_TEST_IDENTITY_BUDGET_SECS");
        assert_eq!(
            READ_TIMEOUT_ENV,
            "AMPLIHACK_TEST_IDENTITY_READ_TIMEOUT_SECS"
        );
    }

    #[test]
    fn an_unset_knob_gives_the_default() {
        assert_eq!(secs_or(None, DEFAULT), DEFAULT);
    }

    #[test]
    fn whole_seconds_from_one_to_an_hour_are_accepted() {
        for (raw, secs) in [("1", 1), ("5", 5), ("600", 600), ("3600", 3600), ("007", 7)] {
            assert_eq!(
                secs_or(Some(raw), DEFAULT),
                Duration::from_secs(secs),
                "{raw:?} must give {secs} s"
            );
        }
    }

    #[test]
    fn any_other_value_gives_the_default() {
        let over_u64 = (u128::from(u64::MAX) + 1).to_string();
        // Built at run time: a 32-character literal trips the secret scan
        // (security_hygiene tc_sec_20).
        let thirty_two_nines = "9".repeat(32);
        let cases: Vec<&str> = vec![
            "",
            "0",
            "00",
            "3601",
            "86400",
            &over_u64,
            &thirty_two_nines,
            "+5",
            "-5",
            " 5",
            "5 ",
            "5\n",
            "5s",
            "1.5",
            "1e3",
            "0x10",
            "five",
            // Unicode digits are not ASCII digits.
            "\u{0665}",         // ARABIC-INDIC DIGIT FIVE
            "\u{FF15}",         // FULLWIDTH DIGIT FIVE
            "\u{0035}\u{0301}", // '5' with a combining accent
        ];
        for raw in cases {
            assert_eq!(
                secs_or(Some(raw), DEFAULT),
                DEFAULT,
                "{raw:?} must fall back to the default"
            );
        }
    }

    #[test]
    fn the_default_passed_in_is_the_one_returned() {
        let other = Duration::from_secs(5);
        assert_eq!(secs_or(None, other), other);
        assert_eq!(secs_or(Some("junk"), other), other);
        assert_eq!(secs_or(Some("9"), other), Duration::from_secs(9));
    }

    /// The functions agree with the parser on whatever this process holds;
    /// in a normal test run neither variable is set, so they give the
    /// constants.
    #[test]
    fn the_functions_read_their_own_knob_and_default_to_the_constants() {
        let budget = std::env::var(BUDGET_ENV).ok();
        let read = std::env::var(READ_TIMEOUT_ENV).ok();
        assert_eq!(
            identity_budget(),
            secs_or(budget.as_deref(), IDENTITY_BUDGET)
        );
        assert_eq!(
            identity_read_timeout(),
            secs_or(read.as_deref(), IDENTITY_READ_TIMEOUT)
        );
        if budget.is_none() {
            assert_eq!(identity_budget(), IDENTITY_BUDGET);
        }
        if read.is_none() {
            assert_eq!(identity_read_timeout(), IDENTITY_READ_TIMEOUT);
        }
    }
}

/// Release builds ignore the environment entirely.
#[cfg(not(debug_assertions))]
#[test]
fn release_builds_always_use_the_constants() {
    assert_eq!(identity_budget(), IDENTITY_BUDGET);
    assert_eq!(identity_read_timeout(), IDENTITY_READ_TIMEOUT);
}
