//! The plain-text table: a pure function over a [`Snapshot`].
//!
//! Every outside string is cleaned with [`sanitize`] before it is cut or
//! padded, so its width is its `char` count. Apart from printable ASCII the
//! output uses only the five single-width glyphs below, and every line is
//! cut to [`MAX_WIDTH`] at the end, in release builds too.

/// Widest line printed.
const MAX_WIDTH: usize = 100;
/// Cells in a progress or usage bar.
const BAR_CELLS: usize = 10;
const FULL: char = '█';
const EMPTY: char = '░';
const RUNNING: char = '▶';
const STOPPED: char = '⏸';
const CUT: char = '…';
/// Width of a workstream label.
const LABEL_WIDTH: usize = 24;
/// Width of a login profile name.
const PROFILE_WIDTH: usize = 16;
/// Width of a login state.
const STATE_WIDTH: usize = 8;
/// Longest elapsed time text: `9999d23h`.
const MAX_DAYS: i64 = 9_999;

pub(crate) const NO_IDENTITY_HINT: &str =
    "no session id found; run with --all to see every run on this host";
const LIVE_LEGEND: &str = "* live in ~/.claude";
const API_KEY_TEXT: &str = "API key: billed per token, no plan limits";
const NO_READING: &str = "no reading";

/// What the `PR` column shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrCell {
    /// No PR: `-`.
    None,
    Known(u64),
    /// GitHub could not be asked: `#?`.
    Unknown,
}

/// One workstream row.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WorkRow {
    pub running: bool,
    pub label: String,
    pub issue: Option<u64>,
    pub pr: PrCell,
    pub percent: u8,
    pub elapsed_secs: Option<i64>,
    pub stage: String,
    pub concerns: Option<u64>,
    pub conflicts: bool,
    pub reason: Option<String>,
}

/// What a login row shows.
// `NoReading` names what the row prints: `no reading`.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Reading {
    Bars {
        session: Option<f64>,
        week: Option<f64>,
        state: String,
        resets: Option<String>,
    },
    NoReading {
        state: String,
    },
    ApiKey,
}

/// One login row.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LoginRow {
    pub profile: String,
    /// The login is live in `~/.claude`.
    pub live: bool,
    pub reading: Reading,
}

/// The logins section: rows, or why there are none.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum LoginsSection {
    Rows(Vec<LoginRow>),
    Refused(String),
}

/// Everything the table shows.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Snapshot {
    /// The caller has no identity and `--all` was not given.
    pub no_identity: bool,
    pub workstreams: Vec<WorkRow>,
    /// `None` with `--no-logins`.
    pub logins: Option<LoginsSection>,
}

/// A 10-cell bar for `percent`, clamped to 0..=100; `None` when it is not
/// a finite number.
pub(crate) fn bar(percent: f64) -> Option<String> {
    if !percent.is_finite() {
        return None;
    }
    let filled = (percent.clamp(0.0, 100.0) / 10.0).round() as usize;
    let filled = filled.min(BAR_CELLS);
    let mut out = String::new();
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

/// `2h05m` under a day, `3d04h` from a day up, `-` when unknown. Negative
/// values count as 0.
pub(crate) fn elapsed(secs: Option<i64>) -> String {
    let Some(secs) = secs else {
        return "-".to_string();
    };
    let secs = secs.max(0);
    let (days, hours, minutes) = (secs / 86_400, secs % 86_400 / 3_600, secs % 3_600 / 60);
    if days == 0 {
        format!("{hours}h{minutes:02}m")
    } else if days > MAX_DAYS {
        format!(">{MAX_DAYS}d")
    } else {
        format!("{days}d{hours:02}h")
    }
}

/// `s` padded with spaces to `width` chars.
fn pad(s: &str, width: usize) -> String {
    let mut out = s.to_string();
    out.extend(std::iter::repeat_n(
        ' ',
        width.saturating_sub(s.chars().count()),
    ));
    out
}

/// The whole table.
pub(crate) fn render(snapshot: &Snapshot) -> String {
    let mut lines = vec!["WORKSTREAMS".to_string()];
    if snapshot.no_identity {
        lines.push(format!("  {NO_IDENTITY_HINT}"));
    }
    if snapshot.workstreams.is_empty() {
        lines.push("  none".to_string());
    } else {
        work_lines(&snapshot.workstreams, &mut lines);
    }
    if let Some(logins) = &snapshot.logins {
        lines.push(String::new());
        lines.push("LOGINS".to_string());
        logins_lines(logins, &mut lines);
    }
    let mut out = String::new();
    for line in lines {
        out.push_str(clip(line.trim_end(), MAX_WIDTH).trim_end());
        out.push('\n');
    }
    out
}

/// The cells of one workstream row, cleaned and cut.
struct WorkCells {
    marker: char,
    label: String,
    issue: String,
    pr: String,
    bar: String,
    percent: String,
    elapsed: String,
    stage: String,
}

/// Columns are as wide as their widest cell and one space apart, so the
/// stage keeps as much of the 100 columns as the other cells leave.
fn work_lines(rows: &[WorkRow], lines: &mut Vec<String>) {
    let cells: Vec<WorkCells> = rows.iter().map(work_cells).collect();
    let width = |f: fn(&WorkCells) -> &str| {
        cells
            .iter()
            .map(|c| f(c).chars().count())
            .max()
            .unwrap_or(0)
    };
    let (label_w, issue_w, pr_w) = (width(|c| &c.label), width(|c| &c.issue), width(|c| &c.pr));
    let (percent_w, elapsed_w) = (width(|c| &c.percent), width(|c| &c.elapsed));
    for c in &cells {
        let head = format!(
            "{} {} {} {} {} {}{} {} ",
            c.marker,
            pad(&c.label, label_w),
            pad(&c.issue, issue_w),
            pad(&c.pr, pr_w),
            c.bar,
            " ".repeat(percent_w.saturating_sub(c.percent.chars().count())),
            c.percent,
            pad(&c.elapsed, elapsed_w),
        );
        let room = MAX_WIDTH.saturating_sub(head.chars().count());
        lines.push(format!("{head}{}", clip(&c.stage, room)));
    }
}

fn work_cells(row: &WorkRow) -> WorkCells {
    let percent = row.percent.min(100);
    let mut stage = vec![sanitize(&row.stage)];
    if let Some(n) = row.concerns {
        stage.push(if n == 1 {
            "1 concern".to_string()
        } else {
            format!("{n} concerns")
        });
    }
    if row.conflicts {
        stage.push("conflicts".to_string());
    }
    if let Some(reason) = &row.reason {
        stage.push(sanitize(reason));
    }
    WorkCells {
        marker: if row.running { RUNNING } else { STOPPED },
        label: clip(&sanitize(&row.label), LABEL_WIDTH),
        issue: row.issue.map_or("-".to_string(), |n| format!("#{n}")),
        pr: match row.pr {
            PrCell::None => "-".to_string(),
            PrCell::Known(n) => format!("#{n}"),
            PrCell::Unknown => "#?".to_string(),
        },
        bar: bar(f64::from(percent)).unwrap_or_default(),
        percent: format!("{percent}%"),
        elapsed: elapsed(row.elapsed_secs),
        stage: stage.join(", "),
    }
}

fn logins_lines(section: &LoginsSection, lines: &mut Vec<String>) {
    let rows = match section {
        LoginsSection::Refused(reason) => {
            lines.push(format!("  {}", sanitize(reason)));
            return;
        }
        LoginsSection::Rows(rows) if rows.is_empty() => {
            lines.push("  none".to_string());
            return;
        }
        LoginsSection::Rows(rows) => rows,
    };
    lines.push(format!(
        "  {} {}{}{} RESETS",
        pad("PROFILE", PROFILE_WIDTH),
        pad("SESSION", 18),
        pad("WEEK", 18),
        pad("STATE", STATE_WIDTH),
    ));
    for row in rows {
        let mut name = clip(&sanitize(&row.profile), PROFILE_WIDTH - 1);
        if row.live {
            name.push('*');
        }
        let head = format!("  {} ", pad(&name, PROFILE_WIDTH));
        let rest = match &row.reading {
            Reading::ApiKey => API_KEY_TEXT.to_string(),
            Reading::NoReading { state } => format!(
                "{}{}",
                pad(NO_READING, 36),
                clip(&sanitize(state), STATE_WIDTH)
            ),
            Reading::Bars {
                session,
                week,
                state,
                resets,
            } => format!(
                "{}{}{} {}",
                usage_cell(*session),
                usage_cell(*week),
                pad(&clip(&sanitize(state), STATE_WIDTH), STATE_WIDTH),
                resets.as_deref().map_or("-".to_string(), sanitize)
            ),
        };
        lines.push(format!("{head}{rest}"));
    }
    if rows.iter().any(|r| r.live) {
        lines.push(LIVE_LEGEND.to_string());
    }
}

/// A bar and its percentage in 18 columns; `-` when there is no number.
fn usage_cell(percent: Option<f64>) -> String {
    match percent.and_then(|p| bar(p).map(|b| (b, p))) {
        Some((b, p)) => pad(&format!("{b} {:>4}", format!("{:.0}%", p)), 18),
        None => pad("-", 18),
    }
}
