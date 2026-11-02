//! A manual `/login` onto an overflow account (issue #1606 against #1693's
//! hand-change recovery). Recovery records the holder as live before the
//! cycle measures, so when the holder is a credit-backed overflow profile
//! the same cycle must still apply the overflow policy: with a plan login
//! that has capacity it moves off the overflow profile, and the overflow
//! profile is never recorded as having lost its login. This is where
//! credits would otherwise be spent by accident: to plain `decide` an
//! overflow profile with headroom looks like a good place to stay.
//!
//! The fixture is `tests_handfix::fx`: the ledger names L (no login), the
//! manual login in `~/.claude` is M's account and M has its own copy, C is
//! a plan login. Here M is marked overflow. The stubs replace only probing;
//! the decision is the real one ([`decide_measured`]).

use super::cycle::{Ctx, CycleOutcome, Mode, run_cycle};
use super::decide::{Probed, Reading, Windows};
use super::identify::LiveId;
use super::observe::{Measurement, Snap, decide_measured};
use super::tests_handfix::{C, L, M, fx, lost_l};
use super::tests_handfix_cycle::ctx;
use super::tests_swap::{ino_at, n};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::config::{self, ReadMode};
use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::overflow::OverflowEntry;
use std::path::Path;

fn mark_overflow(root: &Path, name: &str) {
    config::update(root, ReadMode::Editable, |c| {
        c.overflow.push(OverflowEntry::new(n(name), 1));
        Ok(())
    })
    .unwrap();
}

/// A probe with plenty of headroom: plain `decide` would stay on it.
fn roomy() -> Probed {
    Probed {
        reading: Reading::Measured {
            session: 10.0,
            week: 10.0,
        },
        windows: Windows::default(),
        spend_limit: false,
        unknown: false,
        still_running: false,
    }
}

/// A plan login with capacity.
fn plan_ok() -> Probed {
    Probed {
        reading: Reading::Measured {
            session: 12.0,
            week: 40.0,
        },
        windows: Windows::default(),
        spend_limit: false,
        unknown: false,
        still_running: false,
    }
}

/// Cycle 1: recovery has just recorded overflow profile M as live. M has
/// headroom and C, a plan login, has capacity.
fn m_live_on_overflow(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    assert_eq!(snap.live, LiveId::Known(n(M)), "snapshot after recovery");
    assert_eq!(snap.candidates, vec![n(C)], "L has no login, M is live");
    assert!(
        snap.overflow.iter().any(|e| e.name == n(M)),
        "the snapshot knows M is overflow"
    );
    decide_measured(ctx, snap, roomy(), vec![(n(C), plan_ok())], now())
}

/// Cycle 2: C is live with capacity; M, the overflow profile, is a
/// candidate with headroom.
fn c_live_with_capacity(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    assert_eq!(snap.live, LiveId::Known(n(C)));
    assert_eq!(snap.candidates, vec![n(M)], "M keeps the manual login");
    decide_measured(ctx, snap, plan_ok(), vec![(n(M), roomy())], now())
}

#[cfg(target_os = "linux")]
#[test]
fn a_manual_login_onto_the_overflow_account_is_moved_off_while_a_plan_login_has_capacity() {
    let f = fx(true, true);
    mark_overflow(&f.root, M);
    let mut c = ctx(&f, m_live_on_overflow);
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Swapped,
        "{:?}",
        f.logged()
    );
    assert_eq!(ino_at(&f.live()), f.c, "C, the plan login, is live");
    assert_eq!(
        ino_at(&f.slot(M)),
        Some(f.h),
        "the manual login went back to overflow profile M"
    );
    f.assert_each_login_once();
    let l = f.ledger();
    assert_eq!(l.live.as_deref(), Some(C));
    let last = l.last_cycle.clone().unwrap();
    assert_eq!(last.outcome, "swapped");
    assert_eq!(last.overflow.as_deref(), Some("out"), "moved off credits");
    assert_eq!(l.guidance, None);
    assert!(
        !l.lost_login.iter().any(|x| x == M),
        "the overflow profile still has its login: {:?}",
        l.lost_login
    );
    assert_eq!(l.lost_login, vec![L.to_string()], "only L lost its login");
    let log = f.logged();
    assert!(log.contains(&lost_l()), "{log:?}");
    assert!(
        log.iter().any(|x| x.starts_with(&format!("swap {M}->{C}"))),
        "{log:?}"
    );

    // The next cycle does not move back onto the overflow profile while the
    // plan login has capacity, and still records no lost login for M.
    c.measure = Some(c_live_with_capacity);
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Stayed,
        "{:?}",
        f.logged()
    );
    assert_eq!(ino_at(&f.live()), f.c);
    let l = f.ledger();
    assert_eq!(l.live.as_deref(), Some(C));
    assert_eq!(l.last_cycle.unwrap().overflow, None, "not on credits");
    assert!(!l.lost_login.iter().any(|x| x == M), "{:?}", l.lost_login);
    f.assert_each_login_once();
}
