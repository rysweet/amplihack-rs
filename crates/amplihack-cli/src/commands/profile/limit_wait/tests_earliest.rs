//! `earliest` (issue #2967): which other login's reset sets an
//! earliest-reset wait. Pure: it is given the instants and picks the
//! minimum, a tie going to the lexicographically first profile name, so the
//! same inputs always name the same login.
//!
//! Contract: `earliest(others: &[(ProfileName, i64)]) -> Option<(ProfileName, i64)>`.
//! The caller has already left out every login that is under the threshold, tried,
//! barred or without a reset, and every instant at or before now; this
//! function filters nothing and orders the rest.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_earliest;`.

use super::earliest::earliest;
use crate::commands::profile::name::ProfileName;

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

#[test]
fn no_login_gives_none() {
    assert_eq!(earliest(&[]), None);
}

#[test]
fn one_login_is_the_earliest() {
    assert_eq!(earliest(&[(n("lab"), 500)]), Some((n("lab"), 500)));
}

#[test]
fn the_smallest_instant_wins_whatever_the_order() {
    let a = (n("work"), 900);
    let b = (n("lab"), 300);
    let c = (n("spare"), 600);
    for order in [
        [a.clone(), b.clone(), c.clone()],
        [c.clone(), b.clone(), a.clone()],
        [b.clone(), a.clone(), c.clone()],
    ] {
        assert_eq!(earliest(&order), Some(b.clone()));
    }
}

#[test]
fn a_tie_goes_to_the_first_name() {
    let tied = [(n("spare"), 700), (n("lab"), 700), (n("work"), 700)];
    assert_eq!(earliest(&tied), Some((n("lab"), 700)));
    let reversed: Vec<_> = tied.iter().rev().cloned().collect();
    assert_eq!(earliest(&reversed), Some((n("lab"), 700)));
}

#[test]
fn an_earlier_instant_beats_an_earlier_name() {
    assert_eq!(
        earliest(&[(n("a"), 701), (n("z"), 700)]),
        Some((n("z"), 700))
    );
}

#[test]
fn extreme_instants_order_without_overflow() {
    assert_eq!(
        earliest(&[(n("a"), i64::MAX), (n("b"), i64::MIN), (n("c"), 0)]),
        Some((n("b"), i64::MIN))
    );
    assert_eq!(
        earliest(&[(n("a"), i64::MAX), (n("b"), i64::MAX)]),
        Some((n("a"), i64::MAX))
    );
}
