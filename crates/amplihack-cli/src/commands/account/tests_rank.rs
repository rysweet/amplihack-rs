//! Issue #2006: the ranking rule of `amplihack account pick`.
//!
//! Wired from `account/mod.rs` as `#[cfg(test)] mod tests_rank;`.
//!
//! Contract (docs/reference/account-pick.md, "Ranking"):
//!
//! - `rank::effective(live, jobs, leases, cost) -> f64` is
//!   `live - (jobs + leases) * cost`.
//! - `rank::Scored { home, live: Option<f64>, usable, jobs, leases,
//!   effective: Option<f64> }`, built with
//!   `Scored::new(home, live, usable, jobs, leases, cost)`; `effective` is
//!   `None` exactly when `live` is `None`.
//! - `rank::order(&[Scored]) -> Vec<usize>`: usable candidates first, by
//!   (1) highest effective, (2) highest live, (3) fewest jobs plus leases,
//!   (4) greatest home path as bytes; then unusable candidates in ascending
//!   path order.
//! - `rank::choose(&[Scored], order: &[usize]) -> Option<usize>`: the
//!   first of `order` when it is usable and its effective headroom is above
//!   0, else `None`.
//!
//! Pure functions: no files, no processes, no environment.

use super::rank::{self, Scored};
use std::path::PathBuf;

const COST: f64 = 15.0;

fn s(home: &str, live: Option<f64>, usable: bool, jobs: usize, leases: usize) -> Scored {
    Scored::new(PathBuf::from(home), live, usable, jobs, leases, COST)
}

fn homes(rows: &[Scored], order: &[usize]) -> Vec<String> {
    order
        .iter()
        .map(|&i| rows[i].home.display().to_string())
        .collect()
}

#[test]
fn effective_subtracts_cost_per_job_and_lease() {
    assert_eq!(rank::effective(80.0, 0, 0, 15.0), 80.0);
    assert_eq!(rank::effective(80.0, 1, 0, 15.0), 65.0);
    assert_eq!(rank::effective(80.0, 0, 1, 15.0), 65.0);
    assert_eq!(rank::effective(80.0, 2, 3, 15.0), 5.0);
    assert_eq!(rank::effective(20.0, 1, 1, 15.0), -10.0);
    assert_eq!(rank::effective(50.0, 1, 0, 2.5), 47.5);
}

#[test]
fn scored_new_computes_effective_and_keeps_none_for_no_reading() {
    let a = s("/a", Some(80.0), true, 1, 1);
    assert_eq!(a.effective, Some(50.0));
    let b = s("/b", None, false, 2, 0);
    assert_eq!(b.effective, None, "no reading gives no effective headroom");
}

#[test]
fn highest_effective_wins() {
    let rows = vec![
        s("/z", Some(50.0), true, 0, 0),
        s("/a", Some(80.0), true, 0, 0),
        s("/m", Some(20.0), true, 0, 0),
    ];
    assert_eq!(rank::choose(&rows, &rank::order(&rows)), Some(1));
    assert_eq!(homes(&rows, &rank::order(&rows)), ["/a", "/z", "/m"]);
}

#[test]
fn jobs_move_the_choice_to_the_next_account() {
    // 80 with three jobs is 35; 50 with none wins.
    let rows = vec![
        s("/a", Some(80.0), true, 3, 0),
        s("/b", Some(50.0), true, 0, 0),
    ];
    assert_eq!(rank::choose(&rows, &rank::order(&rows)), Some(1));
}

#[test]
fn equal_effective_is_broken_by_higher_live() {
    // Both at effective 50: /a has live 80 (two leases), /z live 50.
    let rows = vec![
        s("/z", Some(50.0), true, 0, 0),
        s("/a", Some(80.0), true, 0, 2),
    ];
    assert_eq!(
        rank::choose(&rows, &rank::order(&rows)),
        Some(1),
        "live headroom breaks the tie"
    );
}

#[test]
fn equal_effective_and_live_is_broken_by_fewer_jobs_plus_leases() {
    // Same live and effective needs a zero cost; build by hand.
    let rows = vec![
        Scored::new(PathBuf::from("/z"), Some(60.0), true, 2, 1, 0.0),
        Scored::new(PathBuf::from("/a"), Some(60.0), true, 1, 0, 0.0),
    ];
    assert_eq!(
        rank::choose(&rows, &rank::order(&rows)),
        Some(1),
        "fewer jobs plus leases breaks the tie before the path"
    );
}

#[test]
fn a_full_tie_is_broken_by_the_greatest_path_as_bytes() {
    let rows = vec![
        s("/p/a", Some(40.0), true, 0, 0),
        s("/p/c", Some(40.0), true, 0, 0),
        s("/p/b", Some(40.0), true, 0, 0),
    ];
    assert_eq!(rank::choose(&rows, &rank::order(&rows)), Some(1));
    assert_eq!(homes(&rows, &rank::order(&rows)), ["/p/c", "/p/b", "/p/a"]);
}

#[test]
fn the_choice_does_not_depend_on_input_order() {
    let base = vec![
        s("/a", Some(80.0), true, 0, 4), // 20
        s("/b", Some(50.0), true, 0, 2), // 20
        s("/c", Some(20.0), true, 0, 0), // 20
    ];
    let mut rev: Vec<Scored> = base.clone();
    rev.reverse();
    let pick = |rows: &[Scored]| {
        rows[rank::choose(rows, &rank::order(rows)).unwrap()]
            .home
            .clone()
    };
    assert_eq!(pick(&base), PathBuf::from("/a"));
    assert_eq!(pick(&rev), PathBuf::from("/a"));
}

#[test]
fn effective_must_be_above_zero() {
    let rows = vec![s("/a", Some(30.0), true, 2, 0)]; // exactly 0
    assert_eq!(
        rank::choose(&rows, &rank::order(&rows)),
        None,
        "effective 0 does not qualify"
    );
    let rows = vec![s("/a", Some(30.0), true, 1, 1)]; // 0 as well
    assert_eq!(rank::choose(&rows, &rank::order(&rows)), None);
    let rows = vec![s("/a", Some(10.0), true, 1, 0)]; // -5
    assert_eq!(rank::choose(&rows, &rank::order(&rows)), None);
    let rows = vec![s("/a", Some(0.5), true, 0, 0)]; // credits-only room
    assert_eq!(rank::choose(&rows, &rank::order(&rows)), Some(0));
}

#[test]
fn an_unusable_candidate_is_never_chosen_even_with_more_headroom() {
    let rows = vec![
        s("/a", Some(90.0), false, 0, 0),
        s("/b", Some(10.0), true, 0, 0),
    ];
    assert_eq!(rank::choose(&rows, &rank::order(&rows)), Some(1));
}

#[test]
fn unusable_candidates_come_last_in_ascending_path_order() {
    let rows = vec![
        s("/c", None, false, 0, 0),
        s("/x", Some(20.0), true, 0, 0),
        s("/a", None, false, 0, 0),
        s("/b", Some(99.0), false, 0, 0),
        s("/y", Some(40.0), true, 0, 0),
    ];
    assert_eq!(
        homes(&rows, &rank::order(&rows)),
        ["/y", "/x", "/a", "/b", "/c"]
    );
}

#[test]
fn usable_candidates_at_or_below_zero_rank_before_unusable_ones() {
    let rows = vec![
        s("/u", None, false, 0, 0),
        s("/n", Some(20.0), true, 2, 0), // -10
        s("/p", Some(30.0), true, 1, 0), // 15
    ];
    assert_eq!(homes(&rows, &rank::order(&rows)), ["/p", "/n", "/u"]);
}

#[test]
fn nothing_to_choose_from_is_none() {
    assert_eq!(rank::choose(&[], &[]), None);
    assert!(rank::order(&[]).is_empty());
    let rows = vec![s("/a", None, false, 0, 0), s("/b", None, false, 0, 0)];
    assert_eq!(rank::choose(&rows, &rank::order(&rows)), None);
}

/// The worked example in the reference, replayed with the pure functions:
/// each pick adds a lease to the chosen account and the next pick sees it.
#[test]
fn ten_serial_picks_over_80_50_20_split_6_3_1() {
    // Names chosen so the path rule alone (greatest path wins) would favour
    // the 20% account: only the four-key order gives 6/3/1.
    let live = [("/acc/a", 80.0), ("/acc/b", 50.0), ("/acc/c", 20.0)];
    let mut leases = [0usize; 3];
    let mut chosen = Vec::new();
    for _ in 0..10 {
        let rows: Vec<Scored> = live
            .iter()
            .zip(leases.iter())
            .map(|((h, l), n)| s(h, Some(*l), true, 0, *n))
            .collect();
        let i = rank::choose(&rows, &rank::order(&rows))
            .expect("a candidate qualifies at each of the ten picks");
        leases[i] += 1;
        chosen.push(i);
    }
    assert_eq!(leases, [6, 3, 1], "picks in order: {chosen:?}");
    assert_eq!(chosen, [0, 0, 0, 1, 0, 1, 0, 1, 2, 0]);
}
