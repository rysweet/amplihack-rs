//! The wait's log lines (issue #1618): `amplihack.recipe.usage_limit_wait`
//! followed by the event. Built only from static text, integers and parsed
//! instants, with one exception: the recipe-log `start` line carries the
//! login being waited on, which [`recipe_line`] re-validates against the
//! rotator's own name form before it can reach a file (issue #2731).
//!
//! Tested in `tests_log.rs` and `tests_log_recipe.rs`.

use super::plan::{Bound, NoWait};
use super::wait::End;
use crate::commands::recipe::run::limit_patterns::Harness;
use chrono::TimeZone;
use std::fmt::Display;

const PREFIX: &str = "amplihack.recipe.usage_limit_wait";
/// The `account=` value when no login name can be used.
const UNKNOWN: &str = "unknown";

/// Which kind of wait a `start` line describes. A reset wait's line is
/// unchanged; the other two name themselves in the same place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Shape {
    Reset,
    /// Any login with capacity (issue #1801).
    Capacity,
    /// The login in `$HOME/.claude` changing (issue #2731).
    LoginChange,
}

impl Shape {
    fn token(self) -> &'static str {
        match self {
            Shape::Reset => "",
            Shape::Capacity => " wait=capacity",
            Shape::LoginChange => " wait=login_change",
        }
    }

    /// The limit that started the wait, for the `start` line's `reason=`.
    ///
    /// `reason` names the limit and `wait` names what the step is waiting
    /// for; the two answer different questions and both are written. A
    /// login-change wait is started by an organisation spend limit and by
    /// nothing else (`plan::upgrade_spend`), so a reader grepping the recipe
    /// log for `spend_limit` finds the wait as well as the step's closing
    /// line (issue #2731).
    fn reason(self) -> &'static str {
        match self {
            Shape::Reset | Shape::Capacity => "usage_limit",
            Shape::LoginChange => "spend_limit",
        }
    }
}

/// One log line's content.
pub(super) enum Event {
    Start {
        harness: Harness,
        /// `None` when a capacity wait's limit stated no readable reset.
        reset: Option<i64>,
        until: i64,
        bound: Bound,
        /// Which kind of wait this is; a reset wait's line is unchanged.
        shape: Shape,
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
    /// The retry a wait earned met a limit of its own, so the signal that
    /// ended that wait was wrong and the step waits again (issue #2857).
    /// Written once per re-entry, immediately before the new wait's
    /// [`Event::Start`], so a reader can tell a second wait that follows a
    /// contradicted signal from one that follows a new login.
    Reentered {
        met: Met,
    },
}

/// Which limit met the retry a wait earned (issue #2857). A closed set, so
/// the line it builds carries no text from the attempt's output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Met {
    /// The harness's own usage-limit message, planned as any other is.
    UsageLimit,
    /// A bare 429 envelope, which states no reset.
    RateLimit,
}

/// Why the step ended on a usage limit after a wait could not lift it.
#[derive(Clone, Copy)]
pub(super) enum FailReason {
    CapacityExpired,
    /// A login-change wait reached its deadline: no reset lifts an
    /// organisation spend limit, and no login arrived that might (#2731).
    SpendLimit,
    StepTimeout,
    AlreadyWaited,
}

impl FailReason {
    fn as_str(self) -> &'static str {
        match self {
            FailReason::CapacityExpired => "capacity_expired",
            FailReason::SpendLimit => "spend_limit",
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

// `t` in zone `tz`, as every line writes an instant. The one formatter is
// shared with the heartbeat of a waiting step (issue #2967), so that line's
// time and the `until=` field of the wait's `start` line cannot differ. It
// lives beside the heartbeat because that module is built on every host and
// this one only on Unix.
use crate::commands::recipe::run::heartbeat::fmt_time;

/// A `start` line's body, with `who` naming the harness or the account.
fn start_body<Z: TimeZone>(
    shape: Shape,
    who: &str,
    reset: Option<i64>,
    until: i64,
    bound: Bound,
    tz: &Z,
) -> String
where
    Z::Offset: Display,
{
    format!(
        "start reason={}{} {} reset={} until={} bound={}",
        shape.reason(),
        shape.token(),
        who,
        reset.map_or_else(|| "none".to_string(), |r| fmt_time(r, tz)),
        fmt_time(until, tz),
        bound.as_str()
    )
}

/// The line a wait appends to the run's recipe log, or `None` for an event
/// that does not belong there (issue #2731). Only a wait's `start` and `end`,
/// and the `reentered` between two of them (issue #2857), do: the recipe log
/// says that the step is waiting, on which login, until when, how the wait
/// ended and whether the step went back into one, and the step's stderr keeps
/// the rest.
///
/// The `end` and `reentered` lines are the stderr ones. The `start` line is
/// the stderr one with `harness=` dropped and `account=` put in its place, so
/// one `grep` over both sinks reads as one stream. `account` is re-validated
/// here, at the one point where a login name can reach a file: a name outside
/// the rotator's own form is written as `unknown` whole rather than trimmed
/// to fit, so two logins can never collapse onto one identity in the log.
pub(super) fn recipe_line<Z: TimeZone>(e: &Event, account: Option<&str>, tz: &Z) -> Option<String>
where
    Z::Offset: Display,
{
    match e {
        Event::Start {
            reset,
            until,
            bound,
            shape,
            ..
        } => {
            let name = account.filter(|n| amplihack_utils::rotator_status::valid_live_name(n));
            let who = format!("account={}", name.unwrap_or(UNKNOWN));
            Some(format!(
                "{PREFIX} {}",
                start_body(*shape, &who, *reset, *until, *bound, tz)
            ))
        }
        Event::End(_) | Event::Reentered { .. } => Some(line(e, tz)),
        _ => None,
    }
}

/// The log line for `e`, times in zone `tz`.
pub(super) fn line<Z: TimeZone>(e: &Event, tz: &Z) -> String
where
    Z::Offset: Display,
{
    let rest = match e {
        Event::Start {
            harness,
            reset,
            until,
            bound,
            shape,
        } => start_body(
            *shape,
            &format!("harness={}", harness.as_str()),
            *reset,
            *until,
            *bound,
            tz,
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
        Event::Reentered { met } => format!(
            "reentered met={}",
            match met {
                Met::UsageLimit => "usage_limit",
                Met::RateLimit => "rate_limit",
            }
        ),
    };
    format!("{PREFIX} {rest}")
}

pub(super) fn log(e: &Event) {
    eprintln!("{}", line(e, &chrono::Local));
}
