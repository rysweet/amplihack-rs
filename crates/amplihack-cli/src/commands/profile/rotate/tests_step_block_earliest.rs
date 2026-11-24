//! Issue #2967: `step_block::earliest_end(root, logins, now)` is the one
//! reader a step's wait has of another login's reset. It reads the rotate
//! ledger's standing blocks by the rule every block reader uses
//! (`active_blocks`), only for the logins it is given, and nothing else: no
//! usage reading, no threshold, no age rule.
//!
//! Wired from `tests_step_block.rs` with `#[path = ...] mod earliest;`, as
//! `kinds` is, because `rotate/mod.rs` is at its line budget.

use super::super::step_block::earliest_end;
use super::{NOW, Root, root, spend};
use std::fs;

/// A root whose ledger records a block on each login, from a minute ago
/// until the given instant.
fn with_blocks(blocks: &[(&str, i64)]) -> Root {
    let r = root();
    let mut l = r.rs.load_ledger().unwrap().expect("a ledger");
    for (name, until) in blocks {
        l.overflow_blocked
            .insert((*name).to_string(), spend(NOW - 60, *until));
    }
    r.rs.save_ledger(&l).unwrap();
    r
}

#[test]
fn the_earliest_standing_block_among_the_given_logins_is_returned() {
    let r = with_blocks(&[("a", NOW + 900), ("b", NOW + 300)]);
    assert_eq!(
        earliest_end(&r.path, &["a", "b"], NOW),
        Some(("b", NOW + 300))
    );
}

#[test]
fn a_login_not_given_is_never_read() {
    // `c` ends first, but the caller did not give it: the step may not
    // move there.
    let r = with_blocks(&[("a", NOW + 900), ("c", NOW + 100)]);
    assert_eq!(earliest_end(&r.path, &["a"], NOW), Some(("a", NOW + 900)));
    assert_eq!(earliest_end(&r.path, &[], NOW), None);
}

#[test]
fn a_block_that_has_ended_holds_no_login_out() {
    let r = with_blocks(&[("a", NOW), ("b", NOW - 1)]);
    assert_eq!(earliest_end(&r.path, &["a", "b"], NOW), None);
}

#[test]
fn a_tie_goes_to_the_first_name() {
    let r = with_blocks(&[("b", NOW + 300), ("a", NOW + 300)]);
    assert_eq!(
        earliest_end(&r.path, &["b", "a"], NOW),
        Some(("a", NOW + 300))
    );
}

#[test]
fn no_ledger_and_an_unreadable_ledger_give_none() {
    // A host the rotator has never run on records no block at all.
    let bare = tempfile::tempdir().unwrap();
    assert_eq!(earliest_end(bare.path(), &["a"], NOW), None);
    // An unreadable ledger holds no login out of anything, here as in the
    // candidate rule; the reader says so on stderr.
    let r = with_blocks(&[("a", NOW + 300)]);
    fs::write(&r.rs.ledger, "{not json").unwrap();
    assert_eq!(earliest_end(&r.path, &["a"], NOW), None);
}
