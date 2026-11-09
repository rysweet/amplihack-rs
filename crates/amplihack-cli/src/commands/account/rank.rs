//! The ranking rule of `amplihack account pick` (issue #2006). Pure
//! functions: no files, no processes, no environment.
//!
//! - Effective headroom is `live - (jobs + leases) * cost`.
//! - Usable candidates come first, by (1) highest effective headroom,
//!   (2) highest live headroom, (3) fewest jobs plus leases, (4) greatest
//!   home path as bytes. Unusable candidates follow in ascending path order.
//! - The choice is the first candidate when it is usable and its effective
//!   headroom is above 0.

use std::cmp::Ordering;
use std::path::PathBuf;

/// `live - (jobs + leases) * cost`.
pub fn effective(live: f64, jobs: usize, leases: usize, cost: f64) -> f64 {
    live - (jobs + leases) as f64 * cost
}

/// One candidate with what the rule needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Scored {
    pub home: PathBuf,
    /// `None` when the account gave no reading.
    pub live: Option<f64>,
    pub usable: bool,
    pub jobs: usize,
    pub leases: usize,
    /// `None` exactly when `live` is `None`.
    pub effective: Option<f64>,
}

impl Scored {
    pub fn new(
        home: PathBuf,
        live: Option<f64>,
        usable: bool,
        jobs: usize,
        leases: usize,
        cost: f64,
    ) -> Self {
        Self {
            effective: live.map(|l| effective(l, jobs, leases, cost)),
            home,
            live,
            usable,
            jobs,
            leases,
        }
    }

    fn path_bytes(&self) -> &[u8] {
        self.home.as_os_str().as_encoded_bytes()
    }
}

/// `a` before `b` among usable candidates.
fn usable_order(a: &Scored, b: &Scored) -> Ordering {
    let neg = f64::NEG_INFINITY;
    b.effective
        .unwrap_or(neg)
        .total_cmp(&a.effective.unwrap_or(neg))
        .then_with(|| b.live.unwrap_or(neg).total_cmp(&a.live.unwrap_or(neg)))
        .then_with(|| (a.jobs + a.leases).cmp(&(b.jobs + b.leases)))
        .then_with(|| b.path_bytes().cmp(a.path_bytes()))
}

/// The indices of `rows` in ranked order.
pub fn order(rows: &[Scored]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..rows.len()).collect();
    idx.sort_by(|&i, &j| {
        let (a, b) = (&rows[i], &rows[j]);
        match (a.usable, b.usable) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (true, true) => usable_order(a, b),
            (false, false) => a.path_bytes().cmp(b.path_bytes()),
        }
    });
    idx
}

/// Whether a ranked row may be chosen: usable, with effective headroom
/// above 0.
pub fn qualifies(row: &Scored) -> bool {
    row.usable && row.effective.is_some_and(|e| e > 0.0)
}

/// The index of the chosen candidate, if any qualifies.
pub fn choose(rows: &[Scored]) -> Option<usize> {
    order(rows)
        .first()
        .copied()
        .filter(|&i| qualifies(&rows[i]))
}
