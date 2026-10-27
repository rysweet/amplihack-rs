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
/// clear on their own. List order does not pick the reported signal; the
/// classifier reports the marker that appears earliest in the text.
const CLAUDE: Patterns = Patterns {
    markers: &[
        "session limit",
        "usage limit",
        "usage_limit_reached",
        "quota exceeded",
        "monthly limit",
        "weekly limit",
        SPEND_LIMIT,
    ],
    epoch_marker: "Claude AI usage limit reached",
    anchor: "resets ",
    message_prefix: "You've hit your ",
};

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
pub(crate) fn limit_message(stderr: &[u8], stdout: &[u8], now: i64) -> Option<LimitMessage> {
    let err = String::from_utf8_lossy(stderr);
    let out = String::from_utf8_lossy(stdout);
    Harness::ALL.iter().find_map(|&h| {
        let p = h.patterns();
        let line = err
            .lines()
            .map(str::trim)
            .rfind(|l| shaped(p, l))
            .or_else(|| {
                out.lines()
                    .map(str::trim)
                    .rfind(|l| !l.is_empty())
                    .filter(|l| shaped(p, l))
            })?;
        let reset = match line.strip_prefix(p.epoch_marker) {
            Some(rest) => epoch_digits(rest.as_bytes()),
            None => line
                .to_ascii_lowercase()
                .find(p.anchor)
                .and_then(|at| limit_reset::parse(&line[at + p.anchor.len()..], now)),
        };
        Some(LimitMessage {
            harness: h,
            spend: line.to_ascii_lowercase().contains(SPEND_LIMIT),
            reset,
        })
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
