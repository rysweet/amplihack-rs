//! The wait's log lines (issues #1618, #1761): `amplihack.recipe.usage_limit_wait`
//! followed by the event, and the plain lines written after a cap ends the
//! waiting. Built only from static text, integers, parsed instants and a
//! profile name that passes the profile-name rule; never a run id, a path,
//! an account or error text. Tested in `tests_log.rs` and `tests_notes.rs`.

use super::plan::{Bound, NoWait};
use super::settings::{BUDGET_KEY, MAX_WAITS_KEY};
use super::wait::End;
use crate::commands::recipe::run::limit_patterns::Harness;
use amplihack_utils::rotator_status::valid_live_name;
use chrono::TimeZone;
use std::fmt::Display;

const PREFIX: &str = "amplihack.recipe.usage_limit_wait";

/// One log line's content. `step` is the supervisor's process id.
pub(super) enum Event {
    Start {
        harness: Harness,
        reset: i64,
        until: i64,
        bound: Bound,
        step: u32,
        /// The attempt that hit the limit.
        attempt: u32,
        /// A rotator was seen at the check before the wait.
        rotator: bool,
    },
    Progress {
        remaining: u64,
    },
    End {
        end: End,
        /// The wait's own seconds.
        waited: u64,
    },
    Resume {
        attempt: u32,
        step: u32,
        waited: u64,
        /// The profile the retry runs on, if known.
        login: Option<String>,
    },
    Skipped {
        skip: Skip,
        step: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Skip {
    Disabled,
    /// No rotator, and this step has waited already.
    AlreadyWaited,
    /// A rotator runs, and this step has used all its waits.
    StepWaitsSpent,
    /// Nothing is left of the run's wait budget.
    BudgetSpent,
    NotSupervised,
    /// Off because this is CI and nothing turned it on.
    Ci,
    /// The limit is not one a wait can outlast.
    Cannot(NoWait),
}

impl Skip {
    fn as_str(self) -> &'static str {
        match self {
            Skip::Disabled => "disabled",
            Skip::AlreadyWaited => "already_waited",
            Skip::StepWaitsSpent => "step_waits_spent",
            Skip::BudgetSpent => "budget_spent",
            Skip::NotSupervised => "not_supervised",
            Skip::Ci => "ci",
            Skip::Cannot(why) => why.as_str(),
        }
    }
}

/// The log line for `e`, times in zone `tz`.
pub(super) fn line<Z: TimeZone>(e: &Event, tz: &Z) -> String
where
    Z::Offset: Display,
{
    let time = |t: i64| match tz.timestamp_opt(t, 0).single() {
        Some(d) => d.format("%Y-%m-%d %H:%M:%S %:z").to_string(),
        None => format!("@{t}"),
    };
    let rest = match e {
        Event::Start {
            harness,
            reset,
            until,
            bound,
            step,
            attempt,
            rotator,
        } => format!(
            "start reason=usage_limit harness={} reset={} until={} bound={} step={step} \
             attempt={attempt} rotator={}",
            harness.as_str(),
            time(*reset),
            time(*until),
            bound.as_str(),
            if *rotator { "running" } else { "none" }
        ),
        Event::Progress { remaining } => format!("progress remaining={remaining}s"),
        Event::End { end, waited } => format!("end reason={} waited={waited}s", end.as_str()),
        Event::Resume {
            attempt,
            step,
            waited,
            login,
        } => {
            let login = login
                .as_deref()
                .filter(|n| valid_live_name(n))
                .unwrap_or("unknown");
            format!("resume attempt={attempt} step={step} waited={waited}s login={login}")
        }
        Event::Skipped { skip, step } => {
            let rotator = if *skip == Skip::AlreadyWaited {
                " rotator=none"
            } else {
                ""
            };
            format!("skipped reason={} step={step}{rotator}", skip.as_str())
        }
    };
    format!("{PREFIX} {rest}")
}

pub(super) fn log(e: &Event) {
    eprintln!("{}", line(e, &chrono::Local));
}

/// The plain line after the budget ends the waiting: `budget` is the
/// setting, not the amount left; `per_step` when the step keeps its own
/// budget.
pub(super) fn budget_spent_note(budget: u64, per_step: bool) -> String {
    let scope = if per_step {
        "this step's"
    } else {
        "this recipe run's"
    };
    format!(
        "amplihack: {scope} usage-limit wait budget ({BUDGET_KEY} = {budget} s) is spent; \
         not waiting again"
    )
}

/// The plain line after a step has used its `max` waits.
pub(super) fn step_cap_note(max: u32) -> String {
    format!(
        "amplihack: this step's usage-limit waits ({MAX_WAITS_KEY} = {max}) are used up; \
         not waiting again"
    )
}
