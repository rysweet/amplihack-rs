//! The heartbeat of a step that is waiting for a usage limit (issue #2967).
//!
//! The recipe runner prints a heartbeat for each agent step that has been
//! silent for a while. Its label ends in the process id of the step's
//! `amplihack <agent>` process, which is the step's supervisor:
//!
//! ```text
//!   [21:53:10] [amplihack:claude:41822] ... working (3590s elapsed, 3590s since last output)
//! ```
//!
//! A step that sleeps for hours in a usage-limit wait has no output, so that
//! line read as a step that was working. While the step's supervisor holds a
//! wait-state record (`profile::limit_wait::wait_state`), the stderr pump of
//! `amplihack recipe run` prints the line as
//!
//! ```text
//!   [21:53:10] [amplihack:claude:41822] ... waiting for usage limit until 2026-11-21 23:52:00 -08:00 (3590s elapsed, 3590s since last output)
//! ```
//!
//! A wait with no deadline (`until=none` on its own lines, issue #2911)
//! reads `waiting for usage limit until none`, in the words those lines use.
//!
//! Only the word `working` is replaced. Everything here is pure: the record
//! is read by the `lookup` the caller passes, and the zone is the caller's.
//!
//! None of this adds a timer, a polling loop, a retry or a gate. The pump
//! calls [`pump`] for each line the runner writes, as it always copied
//! them; the runner's own clock decides when a heartbeat line comes, and a
//! line that is not a heartbeat costs one string match. Nothing here ends,
//! delays or changes a step: it changes one word of a line a person reads.
//!
//! Why the line is rewritten here at all: the pinned runner counts only a
//! step's standard output as activity and keeps its standard error in a
//! file, so the `progress` line a waiting supervisor writes once a minute
//! (`profile::limit_wait::wait`) neither reaches the round log while the
//! step runs nor resets the heartbeat's `since last output`.
//!
//! The form is the one the pinned runner prints (`HeartbeatReport::render`
//! in `amplihack-recipe-runner`). A line in any other form, including the
//! `pid N alive` form runners printed before they labelled the heartbeat, is
//! left as it is.
//!
//! Two rules keep the rewrite away from the failure classifier, which reads
//! the pump's bounded buffer and would take the words `usage limit` for one:
//!
//! - the buffer keeps the runner's own line and never the rewrite;
//! - a line that is already a wait line ([`is_wait_line`], which a nested
//!   run's pump printed) is printed and not buffered.
//!
//! Both matches are anchored on the whole line and built from one set of
//! wording constants, so `is_wait_line(rewrite(x))` always holds. Any doubt
//! resolves to leaving the line alone and buffering it.
//!
//! Tested in `tests_heartbeat.rs`.

use super::limit_patterns::all_markers;
#[cfg(unix)]
use crate::commands::profile::limit_wait::wait_state;
use chrono::TimeZone;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What follows a heartbeat's label.
const LEAD: &str = "] ... ";
/// The runner's word for a step that is running.
const WORKING: &str = "working";
/// What replaces it while the step waits; the time, or [`NO_DEADLINE`],
/// follows.
const WAITING: &str = "waiting for usage limit until ";
/// What follows [`WAITING`] for a wait with no deadline, as `until=none`
/// says it on the wait's own lines.
const NO_DEADLINE: &str = "none";
/// How every wait line writes an instant.
const TIME_FORMAT: &str = "%Y-%m-%d %H:%M:%S %:z";
/// The length of an instant written with [`TIME_FORMAT`].
const TIME_LEN: usize = "2026-10-18 08:32:00 -07:00".len();
/// The largest process id a heartbeat may name.
const MAX_PID: u32 = i32::MAX as u32;

/// `t` in zone `tz`, as every usage-limit wait line writes an instant: the
/// `until=` field of a wait's `start` line and the time in a waiting step's
/// heartbeat are formatted by this one function.
pub(crate) fn fmt_time<Z: TimeZone>(t: i64, tz: &Z) -> String
where
    Z::Offset: Display,
{
    match tz.timestamp_opt(t, 0).single() {
        Some(d) => d.format(TIME_FORMAT).to_string(),
        None => format!("@{t}"),
    }
}

/// What the stderr pump does with one line of the runner's stderr.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Pumped {
    /// What goes to the terminal and the recipe log.
    pub(crate) print: String,
    /// What goes into the bounded failure-evidence buffer; `None` for
    /// nothing.
    pub(crate) buffer: Option<String>,
}

/// One line through the pump: the heartbeat of a waiting step is printed
/// rewritten and buffered as the runner wrote it, a line that is already a
/// wait line is printed and not buffered, and every other line is printed
/// and buffered unchanged.
pub(crate) fn pump<Z: TimeZone>(
    line: &str,
    lookup: impl Fn(u32) -> Option<Option<i64>>,
    tz: &Z,
) -> Pumped
where
    Z::Offset: Display,
{
    match rewrite(line, lookup, tz) {
        Some(rewritten) => Pumped {
            print: rewritten,
            buffer: Some(line.to_string()),
        },
        None => Pumped {
            print: line.to_string(),
            buffer: (!is_wait_line(line)).then(|| line.to_string()),
        },
    }
}

/// `line` with `working` replaced by the wait wording, when it is the
/// runner's heartbeat for a supervisor that is waiting. `lookup(pid)` is
/// `Some` with the `until` of that supervisor's live, unexpired wait-state
/// record, itself `None` for a wait with no deadline, and owns freshness.
/// `None` for every other line, which the caller then prints byte for byte.
pub(crate) fn rewrite<Z: TimeZone>(
    line: &str,
    lookup: impl Fn(u32) -> Option<Option<i64>>,
    tz: &Z,
) -> Option<String>
where
    Z::Offset: Display,
{
    let (head, pid, rest) = label(line)?;
    let tail = rest.strip_prefix(WORKING)?;
    if !counts(tail) {
        return None;
    }
    let until = match lookup(pid)? {
        Some(t) => fmt_time(t, tz),
        None => NO_DEADLINE.to_string(),
    };
    let rewritten = format!("{head}{WAITING}{until}{tail}");
    // An instant the format cannot write as a date is not rewritten, so the
    // result is always a line `is_wait_line` accepts.
    is_wait_line(&rewritten).then_some(rewritten)
}

/// Whether `line` is, whole, the rewritten heartbeat of a waiting step.
///
/// A line that also carries text beyond the heartbeat, a time that is not
/// one, or a usage-limit marker in its label is not a wait line, and so
/// stays in the buffer the classifier reads.
pub(crate) fn is_wait_line(line: &str) -> bool {
    let Some((head, _, rest)) = label(line) else {
        return false;
    };
    let Some(after) = rest.strip_prefix(WAITING) else {
        return false;
    };
    let tail = match after.strip_prefix(NO_DEADLINE) {
        Some(tail) => tail,
        None => {
            let (Some(time), Some(tail)) = (after.get(..TIME_LEN), after.get(TIME_LEN..)) else {
                return false;
            };
            if chrono::DateTime::parse_from_str(time, TIME_FORMAT).is_err() {
                return false;
            }
            tail
        }
    };
    let lowered = head.to_ascii_lowercase();
    counts(tail) && !all_markers().iter().any(|m| lowered.contains(m))
}

/// A heartbeat's opening, the supervisor it is for, and what follows:
/// `(everything up to and including "] ... ", the process id that ends the
/// label, the rest)`.
///
/// After leading spaces or tabs the line must open with bracketed groups
/// separated by single spaces, as in `[21:53:10] [amplihack:claude:41822]`.
/// The label is the last group, and it ends in `:<pid>` with a process id in
/// plain decimal. A group holds no bracket of its own.
fn label(line: &str) -> Option<(&str, u32, &str)> {
    let close = line.find(LEAD)?;
    let rest = &line[close + LEAD.len()..];
    let mut groups = line[..close].trim_start_matches([' ', '\t']);
    let last = loop {
        let inner = groups.strip_prefix('[')?;
        match inner.split_once("] ") {
            // An earlier group, such as the time of day.
            Some((group, more)) if !group.contains(['[', ']']) => groups = more,
            // The last group, whose `]` is the one `LEAD` opens with.
            None if !inner.contains(['[', ']']) => break inner,
            _ => return None,
        }
    };
    let (_, pid) = last.rsplit_once(':')?;
    Some((&line[..close + LEAD.len()], process_id(pid)?, rest))
}

/// The process id `text` writes in plain decimal: digits only, no sign,
/// space or leading zero, and within what a process id can be.
fn process_id(text: &str) -> Option<u32> {
    if text.is_empty() || text.starts_with('0') || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse::<u32>()
        .ok()
        .filter(|p| (1..=MAX_PID).contains(p))
}

/// Whether `tail` is exactly a heartbeat's counts:
/// ` (<n>s elapsed, <n>s since last output)`.
fn counts(tail: &str) -> bool {
    let parsed = || {
        let inner = tail.strip_prefix(" (")?.strip_suffix(')')?;
        let rest = digits(inner)?.strip_prefix("s elapsed, ")?;
        Some(digits(rest)? == "s since last output")
    };
    parsed() == Some(true)
}

/// What follows the leading run of ASCII digits of `s`, which must not be
/// empty.
fn digits(s: &str) -> Option<&str> {
    let n = s.bytes().take_while(u8::is_ascii_digit).count();
    (n > 0).then(|| &s[n..])
}

// --- the pump's side of the wait-state record --------------------------------
//
// The record is kept by `profile::limit_wait::wait_state`, which is built on
// Unix only. These are the three things `recipe run` asks of it, with the
// answer "nothing" on every other host, so the pump itself needs no
// conditional code.

/// The variable that names the wait-state directory to the runner.
#[cfg(unix)]
pub(crate) use wait_state::DIR_ENV as WAIT_DIR_ENV;
/// The wait-state directory of one `recipe run`, removed when it is dropped
/// if this run created it.
#[cfg(unix)]
pub(crate) use wait_state::RunDir as WaitDir;

#[cfg(not(unix))]
pub(crate) const WAIT_DIR_ENV: &str = "AMPLIHACK_USAGE_LIMIT_WAIT_DIR";

/// Without Unix ownership and mode checks no directory is made or trusted.
#[cfg(not(unix))]
pub(crate) struct WaitDir(PathBuf);

#[cfg(not(unix))]
impl WaitDir {
    pub(crate) fn for_run(_run_dir: Option<&Path>) -> Option<Self> {
        None
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

/// The wait-state directory `command` will run with: the value set on the
/// command itself, else the one this process inherited. `None` when the
/// command removes it or neither names one.
pub(crate) fn wait_dir_of(command: &Command) -> Option<PathBuf> {
    let on_command = command
        .get_envs()
        .find(|(key, _)| *key == std::ffi::OsStr::new(WAIT_DIR_ENV))
        .map(|(_, value)| value.map(PathBuf::from));
    match on_command {
        Some(value) => value,
        None => std::env::var_os(WAIT_DIR_ENV).map(PathBuf::from),
    }
    .filter(|p| !p.as_os_str().is_empty())
}

/// Whether the supervisor `pid` is waiting, and until when, by the live
/// records in `dir`: the pump's `lookup` for [`pump`]. `None` with no
/// directory, and on a host that is not Unix.
pub(crate) fn waiting(dir: Option<&Path>, pid: u32) -> Option<Option<i64>> {
    #[cfg(unix)]
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_secs()).ok())?;
        wait_state::waiting(dir?, pid, now)
    }
    #[cfg(not(unix))]
    {
        let _ = (dir, pid);
        None
    }
}
