//! The one place that holds usage-limit patterns (issue #1618).
//!
//! Each agent harness has one table: the markers the recipe runner's
//! classifier matches (lowercased), the line prefix of the epoch form, and
//! the anchor that starts the text form of the reset time. Only Claude Code
//! is described today; Codex and Copilot get their own rows later.
//!
//! The classifier (`failure_class`) and the auto-switch rerun supervisor
//! decide *whether* an attempt hit a limit from the markers. The usage-limit
//! wait is stricter: it needs the harness's own message shape
//! ([`limit_message`]), so model output that merely talks about a usage limit
//! never starts a wait. `epoch_marker` is the auto-switch ledger's reading.

use super::limit_reset;
use chrono_tz::Tz;

/// An agent CLI whose usage-limit messages are recognised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Harness {
    Claude,
}

impl Harness {
    pub(crate) const ALL: &'static [Harness] = &[Harness::Claude];

    /// The name written in log lines.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
        }
    }

    fn patterns(self) -> &'static Patterns {
        match self {
            Harness::Claude => &CLAUDE,
        }
    }
}

struct Patterns {
    /// Lowercase substrings that mean an account usage or session limit.
    markers: &'static [&'static str],
    /// A line starting with this, then `|` and Unix seconds, states the reset.
    epoch_marker: &'static str,
    /// The text form of the reset follows the first occurrence of this.
    anchor: &'static str,
    /// A line starting with this (either apostrophe) that also contains
    /// `limit` and `·` is the harness's own limit message.
    message_prefix: &'static str,
    /// Lowercase prefixes of the harness's own rate-limit (429) envelope,
    /// matched at the start of a line only. See [`rate_limit`].
    rate_limit: &'static [&'static str],
}

/// The organisation spend-limit phrase. `profile usage` also reads it, from
/// Claude's `/usage` screen, so it is named once here.
pub(crate) const SPEND_LIMIT: &str = "spend limit";

/// Claude Code's patterns.
///
/// These are deliberately not transient markers: a 429 or an overload clears
/// in seconds, a usage limit when its stated reset arrives, typically hours
/// away. A bare "rate limit" is absent because it belongs to those
/// short-lived envelopes. "weekly limit" and "spend limit" are listed
/// because "monthly limit" does not match "monthly spend limit" (#1546).
/// "daily limit" is absent: provider and GitHub API daily quotas use it and
/// clear on their own. "hourly limit" is absent for the same reason.
/// "5-hour limit" is the session window's newer name (#2230).
///
/// The per-model caps are matched only in the stopping form "hit your <model>
/// limit", never as a bare "opus limit". Claude Code 2.1.322 names its limit
/// windows in one table (`five_hour` "5-hour limit", `seven_day` "weekly
/// limit", `seven_day_opus` "Opus limit", `seven_day_sonnet` "Sonnet limit",
/// `seven_day_overage_included` "Fable limit") and prints a stop as
/// "You've hit your <name> · resets <time>". The same names also appear in
/// notices that do not stop work, such as "Fable limit reached · continuing
/// on <model>" or a model fallback of the form "Opus limit reached · now
/// using Sonnet"; a bare marker would class those as `usage_limit`, and
/// auto-drive would then stop as USAGE_LIMIT. Both apostrophes match because
/// the marker starts after it. "fast limit" is absent because fast mode falls
/// back to normal mode and does not block the account. A bare "hit your
/// limit" is absent because model prose and tool output also use it; the
/// harness's own messages always name the limit.
/// List order does not pick the reported signal; the classifier reports the
/// marker that appears earliest in the text.
const CLAUDE: Patterns = Patterns {
    markers: &[
        "session limit",
        "5-hour limit",
        "usage limit",
        "usage_limit_reached",
        "quota exceeded",
        "monthly limit",
        "weekly limit",
        "hit your opus limit",
        "hit your sonnet limit",
        "hit your fable limit",
        SPEND_LIMIT,
    ],
    epoch_marker: "Claude AI usage limit reached",
    anchor: "resets ",
    message_prefix: "You've hit your ",
    rate_limit: CLAUDE_RATE_LIMIT,
};

/// The 429 envelopes Claude Code prints: the one definition of the phrases
/// (issue #2857).
///
/// Two readers share it and read it differently. [`rate_limit`] matches these
/// at the start of one line only, for the one decision #2857 names: whether
/// the retry a limit wait earned was rejected at once. The recipe runner's
/// `failure_markers::TRANSIENT_MARKERS` is composed from this list, so a 429
/// anywhere in a failure's evidence is still a transient transport fault
/// there, and in auto-drive's terminal decisions.
///
/// They are never added to [`CLAUDE`]`.markers`: the classifier reads the
/// markers, and a 429 added there would turn every transient rate limit into
/// a usage limit that waits for hours.
pub(super) const CLAUDE_RATE_LIMIT: &[&str] = &[
    "api error: 429",
    "429 too many requests",
    "rate_limit_error",
    "rate limit exceeded",
];

/// Every harness's markers, for the classifier.
pub(crate) fn all_markers() -> Vec<&'static str> {
    Harness::ALL
        .iter()
        .flat_map(|h| h.patterns().markers.iter().copied())
        .collect()
}

/// `None` when no line of stderr or stdout starts with an epoch marker.
/// Otherwise the reset that line states, `None` inside when it states none.
/// The last such line of stderr wins, then the last of stdout. The marker
/// must start the line: `model: <marker>|1` states nothing.
pub(crate) fn epoch_marker(stderr: &[u8], stdout: &[u8]) -> Option<Option<i64>> {
    Harness::ALL.iter().find_map(|h| {
        let marker = h.patterns().epoch_marker.as_bytes();
        let line =
            last_line_starting(stderr, marker).or_else(|| last_line_starting(stdout, marker))?;
        Some(epoch_digits(&line[marker.len()..]))
    })
}

fn last_line_starting<'a>(bytes: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    bytes
        .split(|&b| b == b'\n')
        .rfind(|l| l.starts_with(prefix))
}

fn epoch_digits(rest: &[u8]) -> Option<i64> {
    let digits = rest.strip_prefix(b"|")?;
    let n = digits.iter().take_while(|b| b.is_ascii_digit()).count();
    if n == 0 || n > 19 {
        return None;
    }
    digits[..n].iter().try_fold(0i64, |acc, d| {
        acc.checked_mul(10)?.checked_add(i64::from(d - b'0'))
    })
}

/// A harness's own usage-limit message, as the usage-limit wait reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LimitMessage {
    pub(crate) harness: Harness,
    /// An organisation spend limit. Its line may also say when the session
    /// or weekly limit resets, which `reset` then holds.
    pub(crate) spend: bool,
    /// The reset the message states, not clamped; `None` when it states
    /// none that parses.
    pub(crate) reset: Option<i64>,
}

/// The harness's own limit message in an attempt's output: the last stderr
/// line that has its shape, else the last non-empty line of the stdout tail
/// when that line has it. In `-p` mode stdout is the model's answer, so
/// only a whole line in the message's shape counts there. The shape is a
/// line starting with the epoch marker and `|`, or with the message prefix
/// and also containing `limit` and `·`.
///
/// A reset that states no zone is not read: see [`limit_message_in`], which
/// every production caller uses. This entry point is built for the tests
/// alone, which pin that the zone-required reading is unchanged.
#[cfg(test)]
pub(crate) fn limit_message(stderr: &[u8], stdout: &[u8], now: i64) -> Option<LimitMessage> {
    limit_message_in(stderr, stdout, now, None)
}

/// [`limit_message`], reading a reset that states no zone in `local` (issue
/// #2967): `You've hit your individual spend limit · ... · your 5-hour limit
/// resets 11:50pm`.
///
/// `local` is consulted only for the one line that already has the harness's
/// message shape, so it widens which reset is read and never which line is a
/// limit message. A zone the message states is always the one used.
pub(crate) fn limit_message_in(
    stderr: &[u8],
    stdout: &[u8],
    now: i64,
    local: Option<Tz>,
) -> Option<LimitMessage> {
    read_message(stderr, stdout, now, local).map(|(message, _)| message)
}

/// The reset the harness's own limit message states as a clock time, read
/// as [`limit_message_in`] reads it; `None` when there is no such message,
/// when it states no reset that parses, and when it is the epoch form.
///
/// This is the auto-switch ledger's second reading (issue #2967), after
/// [`epoch_marker`]. The epoch form is left out because the ledger has its
/// own, stricter rule for it: [`limit_message_in`] trims a line before it
/// looks at it, and the ledger takes an epoch only from a line that starts
/// with the marker.
pub(crate) fn clock_reset_in(
    stderr: &[u8],
    stdout: &[u8],
    now: i64,
    local: Option<Tz>,
) -> Option<i64> {
    let (message, form) = read_message(stderr, stdout, now, local)?;
    message.reset.filter(|_| form == Form::Clock)
}

/// How a limit message states its reset.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Form {
    /// `<epoch marker>|<Unix seconds>`.
    Epoch,
    /// A clock time after the anchor, or no reset at all.
    Clock,
}

fn read_message(
    stderr: &[u8],
    stdout: &[u8],
    now: i64,
    local: Option<Tz>,
) -> Option<(LimitMessage, Form)> {
    let err = String::from_utf8_lossy(stderr);
    let out = String::from_utf8_lossy(stdout);
    Harness::ALL.iter().find_map(|&h| {
        let p = h.patterns();
        let line = positioned(&err, &out, |l| shaped(p, l))?;
        let (reset, form) = match line.strip_prefix(p.epoch_marker) {
            Some(rest) => (epoch_digits(rest.as_bytes()), Form::Epoch),
            None => (
                line.to_ascii_lowercase()
                    .find(p.anchor)
                    .and_then(|at| limit_reset::parse_in(&line[at + p.anchor.len()..], now, local)),
                Form::Clock,
            ),
        };
        Some((
            LimitMessage {
                harness: h,
                spend: line.to_ascii_lowercase().contains(SPEND_LIMIT),
                reset,
            },
            form,
        ))
    })
}

/// The one line an attempt's output may be judged by: the last stderr line
/// that `is_it` accepts, else the last non-empty line of the stdout tail when
/// that line is accepted. Lines are trimmed first.
///
/// The position is the whole rule. In `-p` mode stdout is the model's answer,
/// so a phrase inside a sentence, or in a line the model wrote before its
/// closing one, is the model talking about a limit and not the harness
/// reporting one. Shared by [`limit_message`] and [`rate_limit`] so the two
/// readings cannot drift apart.
fn positioned<'a>(err: &'a str, out: &'a str, is_it: impl Fn(&str) -> bool) -> Option<&'a str> {
    err.lines().map(str::trim).rfind(|l| is_it(l)).or_else(|| {
        out.lines()
            .map(str::trim)
            .rfind(|l| !l.is_empty())
            .filter(|l| is_it(l))
    })
}

fn shaped(p: &Patterns, line: &str) -> bool {
    if line
        .strip_prefix(p.epoch_marker)
        .is_some_and(|rest| rest.starts_with('|'))
    {
        return true;
    }
    let curly = p.message_prefix.replace('\'', "\u{2019}");
    (line.starts_with(p.message_prefix) || line.starts_with(curly.as_str()))
        && line.to_ascii_lowercase().contains("limit")
        && line.contains('\u{b7}')
}

/// The reset a usage-limit message states, quoted verbatim for the
/// operator: the rest of the line after the first anchor, in any case.
/// Never parsed into a timestamp, so no zone is invented.
pub(crate) fn reset_hint(text: &str) -> Option<String> {
    let anchor = CLAUDE.anchor;
    let at = text.to_ascii_lowercase().find(anchor)?;
    let hint: String = text[at + anchor.len()..]
        .chars()
        .take_while(|c| *c != '\n' && *c != '\r')
        .collect();
    let hint = hint.trim();
    (!hint.is_empty()).then(|| hint.to_string())
}

/// Whether an attempt's output carries a harness 429 envelope, read at the
/// one position [`limit_message`] reads a usage-limit message at (issue
/// #2857): a whole line of stderr, or the last non-empty line of the stdout
/// tail.
///
/// This answers one question, asked at one place: the retry a limit wait
/// earned came back rejected at once, so the signal that ended that wait was
/// wrong. Nothing else in the runner reads it. A 429 stays what the
/// classifier calls it, a transient transport fault that clears in seconds,
/// and the wait's own reader of limit messages still ignores it.
///
/// The envelope must start the line, in any case: `429 Too Many Requests`
/// counts, `got a 429 Too Many Requests from the proxy` does not. A prefix
/// ending in a digit must not be followed by another, so `API Error: 4290`
/// is not `API Error: 429`.
pub(crate) fn rate_limit(stderr: &[u8], stdout: &[u8]) -> bool {
    let err = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    let out = String::from_utf8_lossy(stdout).to_ascii_lowercase();
    Harness::ALL.iter().any(|h| {
        let p = h.patterns();
        positioned(&err, &out, |l| p.rate_limit.iter().any(|m| envelope(l, m))).is_some()
    })
}

/// Whether the lowercased `line` opens with the lowercase envelope `marker`.
fn envelope(line: &str, marker: &str) -> bool {
    let Some(rest) = line.strip_prefix(marker) else {
        return false;
    };
    !(marker.ends_with(|c: char| c.is_ascii_digit())
        && rest.starts_with(|c: char| c.is_ascii_digit()))
}
