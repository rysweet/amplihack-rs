//! Real reader, passive reconstruction, decision, confirmation and swap.
//! Only the probes are injected. Every profile and usage file is temporary.

use super::cycle::{Ctx, CycleOutcome, Mode};
use super::decide::Probed;
use super::decide::target_seen::tests_issue_2910_fixture::{
    clock, fleet, name, reporter, run_full,
};
use super::measure::{self, ProbeLabel};
use super::observe::{Measurement, Snap, decide_measured};
use super::tests_issue_1870::exh;
use super::tests_issue_3301_reading::{env, report};
use crate::commands::profile::ProfileError;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::probe_limits::ProbeMode;
use crate::commands::profile::usage_cmd::{ErrorReason, Probe};
use crate::commands::profile::usage_source::Source;
use std::cell::RefCell;
use std::time::Duration;

struct Scene {
    mode: ProbeMode,
    model: Option<f64>,
    stored_model: bool,
    calls: Vec<ProfileName>,
}

thread_local! {
    static SCENE: RefCell<Scene> = const { RefCell::new(Scene {
        mode: ProbeMode::Text, model: Some(100.0), stored_model: false, calls: Vec::new(),
    }) };
}

fn measure_passive(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let probes = snap
        .candidates
        .iter()
        .map(|who| {
            let stored_model = SCENE.with(|s| s.borrow().stored_model);
            let reader = measure::reader(ctx, env(ProbeMode::Text), 1, clock());
            reader.profile_with(who, false, |mode| {
                let mut r = report(mode, clock(), stored_model);
                r.session.as_mut().unwrap().used_percentage =
                    if who.as_str() == "top" { 10.0 } else { 99.0 };
                Probe::read(r, Source::Fallback)
            });
            let reader = measure::reader(ctx, env(ProbeMode::Ordered), 1, clock());
            let p = reader.profile_with(who, false, |_| panic!("expected passive reconstruction"));
            assert_eq!(p.source, Some(Source::Passive));
            assert_eq!(p.report.week_models.len(), usize::from(stored_model));
            (who.clone(), measure::reading(who.as_str(), &p))
        })
        .collect();
    decide_measured(ctx, snap, exh(100.0, 20.0), probes, clock())
}

fn confirm(ctx: &Ctx, who: &ProfileName, _: Duration) -> (Probed, Option<ProbeLabel>) {
    let (mode, model) = SCENE.with(|s| {
        let mut s = s.borrow_mut();
        s.calls.push(who.clone());
        (s.mode, s.model)
    });
    let reader = measure::reader(ctx, env(mode), 1, clock());
    let p = reader.profile_with(who, false, |mode| {
        let Some(used) = model else {
            return Probe::failed(ErrorReason::ReadFailed, None);
        };
        let mut r = report(mode, clock(), true);
        r.week_models[0].used_percentage = used;
        Probe::read(r, Source::Fallback)
    });
    (measure::reading(who.as_str(), &p), measure::label_of(&p))
}

#[test]
fn issue_3301_every_swap_confirms_its_fresh_passive_target_and_rejects_model_equality() {
    for mode in [ProbeMode::Auto, ProbeMode::Text] {
        for used in [97.0, 98.0, 100.0] {
            let f = fleet(&["top"], &[]);
            SCENE.with(|s| {
                *s.borrow_mut() = Scene {
                    mode,
                    model: Some(used),
                    stored_model: false,
                    calls: Vec::new(),
                }
            });
            let mut settings = reporter();
            settings.week_at = 98;
            settings.target_week_max = 100;
            let out = run_full(
                &f,
                settings,
                exh(100.0, 20.0),
                &[],
                &[],
                Mode::Watch,
                Some(confirm),
                measure_passive,
            );
            assert_eq!(
                out == CycleOutcome::Swapped,
                used < 98.0,
                "mode={mode:?}, model={used}, outcome={out:?}\n{}",
                f.log(),
            );
            assert_eq!(f.live_is("top"), used < 98.0);
            assert_eq!(f.live_is("work"), used >= 98.0);
            SCENE.with(|s| assert_eq!(s.borrow().calls, [name("top")]));
        }
    }
}

#[test]
fn issue_3301_reconsideration_waits_for_another_cycle_to_confirm_an_alternative() {
    let f = fleet(&["top", "alternative"], &[]);
    SCENE.with(|s| {
        *s.borrow_mut() = Scene {
            mode: ProbeMode::Text,
            model: Some(100.0),
            stored_model: false,
            calls: Vec::new(),
        }
    });
    let out = run_full(
        &f,
        reporter(),
        exh(100.0, 20.0),
        &[],
        &[],
        Mode::Watch,
        Some(confirm),
        measure_passive,
    );
    assert_ne!(out, CycleOutcome::Swapped);
    assert!(f.live_is("work"));
    SCENE.with(|s| assert_eq!(s.borrow().calls, [name("top")]));
}

#[test]
fn issue_3301_unsuccessful_confirmation_cannot_move_off_a_blocked_login() {
    let f = fleet(&["top", "alternative"], &[]);
    f.block("work", "session", clock() + 3600);
    SCENE.with(|s| {
        *s.borrow_mut() = Scene {
            mode: ProbeMode::Text,
            model: None,
            stored_model: false,
            calls: Vec::new(),
        }
    });
    let out = run_full(
        &f,
        reporter(),
        exh(100.0, 20.0),
        &[],
        &[],
        Mode::Watch,
        Some(confirm),
        measure_passive,
    );
    assert_ne!(out, CycleOutcome::Swapped);
    assert!(f.live_is("work"));
    SCENE.with(|s| assert_eq!(s.borrow().calls, [name("top")]));
}

#[test]
fn issue_3301_stored_exhausted_model_is_never_selected_from_passive_usage() {
    let f = fleet(&["top"], &[]);
    SCENE.with(|s| {
        *s.borrow_mut() = Scene {
            mode: ProbeMode::Text,
            model: Some(10.0),
            stored_model: true,
            calls: Vec::new(),
        }
    });
    let out = run_full(
        &f,
        reporter(),
        exh(100.0, 20.0),
        &[],
        &[],
        Mode::Watch,
        Some(confirm),
        measure_passive,
    );
    assert_ne!(out, CycleOutcome::Swapped);
    assert!(f.live_is("work"));
    SCENE.with(|s| assert!(s.borrow().calls.is_empty()));
}
