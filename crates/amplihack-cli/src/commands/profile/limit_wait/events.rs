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
        reset: i64,
        until: i64,
        bound: Bound,
    },
    Progress {
        remaining: u64,
    },
    End(End),
    Resume {
        attempt: u32,
    },
    Skipped(Skip),
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
        } => format!(
            "start reason=usage_limit harness={} reset={} until={} bound={}",
            harness.as_str(),
            time(*reset),
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
    };
    format!("{PREFIX} {rest}")
}

pub(super) fn log(e: &Event) {
    eprintln!("{}", line(e, &chrono::Local));
}
