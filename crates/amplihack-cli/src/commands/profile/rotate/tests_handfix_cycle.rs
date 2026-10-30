//! Hand-change recovery inside the real `run_cycle` (issue #1693, review of
//! #1702), on the 27 Oct fixture from `tests_handfix.rs`. `Ctx::measure`
//! replaces finding `claude` and probing, so nothing outside the temp dir
//! runs; `Ctx::scan` sees no sessions. Each stub checks the snapshot it is
//! handed, so these tests show what the cycle itself read after recovery.

use super::cycle::{Ctx, CycleOutcome, Mode, run_cycle};
use super::decide::{Decision, Measured, Reading, decide};
use super::identify::{LiveId, live};
use super::log::msg;
use super::observe::{Measurement, Snap};
use super::pin::{self, PinState};
use super::settings;
use super::switch::{Switched, release, switch_live};
use super::tests_handfix::{C, CREDS, Fx, L, M, fx, lost_l, moved_line};
use super::tests_swap::{ino_at, n, private};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{Liveness, now};
use std::collections::BTreeSet;

pub(super) fn ctx(f: &Fx, measure: super::cycle::MeasureFn) -> Ctx {
    let s = settings::resolve(&super::TuneFlags::default(), None, &|_| None).unwrap();
    let mut ctx = Ctx::new(f.store.clone(), f.home.clone(), s);
    ctx.scan = |_| Liveness::Known(BTreeSet::new());
    ctx.measure = Some(measure);
    ctx
}

/// M is spent and every candidate has capacity; decided by the real
/// `decide`. The snapshot must already name M, read from the ledger that
/// recovery wrote earlier in this same cycle.
pub(super) fn m_spent(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    assert_eq!(snap.live, LiveId::Known(n(M)), "snapshot after recovery");
    assert_eq!(snap.candidates, vec![n(C)], "L has no login, M is live");
    let reading = Reading::Measured {
        session: 12.0,
        week: 40.0,
    };
    let measured: Vec<Measured> = snap
        .candidates
        .iter()
        .map(|c| Measured {
            name: c.clone(),
            reading,
            windows: Default::default(),
            spend_limit: false,
        })
        .collect();
    let decision = decide(&ctx.settings, &Reading::Exhausted, &measured, None, now());
    Ok(Measurement {
        live: Reading::Exhausted,
        others: measured.into_iter().map(|m| (m.name, m.reading)).collect(),
        decision,
        note: None,
        failed: 0,
        tag: None,
        new_blocks: Vec::new(),
        next_plan_reset: None,
        measured_at: now(),
        probed: None,
    })
}

pub(super) fn never(_: &Ctx, _: &Snap) -> Result<Measurement, ProfileError> {
    panic!("this cycle must end before measuring")
}

fn stay(_: &Ctx, _: &Snap) -> Result<Measurement, ProfileError> {
    Ok(Measurement {
        live: Reading::Measured {
            session: 10.0,
            week: 10.0,
        },
        others: Vec::new(),
        decision: Decision::Stay,
        note: None,
        failed: 0,
        tag: None,
        new_blocks: Vec::new(),
        next_plan_reset: None,
        measured_at: now(),
        probed: None,
    })
}

/// The one backup directory recovery created (its name has the cycle's
/// real time, so it is found rather than built).
fn only_backup(f: &Fx) -> std::path::PathBuf {
    let dirs: Vec<_> = std::fs::read_dir(f.rejected())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    dirs[0].join(CREDS)
}

fn no_recovery_or_guidance(lines: &[String]) {
    for l in lines {
        assert!(
            !l.starts_with("recovered:") && !l.starts_with("ALERT:") && !l.starts_with("fix "),
            "unexpected line: {l}\n{lines:?}"
        );
    }
}

/// The #1693 reproduction through `run_cycle`: recovery moves M's duplicate
/// aside after swap recovery, the snapshot reads the new ledger, and the same
/// cycle swaps to C, which has capacity.
#[cfg(target_os = "linux")]
#[test]
fn one_rotate_cycle_recovers_the_hand_change_and_swaps_to_capacity() {
    let f = fx(true, true);
    let mut c = ctx(&f, m_spent);
    assert_eq!(
        run_cycle(&mut c, Mode::OneShot).unwrap(),
        CycleOutcome::Swapped
    );
    let backup = only_backup(&f);
    assert_eq!(ino_at(&backup), f.m, "M's own copy, renamed aside");
    assert_eq!(ino_at(&f.live()), f.c, "C is live");
    assert_eq!(ino_at(&f.slot(M)), Some(f.h), "the manual login went to M");
    assert_eq!(
        ino_at(&f.slot(L)),
        None,
        "L had no login and still has none"
    );
    f.assert_each_login_once();
    let l = f.ledger();
    assert_eq!(l.live.as_deref(), Some(C));
    assert_eq!(l.account.as_deref(), Some("u-src"));
    assert_eq!(l.last_cycle.unwrap().outcome, "swapped");
    assert_eq!(l.guidance, None);
    let log = f.logged();
    assert_eq!(log[0], moved_line(&backup), "{log:?}");
    assert_eq!(log[1], lost_l(), "{log:?}");
    assert!(
        log.iter().any(|x| x.starts_with(&format!("swap {M}->{C}"))),
        "{log:?}"
    );
}

/// #1702: an unreadable `~/.claude.json` ends the cycle with the retry
/// ALERT, moves nothing and advises no `/login`; once Claude has written the
/// file again, the next cycle recovers and swaps.
#[cfg(target_os = "linux")]
#[test]
fn an_unreadable_home_account_retries_and_the_next_cycle_recovers() {
    let f = fx(true, true);
    let good = std::fs::read(f.home.join(".claude.json")).unwrap();
    private(&f.home.join(".claude.json"), "{");
    let mut c = ctx(&f, never);
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Alerted
    );
    f.assert_nothing_moved();
    assert_eq!(f.ledger().guidance, None);
    let alerts: Vec<String> = f
        .logged()
        .into_iter()
        .filter(|l| !l.starts_with("skip: "))
        .collect();
    assert_eq!(alerts, vec![msg::home_unreadable(&n(L))], "no fix lines");

    private(
        &f.home.join(".claude.json"),
        std::str::from_utf8(&good).unwrap(),
    );
    c.measure = Some(m_spent);
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Swapped
    );
    assert_eq!(ino_at(&f.live()), f.c);
    f.assert_each_login_once();
}

/// #1701 then #1693: `profile switch` moves the live login through the
/// rotator's swap and writes the ledger, so the rotate cycles after it see
/// no hand change: no recovery, no guidance, nothing moved, held and then,
/// once released, a normal stay.
#[cfg(target_os = "linux")]
#[test]
fn a_profile_switch_is_never_taken_for_a_hand_change() {
    let f = fx(false, true);
    // A correct ledger: ~/.claude holds M's login.
    let mut l = f.ledger();
    l.live = Some(M.into());
    l.account = Some("u-mistt".into());
    f.rs.save_ledger(&l).unwrap();

    assert_eq!(
        switch_live(&f.store, &f.home, &n(C)).unwrap().unwrap(),
        Switched::Moved {
            from: n(M),
            to: n(C)
        }
    );
    assert_eq!(pin::read(&f.rs), PinState::Pinned(n(C)));
    assert_eq!(
        live(&f.store, &f.rs, &f.home).unwrap().0,
        LiveId::Known(n(C))
    );
    let mut c = ctx(&f, never);
    assert_eq!(run_cycle(&mut c, Mode::Watch).unwrap(), CycleOutcome::Held);
    release(&f.store).unwrap();
    c.measure = Some(stay);
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Stayed
    );

    no_recovery_or_guidance(&f.logged());
    let l = f.ledger();
    assert_eq!(l.live.as_deref(), Some(C));
    assert_eq!(l.guidance, None);
    assert!(l.lost_login.is_empty(), "{:?}", l.lost_login);
    assert_eq!(ino_at(&f.live()), f.c);
    assert_eq!(ino_at(&f.slot(M)), Some(f.h));
    assert!(!f.rejected().exists(), "no backup was made");
    f.assert_each_login_once();
}
