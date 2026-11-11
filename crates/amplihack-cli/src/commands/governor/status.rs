//! `amplihack governor status [--json]`: a read-only view. It takes no lock,
//! opens no lock file, and writes, reclaims and quarantines nothing; dead
//! records are shown as `stale`. JSON schema version 1:
//! docs/reference/capacity-governor.md#json-output.

use super::budget::{Headroom, headroom, invalid_reading, stale_reading, weekly_remaining};
use super::config::Settings;
use super::inputs::{self, ProfileRates};
use super::liveness::alive;
use super::model::{Budget, DecisionKind, Inputs, ReasonCode, Request, Start, WindowBudget};
use super::policy::decide;
use super::queue::admissions_to;
use super::rate::RateSignal;
use super::rate::WindowRate;
use super::store::{History, Peek, Queue, State, Store};
use std::collections::BTreeSet;
use std::path::Path;

pub use super::status_table::{display_safe, render_table};
pub use super::status_views::{
    ProfileView, QueueView, ReasonView, ReserveView, SlotView, StatusView, WindowView,
    WorkstreamView,
};

pub const STATUS_VERSION: u32 = 1;

fn window(w: &WindowBudget, r: &WindowRate) -> WindowView {
    WindowView {
        used_pct: w.used_pct,
        resets_at: w.resets_at,
        burn_rate: r.burn,
        sustainable_rate: r.sustainable,
        forecast_exhaustion: r.forecast,
    }
}

/// Whether `policy::decide` would hold a top-level ordinary start on these
/// readings, and at which step: `weekly-exhausted` (step 2),
/// `session-exhausted` (step 3) or `reserve` (step 4). It asks `decide`
/// itself, with no slots and no queue, so steps 5 and 6 never apply and the
/// answer cannot drift from admission. `None` when such a start would be
/// admitted, including the fail-open `no-readings` case (step 1).
pub(super) fn ordinary_hold(
    budgets: &[Budget],
    settings: &Settings,
    now: i64,
) -> Option<ReasonCode> {
    let inputs = Inputs {
        budgets: budgets.to_vec(),
        signal: RateSignal::Unknown,
        limit: settings.max_concurrency.max(1),
        last_evaluated_at: Some(now),
        slots: Vec::new(),
        queue: Vec::new(),
        admissions: Vec::new(),
        settings: *settings,
        request: Request {
            class: super::model::Class::Ordinary,
            workstream: String::new(),
            start: Start::Agent,
            parent: None,
            seq: None,
            deadline: now,
        },
    };
    let d = decide(&inputs, now);
    (d.kind != DecisionKind::Admit).then_some(d.reason.code)
}

fn peek<T: super::store::Persisted>(root: &Path, reason: &mut Option<ReasonView>) -> T {
    let (code, message) = match Store::peek::<T>(root) {
        Peek::Ok(v) => return v,
        Peek::Missing => return T::default(),
        Peek::Corrupt => (
            "corrupt-state",
            format!("{} is corrupt; the next start quarantines it", T::FILE),
        ),
        Peek::NewerVersion => (
            "state-version",
            format!("{} is from a newer amplihack", T::FILE),
        ),
    };
    reason.get_or_insert(ReasonView {
        code: code.into(),
        message,
    });
    T::default()
}

fn empty(settings: &Settings, now: i64, enabled: bool) -> StatusView {
    StatusView {
        version: STATUS_VERSION,
        generated_at: now,
        enabled,
        limit: enabled.then_some(settings.max_concurrency),
        max: settings.max_concurrency,
        profiles: Vec::new(),
        reserve: ReserveView {
            points: settings.reserve_points,
            best_weekly_remaining: None,
            ordinary_held: false,
            held_reason: None,
        },
        slots: Vec::new(),
        queue: Vec::new(),
        workstreams: Vec::new(),
        fail_open_reason: None,
    }
}

/// The status document. `root` is the profile root, `manifest` the
/// workstream usage manifest for `token_share`.
pub fn view(
    root: Option<&Path>,
    settings: &Settings,
    manifest: Option<&Path>,
    now: i64,
) -> StatusView {
    if !settings.enabled {
        return empty(settings, now, false);
    }
    let Some(root) = root else {
        return empty(settings, now, true);
    };
    let mut reason = None;
    let state: State = peek(root, &mut reason);
    let queue: Queue = peek(root, &mut reason);
    let history: History = peek(root, &mut reason);
    if reason.is_none() {
        reason = state.fail_open.as_ref().map(|f| ReasonView {
            code: f.code.as_str().into(),
            message: f.message.clone(),
        });
    }
    let snaps = inputs::snapshots(root);
    let budgets = inputs::budgets(&snaps, now, settings.max_reading_age_secs);
    let (rates, _) = inputs::rates(&budgets, &history, now, settings.band_pct);
    let profiles = snaps
        .iter()
        .map(|s| {
            let b = budgets.iter().find(|b| b.name == s.name);
            let r: Option<&ProfileRates> = rates.get(&s.name);
            let weekly = b.zip(r).map(|(b, r)| window(&b.weekly, &r.weekly));
            let session = b.zip(r).and_then(|(b, r)| {
                b.session
                    .as_ref()
                    .zip(r.session.as_ref())
                    .map(|(w, r)| window(w, r))
            });
            ProfileView {
                name: display_safe(&s.name),
                active: s.active,
                eligible: s.eligible,
                session_window: !s.no_session_window,
                invalid_reading: invalid_reading(s),
                stale_reading: stale_reading(s, settings.max_reading_age_secs, now),
                observed_at: (s.five_hour.is_some() || s.seven_day.is_some())
                    .then_some(s.observed_at),
                burn_rate: weekly.as_ref().and_then(|w| w.burn_rate),
                sustainable_rate: weekly.as_ref().and_then(|w| w.sustainable_rate),
                forecast_exhaustion: weekly.as_ref().and_then(|w| w.forecast_exhaustion),
                session,
                weekly,
            }
        })
        .collect();
    // The pool step 4 of `policy::decide` reads (`with_session`): eligible,
    // and neither window exhausted. A stale reading is kept in `budgets` only
    // when one of its windows is exhausted, so it never enters this pool and
    // its other window never counts as room, here or at admission.
    let pool: Vec<_> = budgets
        .iter()
        .filter(|b| {
            b.eligible
                && !matches!(
                    headroom(b, 0.0),
                    Headroom::WeeklyExhausted | Headroom::SessionExhausted
                )
        })
        .collect();
    let held_reason = ordinary_hold(&budgets, settings, now);
    let reserve = ReserveView {
        points: settings.reserve_points,
        best_weekly_remaining: pool.iter().map(|b| weekly_remaining(b)).reduce(f64::max),
        ordinary_held: held_reason.is_some(),
        held_reason: held_reason.map(ReasonCode::as_str),
    };
    let max = settings.max_concurrency.max(1);
    let limit = if state.limit == 0 {
        max
    } else {
        state.limit.clamp(1, max)
    };
    let slots: Vec<SlotView> = state
        .slots
        .iter()
        .map(|s| SlotView {
            pid: s.pid,
            workstream: s.workstream.clone(),
            class: s.class,
            admitted_at: s.admitted_at,
            nested_children: s.nested_children,
            stale: !alive(s.pid, s.start_time),
        })
        .collect();
    let waiting: Vec<QueueView> = queue
        .entries
        .iter()
        .map(|q| QueueView {
            seq: q.seq,
            workstream: q.workstream.clone(),
            class: q.class,
            pid: q.pid,
            enqueued_at: q.enqueued_at,
            stale: !alive(q.pid, q.start_time),
        })
        .collect();
    let shares = manifest
        .map(super::manifest::token_shares)
        .unwrap_or_default();
    let names: BTreeSet<&str> = state
        .admissions
        .iter()
        .map(|a| a.workstream.as_str())
        .chain(state.slots.iter().map(|s| s.workstream.as_str()))
        .chain(queue.entries.iter().map(|q| q.workstream.as_str()))
        .collect();
    let since = now.saturating_sub(super::queue::GOVERNOR_WINDOW_SECS);
    let recent: Vec<_> = state
        .admissions
        .iter()
        .filter(|a| a.at >= since)
        .cloned()
        .collect();
    let workstreams = names
        .into_iter()
        .map(|name| WorkstreamView {
            name: name.to_string(),
            admissions: admissions_to(name, &recent),
            slots: state.slots.iter().filter(|s| s.workstream == name).count(),
            waiting: queue
                .entries
                .iter()
                .filter(|q| q.workstream == name)
                .count(),
            token_share: shares.get(name).copied(),
        })
        .collect();
    StatusView {
        version: STATUS_VERSION,
        generated_at: now,
        enabled: true,
        limit: Some(limit),
        max: settings.max_concurrency,
        profiles,
        reserve,
        slots,
        queue: waiting,
        workstreams,
        fail_open_reason: reason,
    }
}
