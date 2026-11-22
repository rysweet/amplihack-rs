//! The one place that holds usage-limit patterns (issue #1618).
//!
//! Each agent harness has one table: the markers the recipe runner's
//! classifier matches (lowercased), and whichever message shapes that
//! harness prints. Claude Code states a reset, in an epoch form and a text
//! form; Codex states none and prints one whole line of its own. Every shape
//! field is optional, so a row carries only the shapes its harness has and a
//! reader skips a row that has none (issue #2911). Copilot gets its own row
//! later.
//!
//! The classifier (`failure_class`) and the auto-switch rerun supervisor
//! decide *whether* an attempt hit a limit from the markers. The usage-limit
//! wait is stricter: it needs the harness's own message shape
//! ([`limit_message`]), so model output that merely talks about a usage limit
//! never starts a wait. `epoch_marker` is the auto-switch ledger's reading.

use super::limit_reset;

/// An agent CLI whose usage-limit messages are recognised.
///
/// The order of [`Harness::ALL`] is the order every reader here tries the
/// rows in, and Claude comes first: output carrying both harnesses' messages
/// is reported by Claude's, which is the one that states a reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Harness {
    Claude,
    /// OpenAI Codex. It states no reset and has no epoch form; its one shape
    /// is the whole-line message in [`CODEX`] (issue #2911).
    Codex,
}

impl Harness {
    pub(crate) const ALL: &'static [Harness] = &[Harness::Claude, Harness::Codex];

    /// The name written in log lines.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
        }
    }

    fn patterns(self) -> &'static Patterns {
        match self {
            Harness::Claude => &CLAUDE,
            Harness::Codex => &CODEX,
        }
    }
}

struct Patterns {
    /// Lowercase substrings that mean an account usage or session limit.
    markers: &'static [&'static str],
    /// A line starting with this, then `|` and Unix seconds, states the
    /// reset. `None` for a harness with no epoch form.
    ///
    /// It is an [`Option`] and not an empty string because
    /// [`last_line_starting`] accepts an empty prefix on every line: an
    /// empty marker would make [`epoch_marker`] answer `Some(None)` for any
    /// output at all, and the auto-switch ledger would then record a reading
    /// no harness made (issue #2911).
    epoch_marker: Option<&'static str>,
    /// The text form of the reset follows the first occurrence of this.
    /// `None` for a harness whose message states no reset.
    anchor: Option<&'static str>,
    /// A line starting with this (either apostrophe) that also contains
    /// `limit` and the middle dot is the harness's own limit message. `None`
    /// for a harness that prints no such line.
    message_prefix: Option<&'static str>,
    /// A lowercase whole-line message: the harness's own limit line opens
    /// with this phrase and continues, if at all, past a delimiter. `None`
    /// for a harness whose message has a prefix shape instead. See
    /// [`whole_line`].
    whole_line: Option<&'static str>,
    /// Lowercase prefixes of the harness's own rate-limit (429) envelope,
    /// matched at the start of a line only. See [`rate_limit`].
    rate_limit: &'static [&'static str],
}

/// The organisation spend-limit phrase. `profile usage` also reads it, from
/// Claude's `/usage` screen, so it is named once here.
pub(crate) const SPEND_LIMIT: &str = "spend limit";

/// The seven-day window's phrase, named once for the same reason.
pub(crate) const WEEKLY_LIMIT: &str = "weekly limit";

/// Codex's own words when a workspace has no credits left (issue #2911). It
/// states no reset, so a step that meets it waits for capacity rather than
/// for a time.
pub(crate) const CODEX_CREDITS: &str = "your workspace is out of credits";

/// The line prefix of Claude Code's epoch form.
const EPOCH_MARKER: &str = "Claude AI usage limit reached";

/// The anchor the text form of a reset follows, in any case.
const RESETS: &str = "resets ";

/// Which window or budget a limit message names (issue #2911).
///
/// A closed set read from the message's own words and never a copy of them.
/// It is what `rotate::step_block` records for the login a step ran under,
/// and what the wait names when it says why it is waiting, so no harness
/// text reaches a log line or the rotate ledger through it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LimitKind {
    /// The rolling session window ("session limit", "5-hour limit"), and the
    /// fallback for wording this table does not recognise: it is the
    /// shortest window, so a malformed or forged message buys the least.
    Session,
    /// The seven-day window.
    Weekly,
    /// An organisation or individual spend limit. No usage window's reset
    /// lifts it, so it takes precedence over any window the same line names.
    Spend,
    /// Codex workspace credits, which state no reset at all.
    Credits,
}

impl LimitKind {
    /// The words a log line names this kind with: fixed text, one per
    /// variant. `Spend`'s wording is what 1.2.193 wrote, so the spend ALERT
    /// line is unchanged byte for byte.
    pub(crate) fn noun(self) -> &'static str {
        match self {
            LimitKind::Session => "session limit",
            LimitKind::Weekly => "weekly limit",
            LimitKind::Spend => "spend limit",
            LimitKind::Credits => "credit limit",
        }
    }

    /// The kind `line` states. `whole` is the harness's whole-line shape,
    /// which names its own kind outright. Otherwise the words decide, and a
    /// spend limit wins over any window the same line names, because no
    /// window reset lifts one.
    fn of(line: &str, whole: Option<&str>) -> Self {
        if whole.is_some() {
            return LimitKind::Credits;
        }
        let lower = line.to_ascii_lowercase();
        if lower.contains(SPEND_LIMIT) {
            LimitKind::Spend
        } else if lower.contains(WEEKLY_LIMIT) {
            LimitKind::Weekly
        } else {
            LimitKind::Session
        }
    }
}

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
        WEEKLY_LIMIT,
        "hit your opus limit",
        "hit your sonnet limit",
        "hit your fable limit",
        SPEND_LIMIT,
    ],
    epoch_marker: Some(EPOCH_MARKER),
    anchor: Some(RESETS),
    message_prefix: Some("You've hit your "),
    whole_line: None,
    rate_limit: CLAUDE_RATE_LIMIT,
};

/// Codex's patterns (issue #2911).
///
/// Codex carries neither shape Claude's row carries: its message has no
/// middle dot and does not open with "You've hit your ", so [`shaped`] would
/// never match it, and it states no reset to read. [`whole_line`] is the
/// shape it does have. `rate_limit` is empty because nothing reads Codex's
/// own 429 envelope yet, and an empty list matches nothing.
///
/// Over the 72 hours to 21 November 2026, 36 runs ended as `indeterminate`
/// on this message and were reported as authentication rejections (#2876).
/// It is a usage limit, so it classifies as one and waits like one.
const CODEX: Patterns = Patterns {
    markers: &[CODEX_CREDITS],
    epoch_marker: None,
    anchor: None,
    message_prefix: None,
    whole_line: Some(CODEX_CREDITS),
    rate_limit: &[],
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
        // A row with no epoch form is skipped whole: an empty prefix would
        // match every line and claim a reset no harness stated (#2911).
        let marker = h.patterns().epoch_marker?.as_bytes();
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
    /// Which window or budget the message names (issue #2911). A spend
    /// limit's line may also say when the session or weekly window resets,
    /// which `reset` then holds, and the kind stays [`LimitKind::Spend`].
    pub(crate) kind: LimitKind,
    /// The reset the message states, not clamped; `None` when it states
    /// none that parses.
    pub(crate) reset: Option<i64>,
}

impl LimitMessage {
    /// Whether this is a spend limit, which no usage window's reset lifts.
    /// Derived from [`Self::kind`] rather than stored beside it, so the two
    /// cannot disagree.
    pub(crate) fn spend(self) -> bool {
        self.kind == LimitKind::Spend
    }
}

/// The harness's own limit message in an attempt's output: the last stderr
/// line that has its shape, else the last non-empty line of the stdout tail
/// when that line has it. In `-p` mode stdout is the model's answer, so
/// only a whole line in the message's shape counts there. The shape is a
/// line starting with the epoch marker and `|`, a line opening with the
/// harness's whole-line message, or one starting with the message prefix and
/// also containing `limit` and the middle dot.
pub(crate) fn limit_message(stderr: &[u8], stdout: &[u8], now: i64) -> Option<LimitMessage> {
    let err = String::from_utf8_lossy(stderr);
    let out = String::from_utf8_lossy(stdout);
    Harness::ALL.iter().find_map(|&h| {
        let p = h.patterns();
        let line = positioned(&err, &out, |l| shaped(p, l))?;
        let epoch = p
            .epoch_marker
            .and_then(|m| line.strip_prefix(m))
            .and_then(|rest| epoch_digits(rest.as_bytes()));
        let reset = match epoch {
            Some(r) => Some(r),
            None => p.anchor.and_then(|anchor| {
                line.to_ascii_lowercase()
                    .find(anchor)
                    .and_then(|at| limit_reset::parse(&line[at + anchor.len()..], now))
            }),
        };
        Some(LimitMessage {
            harness: h,
            kind: LimitKind::of(line, p.whole_line),
            reset,
        })
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
    if p.epoch_marker
        .and_then(|m| line.strip_prefix(m))
        .is_some_and(|rest| rest.starts_with('|'))
    {
        return true;
    }
    if p.whole_line.is_some_and(|phrase| whole_line(line, phrase)) {
        return true;
    }
    let Some(prefix) = p.message_prefix else {
        return false;
    };
    let curly = prefix.replace('\'', "\u{2019}");
    (line.starts_with(prefix) || line.starts_with(curly.as_str()))
        && line.to_ascii_lowercase().contains("limit")
        && line.contains('\u{b7}')
}

/// Whether `line` is the whole-line message `phrase` (issue #2911).
///
/// The line, already trimmed by [`positioned`], must *open* with the phrase,
/// and what follows must be a delimiter rather than more of a word: the
/// harness prints its own sentence after it ("... out of credits. Add
/// credits to continue."), while a model writing about the phrase puts words
/// in front of it. A bare `contains` would match the phrase anywhere in a
/// sentence, which is the weaker rule this deliberately is not. The
/// comparison is case-insensitive, as every other reading here is.
fn whole_line(line: &str, phrase: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    let Some(rest) = lower.strip_prefix(phrase) else {
        return false;
    };
    rest.chars().next().is_none_or(|c| !c.is_alphanumeric())
}

/// The reset a usage-limit message states, quoted verbatim for the
/// operator: the rest of the line after the first anchor, in any case.
/// Never parsed into a timestamp, so no zone is invented.
pub(crate) fn reset_hint(text: &str) -> Option<String> {
    let at = text.to_ascii_lowercase().find(RESETS)?;
    let hint: String = text[at + RESETS.len()..]
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
