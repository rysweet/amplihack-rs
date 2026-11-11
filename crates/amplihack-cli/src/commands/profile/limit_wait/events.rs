//! The wait's log lines (issue #1618): `amplihack.recipe.usage_limit_wait`
//! followed by the event. Built only from static text, integers and parsed
//! instants. Tested in `tests_log.rs`.

use super::plan::{Bound, NoWait};
use super::wait::End;
use crate::commands::recipe::run::limit_patterns::Harness;
use chrono::TimeZone;
use std::fmt::Display;

const PREFIX: &str = "amplihack.recipe.usage_limit_wait";

/// One log line's content.
pub(super) enum Event {
    Start {
        harness: Harness,
        /// `None` when a capacity wait's limit stated no readable reset.
        reset: Option<i64>,
        until: i64,
        bound: Bound,
        /// A capacity wait (issue #1801); a reset wait's line is unchanged.
        capacity: bool,
    },
    Progress {
        remaining: u64,
    },
    End(End),
    Resume {
        attempt: u32,
    },
    Skipped(Skip),
    /// The last line of a step that ends on a limit no wait lifted. Its text
    /// keeps the failure class in the recipe runner's 40-line evidence tail
    /// when progress lines have pushed the harness's own message out.
    Failed(FailReason),
}

/// Why the step ended on a usage limit after a wait could not lift it.
#[derive(Clone, Copy)]
pub(super) enum FailReason {
    CapacityExpired,
    StepTimeout,
    AlreadyWaited,
}

impl FailReason {
    fn as_str(self) -> &'static str {
        match self {
            FailReason::CapacityExpired => "capacity_expired",
            FailReason::StepTimeout => "step_timeout",
            FailReason::AlreadyWaited => "already_waited",
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum Skip {
    Disabled,
    AlreadyWaited,
    NotSupervised,
    /// Off because this is CI and nothing turned it on.
    Ci,
    /// The limit is not one a wait can outlast.
    Cannot(NoWait),
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
            capacity,
        } => format!(
            "start reason=usage_limit{} harness={} reset={} until={} bound={}",
            if *capacity { " wait=capacity" } else { "" },
            harness.as_str(),
            reset.map_or_else(|| "none".to_string(), time),
            time(*until),
            bound.as_str()
        ),
        Event::Progress { remaining } => format!("progress remaining={remaining}s"),
        Event::End(end) => format!("end reason={}", end.as_str()),
        Event::Resume { attempt } => format!("resume attempt={attempt}"),
        Event::Skipped(skip) => format!(
            "skipped reason={}",
            match skip {
                Skip::Disabled => "disabled",
                Skip::AlreadyWaited => "already_waited",
                Skip::NotSupervised => "not_supervised",
                Skip::Ci => "ci",
                Skip::Cannot(why) => why.as_str(),
            }
        ),
        Event::Failed(why) => format!("failed reason={}: usage limit not lifted", why.as_str()),
    };
    format!("{PREFIX} {rest}")
}

pub(super) fn log(e: &Event) {
    eprintln!("{}", line(e, &chrono::Local));
}
