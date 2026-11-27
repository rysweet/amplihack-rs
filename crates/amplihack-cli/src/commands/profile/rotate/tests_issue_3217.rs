//! Post-swap failure scenarios through the real watch cycle, with an injected
//! clock, a stand-in usage source and a temporary profile store.

use super::cycle::{Ctx, CycleOutcome, Mode, run_cycle};
use super::decide::Probed;
use super::decide::target_seen::Extra;
use super::decide::target_seen::tests_issue_2910_fixture::{
    Fleet, clock, expired, fleet, name, reporter, set_clock,
};
use super::measure::ProbeLabel;
use super::observe::{Measurement, Snap, decide_with};
use super::tests_issue_1870::{exh, rd};
use super::tests_revalidate_cause::ctx_with;
use crate::commands::profile::ProfileError;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::usage_cmd::ENV_CREDENTIAL_VARS;
use crate::test_support::{EnvGuard, env_lock};
use std::cell::RefCell;
use std::time::Duration;

struct CycleProbes {
    live: Probed,
    target: Option<(&'static str, Probed)>,
    label: Option<ProbeLabel>,
    confirmation: Probed,
    confirmations: Vec<ProfileName>,
}

thread_local! {
    static PROBES: RefCell<Option<CycleProbes>> = const { RefCell::new(None) };
}

fn measure(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    PROBES.with(|script| {
        let script = script.borrow();
        let script = script.as_ref().unwrap();
        let probes = snap
            .candidates
            .iter()
            .map(|candidate| {
                let probe = script
                    .target
                    .as_ref()
                    .filter(|(who, _)| *who == candidate.as_str())
                    .map_or_else(|| exh(100.0, 100.0), |(_, p)| p.clone());
                (candidate.clone(), probe)
            })
            .collect();
        let labels = snap.measured_live().zip(script.label);
        let extra = Extra::for_cycle(
            ctx.store.root(),
            labels.into_iter().map(|(n, l)| (n.clone(), l)).collect(),
            ctx.stored.times(clock()),
            &snap.decision_overflow(),
            snap.measured_live(),
            clock(),
        );
        decide_with(ctx, snap, script.live.clone(), probes, clock(), &extra)
    })
}

fn confirm(_: &Ctx, who: &ProfileName, _: Duration) -> (Probed, Option<ProbeLabel>) {
    PROBES.with(|script| {
        let mut script = script.borrow_mut();
        let script = script.as_mut().unwrap();
        script.confirmations.push(who.clone());
        (script.confirmation.clone(), None)
    })
}

fn cycle(
    ctx: &mut Ctx,
    at: i64,
    live: Probed,
    target: Option<(&'static str, Probed)>,
    label: Option<ProbeLabel>,
    confirmation: Probed,
) -> (CycleOutcome, Vec<ProfileName>) {
    set_clock(at);
    PROBES.with(|script| {
        *script.borrow_mut() = Some(CycleProbes {
            live,
            target,
            label,
            confirmation,
            confirmations: Vec::new(),
        });
    });
    let outcome = run_cycle(ctx, Mode::Watch).unwrap();
    let calls = PROBES.with(|script| script.borrow_mut().take().unwrap().confirmations);
    (outcome, calls)
}

fn after_swap(run: impl FnOnce(&Fleet, &mut Ctx, i64)) {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let f = fleet(&["target", "reserve"], &[]);
    let mut env = EnvGuard::set([
        ("HOME", f.e.home.to_str().unwrap()),
        ("AMPLIHACK_CLAUDE_PROFILE_ROOT", f.e.root.to_str().unwrap()),
    ]);
    env.clear("CLAUDE_CONFIG_DIR");
    for var in ENV_CREDENTIAL_VARS {
        env.clear(var);
    }
    let mut ctx = ctx_with(&f.e, measure);
    ctx.now = clock;
    ctx.settings = reporter();
    ctx.reread = Some(confirm);
    ctx.reread_live = None;
    let t = clock();
    let (outcome, calls) = cycle(
        &mut ctx,
        t,
        rd(98.0, 86.0, None),
        Some(("target", rd(0.0, 72.0, None))),
        None,
        rd(0.0, 72.0, Some(t)),
    );
    assert_eq!(outcome, CycleOutcome::Swapped);
    assert_eq!(calls, [name("target")]);
    assert!(f.live_is("target"));
    let swapped_at = f.e.rs.load_ledger().unwrap().unwrap().swapped_at.unwrap();
    run(&f, &mut ctx, swapped_at);
}

fn reading(session: f64, week: f64, at: i64, resets_at: i64) -> Probed {
    let mut p = rd(session, week, Some(at));
    p.windows.session.as_mut().unwrap().resets_at = Some(resets_at);
    p.windows.week.as_mut().unwrap().resets_at = Some(resets_at + 3 * 86_400);
    p
}

fn rising_then_cached(f: &Fleet, ctx: &mut Ctx, t: i64, reset_after: i64) {
    // The reported points, with interpolated readings between them. The
    // last part rises two points a minute. All 27 are taken after the swap.
    let mut start = (37, 2.0, 72.0);
    let mut samples = vec![start];
    for (end, steps) in [
        ((341, 17.0, 74.0), 7),
        ((601, 31.0, 77.0), 6),
        ((910, 44.0, 79.0), 7),
        ((1038, 46.5, 79.5), 2),
        ((1173, 51.0, 80.0), 4),
    ] {
        for i in 1..=steps {
            let fraction = i as f64 / steps as f64;
            samples.push((
                start.0 + (end.0 - start.0) * i / steps,
                start.1 + (end.1 - start.1) * fraction,
                start.2 + (end.2 - start.2) * fraction,
            ));
        }
        start = end;
    }
    let room = rd(0.0, 77.0, None);
    let mut last = Probed::failed();
    for (offset, session, week) in samples {
        last = reading(session, week, t + offset, t + reset_after);
        let (outcome, calls) = cycle(
            ctx,
            t + offset,
            last.clone(),
            Some(("reserve", room.clone())),
            None,
            room.clone(),
        );
        assert_eq!(outcome, CycleOutcome::Stayed);
        assert!(calls.is_empty());
    }
    last.passive = true;
    for offset in [1224, 1274, 1325, 1376, 1427, 1478] {
        let (outcome, calls) = cycle(
            ctx,
            t + offset,
            last.clone(),
            Some(("reserve", room.clone())),
            Some(ProbeLabel::Http429),
            room.clone(),
        );
        assert_eq!(outcome, CycleOutcome::Stayed);
        assert_eq!(ctx.failed_live, 0);
        assert!(calls.is_empty());
    }
    assert!(f.live_is("target"));
}

fn fail(
    ctx: &mut Ctx,
    at: i64,
    label: Option<ProbeLabel>,
    target: Option<Probed>,
    confirmation: Probed,
) -> (CycleOutcome, Vec<ProfileName>) {
    let mut live = if label.is_none() {
        expired()
    } else {
        Probed::failed()
    };
    live.still_running = label == Some(ProbeLabel::StillRunning);
    cycle(
        ctx,
        at,
        live,
        target.map(|p| ("reserve", p)),
        label,
        confirmation,
    )
}

#[test]
fn rising_use_swaps_on_the_third_counted_failure_after_cached_cycles() {
    for label in [
        None,
        Some(ProbeLabel::Http429),
        Some(ProbeLabel::StillRunning),
    ] {
        after_swap(|f, ctx, t| {
            rising_then_cached(f, ctx, t, 7200);
            for (i, offset) in [1528, 1578, 1627].into_iter().enumerate() {
                let at = t + offset;
                let (outcome, calls) = fail(
                    ctx,
                    at,
                    label,
                    Some(rd(0.0, 77.0, None)),
                    rd(0.0, 77.0, Some(at)),
                );
                if i < 2 {
                    assert_eq!(outcome, CycleOutcome::StayedUnread);
                    assert_eq!(ctx.failed_live, i as u32 + 1);
                    assert!(calls.is_empty());
                    assert!(f.live_is("target"));
                } else {
                    assert_eq!(outcome, CycleOutcome::Swapped);
                    assert_eq!(calls, [name("reserve")]);
                    assert!(f.live_is("reserve"));
                }
            }
        });
    }
}

#[test]
fn the_first_incident_swaps_on_the_third_failure_with_an_applicable_peak() {
    for open in [true, false] {
        after_swap(|f, ctx, t| {
            // The report does not give a pre-failure figure. Assume a
            // stored peak at the weekly threshold. The later 100% reading
            // is absent, as it had yet to arrive at the time of decision.
            let reset = t + if open { 7200 } else { 1000 };
            let mut ledger = f.e.rs.load_ledger().unwrap().unwrap();
            let mut peak = rd(5.0, f64::from(ctx.settings.week_at), None);
            peak.windows.week.as_mut().unwrap().resets_at = Some(reset);
            ledger.high_water.raise("target", &peak, t);
            f.e.rs.save_ledger(&ledger).unwrap();
            for (i, offset) in [926, 1117, 1308].into_iter().enumerate() {
                let at = t + offset;
                let (outcome, calls) = fail(
                    ctx,
                    at,
                    None,
                    Some(rd(0.0, 0.0, None)),
                    rd(0.0, 0.0, Some(at)),
                );
                let swaps = open && i == 2;
                assert_eq!(outcome == CycleOutcome::Swapped, swaps);
                assert_eq!(calls.len(), usize::from(swaps));
                assert!(f.live_is(if swaps { "reserve" } else { "target" }));
            }
        });
    }
}

#[test]
fn a_last_weekly_reading_at_the_threshold_is_enough_without_a_peak() {
    after_swap(|f, ctx, t| {
        // The probe has no reset time for the peak store. No target has room
        // on this cycle, so the high observation is available later.
        let (_, calls) = cycle(
            ctx,
            t + 37,
            rd(5.0, f64::from(ctx.settings.week_at), None),
            None,
            None,
            Probed::failed(),
        );
        assert!(calls.is_empty());
        assert!(f.live_is("target"));
        for offset in [85, 133] {
            let (outcome, calls) = fail(
                ctx,
                t + offset,
                None,
                Some(rd(0.0, 77.0, None)),
                Probed::failed(),
            );
            assert_eq!(outcome, CycleOutcome::StayedUnread);
            assert!(calls.is_empty());
        }
        let (outcome, calls) = fail(
            ctx,
            t + 181,
            None,
            Some(rd(0.0, 77.0, None)),
            rd(0.0, 77.0, Some(t + 181)),
        );
        assert_eq!(outcome, CycleOutcome::Swapped);
        assert_eq!(calls, [name("reserve")]);
        assert!(f.live_is("reserve"));
    });
}

#[test]
fn flat_low_use_and_a_projection_past_the_reset_leave_the_live_login_in_place() {
    for rising in [false, true] {
        after_swap(|f, ctx, t| {
            if rising {
                rising_then_cached(f, ctx, t, 1900);
            } else {
                for offset in [37, 341, 601, 910, 1173] {
                    let (outcome, calls) = cycle(
                        ctx,
                        t + offset,
                        reading(5.0, 72.0, t + offset, t + 7200),
                        Some(("reserve", rd(0.0, 77.0, None))),
                        None,
                        Probed::failed(),
                    );
                    assert_eq!(outcome, CycleOutcome::Stayed);
                    assert!(calls.is_empty());
                }
            }
            for offset in [1528, 1578, 1627] {
                let (outcome, calls) = fail(
                    ctx,
                    t + offset,
                    None,
                    Some(rd(0.0, 77.0, None)),
                    Probed::failed(),
                );
                assert_eq!(outcome, CycleOutcome::StayedUnread);
                assert!(calls.is_empty());
                assert!(f.live_is("target"));
            }
        });
    }
}

#[test]
fn high_use_cannot_move_without_room_and_a_fresh_confirmation() {
    after_swap(|f, ctx, t| {
        rising_then_cached(f, ctx, t, 7200);
        for offset in [1528, 1578, 1627] {
            let (outcome, calls) = fail(ctx, t + offset, None, None, Probed::failed());
            assert_eq!(outcome, CycleOutcome::Alerted);
            assert!(calls.is_empty());
            assert!(f.live_is("target"));
        }
        let cached = rd(0.0, 77.0, Some(t + 1173));
        let (outcome, calls) = fail(ctx, t + 1677, None, Some(rd(0.0, 77.0, None)), cached);
        assert_ne!(outcome, CycleOutcome::Swapped);
        assert_eq!(calls, [name("reserve")]);
        assert!(f.live_is("target"));
        let (outcome, calls) = fail(
            ctx,
            t + 1727,
            None,
            Some(rd(0.0, 77.0, None)),
            rd(0.0, 77.0, Some(t + 1727)),
        );
        assert_eq!(outcome, CycleOutcome::Swapped);
        assert_eq!(calls, [name("reserve")]);
        assert!(f.live_is("reserve"));
    });
}
