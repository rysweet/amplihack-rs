//! Convert generation-bound evidence without substituting display values.
use super::coordinator::InputState;
use super::cycle::Ctx;
use super::decide::{Decision, Probed, Reading, Win, Windows};
use super::observe::Measurement;
use crate::commands::profile::{ProfileError, name::ProfileName};
pub(super) fn observed(inputs: &InputState, name: &ProfileName) -> Probed {
    let Some(login) = inputs
        .logins
        .iter()
        .find(|l| l.profile == name.as_str() && l.eligible)
    else {
        return Probed::failed();
    };
    let Some(record) = inputs.records.iter().find(|r| {
        r.capture.binding.profile == login.profile
            && r.capture.binding.generation == login.generation
    }) else {
        return Probed::failed();
    };
    let e = &record.evidence;
    let reading = if e.limit_reached {
        Reading::Exhausted {
            session: e.session_used,
            week: e.week_used,
        }
    } else if let (Some(session), Some(week)) = (e.session_used, e.week_used) {
        Reading::Measured { session, week }
    } else if e.session_window == amplihack_utils::usage_observation::SessionWindow::Absent
        && let Some(week) = e.week_used
    {
        // The policy's numeric view uses zero only alongside the explicit
        // no-session-limit flag. Stored evidence retains the absent window.
        Reading::Measured { session: 0.0, week }
    } else {
        Reading::Failed
    };
    Probed {
        reading,
        windows: Windows {
            session: e.session_used.map(|used| Win {
                used,
                resets_at: e.session_reset_at,
            }),
            week: e.week_used.map(|used| Win {
                used,
                resets_at: e.week_reset_at,
            }),
            no_session_limit: e.session_window
                == amplihack_utils::usage_observation::SessionWindow::Absent,
        },
        unknown: reading == Reading::Failed,
        spend_limit: false,
        still_running: false,
        observed_at: Some(record.capture.captured_at),
        carried: false,
        passive: true,
    }
}

/// Generation validity replaces age-based rereads. A login used since its
/// evidence was captured still needs qualifying evidence after the move.
pub(super) fn gate_observed(ctx: &Ctx, mut m: Measurement) -> Result<Measurement, ProfileError> {
    let Some(to) = super::fresh::gated_target(&m.decision) else {
        return Ok(m);
    };
    let Some(inputs) = &ctx.event_inputs else {
        return Ok(m);
    };
    let ledger = ctx.rs.load_ledger()?.unwrap_or_default();
    let after = ledger.swapped_away.get(to.as_str()).copied().max(
        (ledger.previous.as_deref() == Some(to.as_str()))
            .then_some(ledger.swapped_at)
            .flatten(),
    );
    let captured = inputs
        .observations
        .iter()
        .find(|e| e.profile == to.as_str())
        .map(|e| e.captured_at);
    if captured.is_none() || after.is_some_and(|at| captured.is_none_or(|c| c <= at)) {
        m.decision = Decision::Stay;
    }
    Ok(m)
}

pub(super) fn measure(
    ctx: &Ctx,
    live: super::measure::Live<'_>,
    candidates: &[ProfileName],
) -> (Probed, Vec<(ProfileName, Probed)>) {
    use super::measure::Live;
    let Some(inputs) = &ctx.event_inputs else {
        unreachable!("event measurement requires captured inputs");
    };
    let current = match live {
        Live::Profile(n) => observed(inputs, n),
        Live::Platform => Probed::platform(),
        Live::Excluded => Probed::excluded(),
        Live::Unowned => Probed::failed(),
    };
    (
        current,
        candidates
            .iter()
            .map(|n| (n.clone(), observed(inputs, n)))
            .collect(),
    )
}
