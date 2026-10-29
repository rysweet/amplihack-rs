//! The bars of `workstreams status` (issue #1710): the 20-cell progress
//! bar of each workstream and the logins section, one row per login with
//! its session and weekly usage, one cell per 5%.
//!
//! This is the only bar code in the tree. `amplihack profile usage` prints
//! a table and has no bars yet: #1739, which asks for its bar view, is an
//! open issue with no PR. Its target rows are the ones drawn here. The login
//! rows come from `amplihack profile usage --json` (see `logins.rs`), so when
//! #1739 lands, its renderer can replace this module without touching how
//! the rows are read; #1749 tracks that change and the removal of the
//! login bar code here.
//!
//! Pure: no I/O. Every outside string is cleaned with [`sanitize`] before it
//! is cut or padded, so its width is its `char` count. Lines are cut to
//! [`MAX_WIDTH`]: a target row in #1710 runs to 116 columns (a 27-column
//! name, two bars and a reset note).

/// Cells in a bar: one per 5%.
pub(crate) const BAR_CELLS: usize = 20;
/// Widest line printed.
pub(crate) const MAX_WIDTH: usize = 120;
const FULL: char = '█';
const EMPTY: char = '░';
const CUT: char = '…';
/// The login column, with the space after it.
const NAME_COLUMN: usize = 27;
/// Narrowest percentage: `100%`.
const MIN_PCT_WIDTH: usize = 4;
const LIVE_SUFFIX: &str = " (live)";
const API_KEY_TEXT: &str = "API key, billed per token (no plan limits)";
const OVERFLOW_TEXT: &str = "overflow, paid credits (no plan limits)";

/// What a login row shows.
// `NoReading` names what the row prints: `no reading (<state>)`.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Reading {
    Bars {
        session: Option<f64>,
        week: Option<f64>,
        /// The `state` of `profile usage --json`: `ok`, `session_exhausted`...
        state: String,
        /// Reset text with the trailing `(Zone/Name)` dropped.
        session_resets: Option<String>,
        week_resets: Option<String>,
    },
    NoReading {
        state: String,
    },
    ApiKey,
    /// A credit-backed overflow profile (#1695): its plan numbers do not
    /// limit it, so no bars.
    Overflow,
}

/// One login row.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LoginRow {
    pub profile: String,
    /// The login is live in `~/.claude`: printed as `(live)`.
    pub live: bool,
    pub reading: Reading,
}

/// A bar for `percent`, clamped to 0..=100: one full cell per whole 5%,
/// rounded down, so 29% is 5 cells, 43% is 8 and only 100% fills the bar
/// (the rows in #1710 and #1739 are drawn this way). `None` when it is not
/// a finite number.
pub(crate) fn bar(percent: f64) -> Option<String> {
    if !percent.is_finite() {
        return None;
    }
    let step = 100.0 / BAR_CELLS as f64;
    let filled = ((percent.clamp(0.0, 100.0) / step).floor() as usize).min(BAR_CELLS);
    let mut out = String::with_capacity(BAR_CELLS * FULL.len_utf8());
    out.extend(std::iter::repeat_n(FULL, filled));
    out.extend(std::iter::repeat_n(EMPTY, BAR_CELLS - filled));
    Some(out)
}

/// `s` with every char outside printable ASCII replaced by `?`.
pub(crate) fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if (' '..='~').contains(&c) { c } else { '?' })
        .collect()
}

/// `s` when it has at most `max` chars, else its first `max - 1` and `…`.
pub(crate) fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push(CUT);
    out
}

/// `s` padded with spaces to `width` chars.
pub(crate) fn pad(s: &str, width: usize) -> String {
    let mut out = s.to_string();
    out.extend(std::iter::repeat_n(
        ' ',
        width.saturating_sub(s.chars().count()),
    ));
    out
}

/// `s` padded on the left with spaces to `width` chars.
pub(crate) fn pad_left(s: &str, width: usize) -> String {
    let mut out = " ".repeat(width.saturating_sub(s.chars().count()));
    out.push_str(s);
    out
}

/// `4:50pm (Europe/London)` without ` (Europe/London)`.
pub(crate) fn without_zone(text: &str) -> String {
    let text = text.trim();
    match text.rfind(" (") {
        Some(at) if text.ends_with(')') => text[..at].trim_end().to_string(),
        _ => text.to_string(),
    }
}

/// The heading line and one line per row, each starting with two spaces
/// and with no trailing space:
///
/// ```text
///   LOGIN                      SESSION                   WEEK
///   hharkness (live)           ░░░░░░░░░░░░░░░░░░░░   0% █████░░░░░░░░░░░░░░░  29%
///   rssourcelabs               -                         ████████████████████ 100%  week out, resets Nov 2, 10am
/// ```
pub(crate) fn render_login_lines(rows: &[LoginRow]) -> Vec<String> {
    let pct_width = rows
        .iter()
        .flat_map(|r| match &r.reading {
            Reading::Bars { session, week, .. } => vec![*session, *week],
            _ => Vec::new(),
        })
        .flatten()
        .filter(|p| p.is_finite())
        .map(|p| format_pct(p).chars().count())
        .fold(MIN_PCT_WIDTH, usize::max);
    // A bar cell and the space after it.
    let cell_column = BAR_CELLS + 1 + pct_width + 1;
    let mut lines = vec![format!(
        "  {}{}WEEK",
        pad("LOGIN", NAME_COLUMN),
        pad("SESSION", cell_column)
    )];
    for row in rows {
        let suffix = if row.live { LIVE_SUFFIX.len() } else { 0 };
        let mut name = clip(&sanitize(&row.profile), NAME_COLUMN - 1 - suffix);
        if row.live {
            name.push_str(LIVE_SUFFIX);
        }
        let rest = match &row.reading {
            Reading::ApiKey => API_KEY_TEXT.to_string(),
            Reading::Overflow => OVERFLOW_TEXT.to_string(),
            Reading::NoReading { state } => format!("no reading ({})", sanitize(state)),
            Reading::Bars {
                session,
                week,
                state,
                session_resets,
                week_resets,
            } => {
                let weekly_out = state == "weekly_exhausted";
                let session = if weekly_out { None } else { *session };
                let mut text = format!(
                    "{} {}",
                    usage_cell(session, pct_width),
                    usage_cell(*week, pct_width)
                );
                if let Some(note) = note(state, session_resets, week_resets) {
                    text.push_str("  ");
                    text.push_str(&note);
                }
                text
            }
        };
        lines.push(format!("  {}{rest}", pad(&name, NAME_COLUMN)));
    }
    lines
        .into_iter()
        .map(|l| clip(l.trim_end(), MAX_WIDTH).trim_end().to_string())
        .collect()
}

/// A bar and its percentage, or `-` padded to the same width.
fn usage_cell(percent: Option<f64>, pct_width: usize) -> String {
    match percent.and_then(|p| bar(p).map(|b| (b, p))) {
        Some((b, p)) => format!("{b} {}", pad_left(&format_pct(p), pct_width)),
        None => pad("-", BAR_CELLS + 1 + pct_width),
    }
}

/// What follows the bars: which window is out and when it resets, or the
/// state when it is neither `ok` nor an exhausted window.
fn note(
    state: &str,
    session_resets: &Option<String>,
    week_resets: &Option<String>,
) -> Option<String> {
    let out = |window: &str, resets: &Option<String>| match resets {
        Some(at) => format!("{window} out, resets {}", sanitize(at)),
        None => format!("{window} out"),
    };
    match state {
        "ok" => None,
        "session_exhausted" => Some(out("session", session_resets)),
        "weekly_exhausted" => Some(out("week", week_resets)),
        other => Some(sanitize(other).replace('_', " ")),
    }
}

/// A percentage as a whole number when whole, otherwise to one decimal,
/// as `profile usage` prints it.
pub(crate) fn format_pct(p: f64) -> String {
    if p.fract() == 0.0 {
        format!("{p:.0}%")
    } else {
        format!("{p:.1}%")
    }
}
