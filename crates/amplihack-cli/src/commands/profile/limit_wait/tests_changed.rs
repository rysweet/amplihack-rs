//! Issue #2731: [`changed`], the comparison that decides whether the login
//! moved while the attempt was failing.
//!
//! It is read once, between the wait's `start` line and the wait itself, and
//! a `true` costs the step its one retry: the step runs again at once rather
//! than waiting, on whatever login is in `~/.claude` now. So the cost of the
//! two answers is not symmetric. A missed change costs the wait the step was
//! going to make anyway, and the rotator's next swap will be seen. A false
//! change spends the retry on a login that is still at its limit and ends
//! the step. The comparison therefore fails closed, and the three ways it
//! can be asked a question it cannot answer all read as no change:
//!
//! - either reading failed, so there is no name to compare;
//! - the two readings came from different files, so they are answers to
//!   different questions ([`Source`]);
//! - the names agree.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_changed;`.

use super::changed;
use super::fingerprint::Source;

fn cycle(name: &str) -> Option<(Source, String)> {
    Some((Source::Cycle, name.to_string()))
}

fn swap(name: &str) -> Option<(Source, String)> {
    Some((Source::Swap, name.to_string()))
}

// --- the change the wait exists for --------------------------------------

#[test]
fn two_ledger_readings_with_different_names_are_a_change() {
    assert!(changed(&cycle("a"), cycle("b")));
}

#[test]
fn two_swap_record_readings_with_different_names_are_a_change() {
    // A host whose rotate ledger is absent still gets the swap seen: both
    // readings come from the swap record, so they are comparable.
    assert!(changed(&swap("a"), swap("b")));
}

// --- no change -----------------------------------------------------------

#[test]
fn the_same_name_from_the_same_file_is_no_change() {
    assert!(!changed(&cycle("a"), cycle("a")));
    assert!(!changed(&swap("a"), swap("a")));
}

#[test]
fn an_unreadable_first_reading_is_no_change() {
    assert!(
        !changed(&None, cycle("b")),
        "nothing to compare with: the step waits, as it would have"
    );
}

#[test]
fn an_unreadable_second_reading_is_no_change() {
    assert!(!changed(&cycle("a"), None));
}

#[test]
fn two_unreadable_readings_are_no_change() {
    assert!(!changed(&None, None));
}

// --- the readings must be answers to the same question -------------------

/// The deputy's point 3. `Watch::login` prefers the rotate ledger and falls
/// back to the swap record, so a ledger that is missing, stale or refused
/// between the two readings silently moves the answer to the other file.
/// The swap record names the login the *last* swap put in place, which on a
/// host that has swapped before is a different name with no swap between the
/// two readings at all.
#[test]
fn a_ledger_reading_and_a_swap_record_reading_are_never_compared() {
    assert!(
        !changed(&cycle("a"), swap("b")),
        "the ledger stopped being readable; that is not a swap"
    );
    assert!(
        !changed(&swap("a"), cycle("b")),
        "and neither is the ledger becoming readable again"
    );
}

#[test]
fn the_same_name_from_different_files_is_also_no_change() {
    // There is nothing to report either way, and the rule is about the
    // sources rather than about the names agreeing by luck.
    assert!(!changed(&cycle("a"), swap("a")));
}

/// A stale swap record is the ordinary state of a host that has swapped
/// once and then had its ledger rewritten: the record keeps the old swap
/// until the next one. It must never, on its own, send a step back to work.
#[test]
fn a_stale_swap_record_cannot_report_a_change_against_a_ledger_reading() {
    let before = cycle("live-now");
    let after = swap("live-last-week");
    assert!(!changed(&before, after));
}
