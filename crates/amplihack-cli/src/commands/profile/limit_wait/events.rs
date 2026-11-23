//! The wait's log lines (issue #1618): `amplihack.recipe.usage_limit_wait`
//! followed by the event. Built only from static text, integers and parsed
//! instants, with one exception: the recipe-log `start` and
//! `waiting_for_capacity` lines carry the login being waited on, which
//! [`recipe_line`] re-validates against the rotator's own name form before
//! it can reach a file (issues #2731, #2911).
//!
//! Tested in `tests_log.rs` and `tests_log_recipe.rs`.

use super::plan::{Bound, NoWait, Waiting};
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
    /// login-change wait is started by a spend limit that states no reset
    /// and by nothing else (`plan::wait_for_message`), so a reader grepping
    /// the recipe log for `spend_limit` finds the wait's `start` line beside
    /// its `waiting_for_capacity` line (issues #2731, #2911).
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
        /// `None` for a wait with no deadline, written `until=none`: its
        /// limit stated no usable reset, so it ends only on an event.
        until: Option<i64>,
        bound: Bound,
        /// Which kind of wait this is; a reset wait's line is unchanged.
        shape: Shape,
    },
    Progress {
        /// Seconds to the deadline, or `None` for a wait that has none,
        /// written `remaining=none`.
        remaining: Option<u64>,
    },
    End(End),
    Resume {
        attempt: u32,
    },
    Skipped(Skip),
    /// The retry a wait earned met a limit of its own, so the signal that
    /// ended that wait was wrong and the step waits again (issue #2857).
    /// Written once per re-entry, immediately before the new wait's
    /// [`Event::Start`], so a reader can tell a second wait that follows a
    /// contradicted signal from one that follows a new login.
    Reentered {
        met: Met,
    },
    /// The step is waiting for capacity rather than for a reset within
    /// five hours (issue #2911). Written immediately before the wait's
    /// [`Event::Start`], to the step's stderr and to the run's recipe log, so
    /// an operator reading either can see that the step is waiting rather
    /// than hung, which login it is on, why, and until when: a far reset, or
    /// `until=none` when nothing can be known about when the limit lifts and
    /// the wait ends only on a login with room or a login change.
    ///
    /// Every field is a closed set, an integer or a formatted instant: the
    /// harness's own words reach neither sink.
    WaitingForCapacity {
        harness: Harness,
        why: Waiting,
        until: Option<i64>,
    },
    /// The step met a limit, but it ends because its prompt was cut short,
    /// not because of the limit: the attempt was given part of its task, so
    /// the step fails whether or not it met one, and no retry can be given
    /// the whole prompt. Written as `failed reason=prompt_cut_short`, a
    /// failure of the step's input, and never as a skipped wait, which would
    /// read as a wait refused to a limit.
    PromptCutShort,
    /// No file event can be had for this wait (issue #2911), so it cannot
    /// see a login gain room or change: it ends on its reset, a stop signal
    /// or the end of its session, and reads no file on a timer instead.
    /// `why` is a fixed word naming the cause (`wake`).
    NoFileEvents {
        why: &'static str,
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

/// Why no wait was made at all.
#[derive(Clone, Copy)]
pub(super) enum Skip {
    /// The operator set `recipe.usage_limit_wait = false`. `CI` no longer
    /// turns the wait off, so there is no `ci` reason (issue #2911).
    Disabled,
    NotSupervised,
    /// The output carries no limit message. A prompt cut short is not a
    /// skip and is written as [`Event::PromptCutShort`].
    Cannot(NoWait),
}

// `t` in zone `tz`, as every line writes an instant. The one formatter is
// shared with the heartbeat of a waiting step (issue #2967), so that line's
// time and the `until=` field of the wait's `start` line cannot differ. It
// lives beside the heartbeat because that module is built on every host and
// this one only on Unix. Re-exported `pub(super)` because the sentence the
// supervisor prints beside the prompt-loss ending (`ending`) also names an
// instant, and an instant must read the same in every line a step writes.
pub(super) use crate::commands::recipe::run::heartbeat::fmt_time;

/// An optional instant as a line writes it: the time, or `none`.
fn fmt_opt<Z: TimeZone>(t: Option<i64>, tz: &Z) -> String
where
    Z::Offset: Display,
{
    t.map_or_else(|| "none".to_string(), |t| fmt_time(t, tz))
}

/// A `start` line's body, with `who` naming the harness or the account.
fn start_body<Z: TimeZone>(
    shape: Shape,
    who: &str,
    reset: Option<i64>,
    until: Option<i64>,
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
        fmt_opt(reset, tz),
        fmt_opt(until, tz),
        bound.as_str()
    )
}

/// The line a wait appends to the run's recipe log, or `None` for an event
/// that does not belong there (issue #2731). Only a wait's `start` and `end`,
/// the `reentered` between two of them (issue #2857) and the
/// `waiting_for_capacity` before a `start` (issue #2911) do: the recipe log
/// says that the step is waiting, on which login, until when and why, how the
/// wait ended and whether the step went back into one, and the step's stderr
/// keeps the rest.
///
/// The `end` and `reentered` lines are the stderr ones. The `start` and
/// `waiting_for_capacity` lines are the stderr ones with `harness=` dropped
/// and `account=` put in its place, so one `grep` over both sinks reads as
/// one stream. `account` is re-validated here ([`account_field`]), at the one
/// point where a login name can reach a file: a name outside the rotator's
/// own form is written as `unknown` whole rather than trimmed to fit, so two
/// logins can never collapse onto one identity in the log.
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
        } => Some(format!(
            "{PREFIX} {}",
            start_body(*shape, &account_field(account), *reset, *until, *bound, tz)
        )),
        Event::WaitingForCapacity { why, until, .. } => Some(format!(
            "{PREFIX} {}",
            capacity_body(*why, &account_field(account), *until, tz)
        )),
        Event::End(_) | Event::Reentered { .. } => Some(line(e, tz)),
        _ => None,
    }
}

/// The `account=` field of a recipe-log line, re-validated as
/// [`recipe_line`] describes.
fn account_field(account: Option<&str>) -> String {
    let name = account.filter(|n| amplihack_utils::rotator_status::valid_live_name(n));
    format!("account={}", name.unwrap_or(UNKNOWN))
}

/// A `waiting_for_capacity` line's body, with `who` naming the harness or
/// the account, as [`start_body`] does.
///
/// The reason comes first, because that is the question an operator finding
/// a waiting step asks, and it is the field the tests match on.
fn capacity_body<Z: TimeZone>(why: Waiting, who: &str, until: Option<i64>, tz: &Z) -> String
where
    Z::Offset: Display,
{
    format!(
        "waiting_for_capacity reason={} {} until={}",
        why.as_str(),
        who,
        fmt_opt(until, tz)
    )
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
        Event::Progress { remaining } => match remaining {
            Some(secs) => format!("progress remaining={secs}s"),
            None => "progress remaining=none".to_string(),
        },
        Event::End(end) => format!("end reason={}", end.as_str()),
        Event::Resume { attempt } => format!("resume attempt={attempt}"),
        Event::WaitingForCapacity {
            harness,
            why,
            until,
        } => capacity_body(*why, &format!("harness={}", harness.as_str()), *until, tz),
        Event::Skipped(skip) => format!(
            "skipped reason={}",
            match skip {
                Skip::Disabled => "disabled",
                Skip::NotSupervised => "not_supervised",
                Skip::Cannot(why) => why.as_str(),
            }
        ),
        Event::PromptCutShort => format!("failed reason={}", NoWait::PromptCutShort.as_str()),
        Event::NoFileEvents { why } => format!("no_file_events reason={why}"),
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
