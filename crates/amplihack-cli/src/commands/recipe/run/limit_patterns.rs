//! The one place that holds usage-limit patterns (issue #1618).
//!
//! Each agent harness has one table: the markers the recipe runner's
//! classifier matches (lowercased), the line prefix of the epoch form, and
//! the anchor that starts the text form of the reset time. Only Claude Code
//! is described today; Codex and Copilot get their own rows later.
//!
//! Two readers use the tables. The classifier (`failure_class`) and the
//! rerun supervisor decide *whether* an attempt hit a limit; the readers here
//! only say *when* it resets. `epoch_reset` is the auto-switch ledger's
//! reading (unclamped); `read_reset` is the usage-limit wait's (capped at now
//! plus five hours, never raised to now).

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
}

/// The organisation spend-limit phrase. `profile usage` also reads it, from
/// Claude's `/usage` screen, so it is named once here.
pub(crate) const SPEND_LIMIT: &str = "spend limit";

/// Claude's out-of-credit message. Also an environmental marker, which the
/// classifier checks first, so it never starts a rerun or a wait.
pub(crate) const CREDIT_BALANCE_LOW: &str = "credit balance is too low";

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
        CREDIT_BALANCE_LOW,
    ],
    epoch_marker: "Claude AI usage limit reached",
    anchor: "resets ",
};

/// The harness whose marker appears in `text`, matched case-insensitively.
pub(crate) fn detect(text: &str) -> Option<Harness> {
    let lower = text.to_ascii_lowercase();
    Harness::ALL
        .iter()
        .copied()
        .find(|h| h.patterns().markers.iter().any(|m| lower.contains(m)))
}

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

/// The reset an epoch marker line states: 1 to 19 ASCII digits right after
/// `|`, which must fit in an `i64`. Not clamped.
pub(crate) fn epoch_reset(stderr: &[u8], stdout: &[u8]) -> Option<i64> {
    epoch_marker(stderr, stdout).flatten()
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

/// The reset the usage-limit wait uses: the epoch form, else the text form
/// after the first anchor (stderr, then stdout). Capped at `now` plus five
/// hours; a reset in the past is kept as read. `None` means unparsed.
pub(crate) fn read_reset(stderr: &[u8], stdout: &[u8], now: i64) -> Option<i64> {
    const FIVE_HOURS: i64 = 5 * 3600;
    let reset = epoch_reset(stderr, stdout).or_else(|| {
        [stderr, stdout]
            .into_iter()
            .find_map(|bytes| text_reset(&String::from_utf8_lossy(bytes), now))
    })?;
    Some(reset.min(now.saturating_add(FIVE_HOURS)))
}

fn text_reset(text: &str, now: i64) -> Option<i64> {
    Harness::ALL.iter().find_map(|h| {
        let anchor = h.patterns().anchor;
        let at = text.to_ascii_lowercase().find(anchor)?;
        limit_reset::parse(&text[at + anchor.len()..], now)
    })
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
