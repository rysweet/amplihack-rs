//! The plain-text table (issue #1710): at most 100 columns, no colour codes,
//! every external string sanitised and fitted to its column. Pure.
//!
//! Workstream row columns (each width includes its trailing space):
//! indent 2, label 28, issue 6, PR 6, bar 20, percent 5, marker 3, stage 30.

use chrono::{DateTime, FixedOffset};

use super::logins::{LoginCell, LoginRow};
use super::stage::Status;
use crate::commands::profile::usage_cmd::sanitize_display;

pub(crate) const WIDTH: usize = 100;
const INDENT: &str = "  ";
const LABEL_W: usize = 28;
const NUM_W: usize = 6;
const BAR_W: usize = 20;
const STAGE_W: usize = 30;
const PROFILE_W: usize = 18;
const LOGIN_BAR_W: usize = 10;
const LIVE: &str = " (live)";

/// One workstream row.
#[derive(Debug, Clone)]
pub(crate) struct WorkRow {
    pub label: String,
    pub issue: Option<u64>,
    pub pr: Option<u64>,
    pub status: Status,
}

#[derive(Debug, Clone)]
pub(crate) struct Report {
    pub now: DateTime<FixedOffset>,
    /// Shown in place of the workstream table.
    pub note: Option<String>,
    pub rows: Vec<WorkRow>,
    /// `None` with `--no-logins`.
    pub logins: Option<Vec<LoginRow>>,
}

/// `cells` cells, one filled per `100 / cells` percent, `pct` clamped to 100.
pub(crate) fn bar(pct: u8, cells: usize) -> String {
    let filled = usize::from(pct.min(100)) * cells / 100;
    format!("{}{}", "█".repeat(filled), "░".repeat(cells - filled))
}

/// `s` padded to `w` characters, or cut to `w - 1` characters and `…`.
pub(crate) fn fit(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n <= w {
        return format!("{s}{}", " ".repeat(w - n));
    }
    if w == 0 {
        return String::new();
    }
    let mut out: String = s.chars().take(w - 1).collect();
    out.push('…');
    out
}

fn clean(s: &str) -> String {
    sanitize_display(s, false)
}

fn rule() -> String {
    format!("{INDENT}{}", "─".repeat(WIDTH - INDENT.len()))
}

/// `s` in a column of `w` characters whose last one is always a space, so
/// a cut value never runs into the next column.
fn cell(s: &str, w: usize) -> String {
    format!("{} ", fit(s, w - 1))
}

fn number(n: Option<u64>) -> String {
    cell(
        &n.map_or_else(|| "-".to_string(), |n| format!("#{n}")),
        NUM_W,
    )
}

pub(crate) fn render(r: &Report) -> String {
    let mut lines: Vec<String> = vec![format!("WORKSTREAMS  {}", r.now.format("%Y-%m-%d %H:%M"))];
    if let Some(note) = &r.note {
        lines.push(format!("{INDENT}{}", clean(note)));
    } else {
        lines.push(format!(
            "{INDENT}{}{}{}{}{}",
            fit("WORKSTREAM", LABEL_W),
            fit("ISSUE", NUM_W),
            fit("PR", NUM_W),
            fit("PROGRESS", BAR_W + 5 + 3),
            "STAGE"
        ));
        lines.push(rule());
        if r.rows.is_empty() {
            lines.push(format!("{INDENT}(no workstreams)"));
        }
        for row in &r.rows {
            let s = &row.status;
            lines.push(format!(
                "{INDENT}{}{}{}{}{:>4}% {} {}",
                cell(&clean(&row.label), LABEL_W),
                number(row.issue),
                number(row.pr),
                bar(s.pct, BAR_W),
                s.pct.min(100),
                s.marker.glyph(),
                fit(&clean(&s.text), STAGE_W),
            ));
        }
    }
    if let Some(logins) = &r.logins {
        lines.push(String::new());
        lines.push("LOGINS".to_string());
        lines.push(format!(
            "{INDENT}{}{}",
            fit("PROFILE", PROFILE_W),
            "SESSION           WEEK              STATE, NEXT RESET"
        ));
        lines.push(rule());
        if logins.is_empty() {
            lines.push(format!("{INDENT}(no profiles)"));
        }
        for l in logins {
            lines.push(login_line(l));
        }
    }
    let mut out = String::new();
    for line in lines {
        out.push_str(&finish(&line));
        out.push('\n');
    }
    out
}

fn login_line(l: &LoginRow) -> String {
    let name = clean(&l.profile);
    let name = if l.live {
        let keep = PROFILE_W - 1 - LIVE.chars().count();
        let short = if name.chars().count() > keep {
            fit(&name, keep)
        } else {
            name
        };
        format!("{short}{LIVE}")
    } else {
        name
    };
    let head = format!("{INDENT}{}", cell(&name, PROFILE_W));
    let rest = match &l.cell {
        LoginCell::Reading {
            session,
            week,
            state,
            reset,
        } => {
            let meter = |tag: &str, p: &Option<u8>| match p {
                Some(p) => format!("{tag} {}{:>4}%", bar(*p, LOGIN_BAR_W), p),
                None => format!("{tag} {}{:>5}", " ".repeat(LOGIN_BAR_W), "-"),
            };
            let tail = match reset {
                Some(reset) => format!("{}, {}", clean(state), clean(reset)),
                None => clean(state),
            };
            format!("{} {} {tail}", meter("S", session), meter("W", week))
        }
        LoginCell::NoReading(state) => format!("no reading ({})", clean(state)),
        LoginCell::ApiKey => "API key, billed per token (no plan limits)".to_string(),
    };
    let room = WIDTH - head.chars().count();
    format!("{head}{}", fit(&rest, room))
}

/// The last pass over every line: no control character, no trailing spaces,
/// at most [`WIDTH`] characters.
fn finish(line: &str) -> String {
    let clean = clean(line);
    let trimmed = clean.trim_end();
    if trimmed.chars().count() <= WIDTH {
        trimmed.to_string()
    } else {
        fit(trimmed, WIDTH)
    }
}
