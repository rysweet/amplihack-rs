//! The plain-text report: a pure function over a [`Snapshot`], in the
//! layout the owner set in the 2026-10-29T19:09:55Z comment on issue #1710:
//!
//! ```text
//!   amplihack-rs workstreams — Thu 11:56 PDT
//!
//!   WORKSTREAM                     ISSUE  PR     PROGRESS                   STAGE
//!   Credit-backed overflow         #1694  #1695  ████████████████████ 100%  merged
//!   API-key profiles (last resort) #1700  #1727  ██████░░░░░░░░░░░░░░  30%  ▶ PR open, conflicts with main
//!   Usage probe timeouts           #1698  -      ███░░░░░░░░░░░░░░░░░  15%  ⏸ build stopped on a usage limit
//!
//!   LOGIN                      SESSION                   WEEK
//!   hharkness (live)           ░░░░░░░░░░░░░░░░░░░░   0% █████░░░░░░░░░░░░░░░  29%
//! ```
//!
//! Each heading sits over the first character of its column. (In the
//! comment, `STAGE` and `WEEK` sit one or two columns right of their data.)
//!
//! Bars, the logins section and text cleaning come from `bars.rs`, the one
//! copy of the bar code (see its module comment on #1739). Every outside
//! string is cleaned with [`sanitize`] before it is cut
//! or padded. Apart from printable ASCII the output uses only `█ ░ ▶ ⏸ … —`,
//! and every line is cut to [`MAX_WIDTH`].

use super::bars::{self, MAX_WIDTH, bar, pad, pad_left};
pub(crate) use super::bars::{LoginRow, clip, sanitize};

const RUNNING: char = '▶';
const WAITING: char = '⏸';
/// Longest workstream label; longer ones are cut.
const LABEL_WIDTH: usize = 30;
/// Width of a percentage: `100%`.
const PCT_WIDTH: usize = 4;
/// The percentage at which a workstream is done (merged): no marker.
const DONE: u8 = 100;

pub(crate) const NO_IDENTITY_HINT: &str =
    "no session id found; run with --all to see every run on this host";

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
    /// A live run: marked `▶`. A row that is neither running nor done is
    /// waiting: marked `⏸`, with its reason.
    pub running: bool,
    pub label: String,
    pub issue: Option<u64>,
    pub pr: PrCell,
    pub percent: u8,
    pub stage: String,
    pub concerns: Option<u64>,
    /// GitHub reports the PR as conflicting: `conflicts with main`.
    pub conflicts: bool,
    pub reason: Option<String>,
}

/// The logins section: rows, or why there are none.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum LoginsSection {
    Rows(Vec<LoginRow>),
    Refused(String),
}

/// Everything the report shows.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Snapshot {
    /// The repository name for the title (`amplihack-rs`), when every run
    /// names the same one.
    pub repo: Option<String>,
    /// When the report was made, as the title shows it: `Thu 11:56 PDT`.
    pub when: String,
    /// The caller has no identity and `--all` was not given.
    pub no_identity: bool,
    pub workstreams: Vec<WorkRow>,
    /// `None` with `--no-logins`.
    pub logins: Option<LoginsSection>,
}

/// The whole report.
pub(crate) fn render(snapshot: &Snapshot) -> String {
    let repo = snapshot
        .repo
        .as_deref()
        .map_or(String::new(), |r| format!("{} ", sanitize(r)));
    let mut lines = vec![
        format!("  {repo}workstreams — {}", sanitize(&snapshot.when)),
        String::new(),
    ];
    if snapshot.no_identity {
        lines.push(format!("  {NO_IDENTITY_HINT}"));
    }
    if snapshot.workstreams.is_empty() {
        lines.push("  no workstreams".to_string());
    } else {
        work_lines(&snapshot.workstreams, &mut lines);
    }
    if let Some(logins) = &snapshot.logins {
        lines.push(String::new());
        match logins {
            LoginsSection::Refused(reason) => {
                lines.push(format!("  logins: {}", sanitize(reason)));
            }
            LoginsSection::Rows(rows) if rows.is_empty() => {
                lines.push("  logins: none".to_string());
            }
            LoginsSection::Rows(rows) => lines.extend(bars::render_login_lines(rows)),
        }
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
    label: String,
    issue: String,
    pr: String,
    progress: String,
    stage: String,
}

/// Each text column is as wide as its widest cell or heading; the label
/// column is followed by one space and the number columns by two, as in the
/// issue.
fn work_lines(rows: &[WorkRow], lines: &mut Vec<String>) {
    let cells: Vec<WorkCells> = rows.iter().map(work_cells).collect();
    let width = |heading: &str, f: fn(&WorkCells) -> &str| {
        cells
            .iter()
            .map(|c| f(c).chars().count())
            .fold(heading.chars().count(), usize::max)
    };
    let label_w = width("WORKSTREAM", |c| &c.label) + 1;
    let issue_w = width("ISSUE", |c| &c.issue) + 2;
    let pr_w = width("PR", |c| &c.pr) + 2;
    let progress_w = bars::BAR_CELLS + 1 + PCT_WIDTH + 2;
    lines.push(format!(
        "  {}{}{}{}STAGE",
        pad("WORKSTREAM", label_w),
        pad("ISSUE", issue_w),
        pad("PR", pr_w),
        pad("PROGRESS", progress_w),
    ));
    for c in &cells {
        let head = format!(
            "  {}{}{}{}",
            pad(&c.label, label_w),
            pad(&c.issue, issue_w),
            pad(&c.pr, pr_w),
            pad(&c.progress, progress_w),
        );
        let room = MAX_WIDTH.saturating_sub(head.chars().count());
        lines.push(format!("{head}{}", clip(&c.stage, room)));
    }
}

fn work_cells(row: &WorkRow) -> WorkCells {
    let percent = row.percent.min(DONE);
    WorkCells {
        label: clip(&sanitize(&row.label), LABEL_WIDTH),
        issue: row.issue.map_or("-".to_string(), |n| format!("#{n}")),
        pr: match row.pr {
            PrCell::None => "-".to_string(),
            PrCell::Known(n) => format!("#{n}"),
            PrCell::Unknown => "#?".to_string(),
        },
        progress: format!(
            "{} {}",
            bar(f64::from(percent)).unwrap_or_default(),
            pad_left(&format!("{percent}%"), PCT_WIDTH)
        ),
        stage: stage_text(row, percent),
    }
}

/// The marker, then the stage and what follows it: `▶ PR open, conflicts
/// with main`, `⏸ build stopped on a usage limit`, `merged`. A done row has
/// no marker: it is not waiting.
fn stage_text(row: &WorkRow, percent: u8) -> String {
    let mut parts = Vec::new();
    let mut reason = row.reason.as_deref().map(sanitize);
    if row.stage == "building"
        && let Some(why) = reason.take()
    {
        parts.push(format!("build {why}"));
    } else {
        parts.push(sanitize(&row.stage));
    }
    if let Some(n) = row.concerns {
        parts.push(if n == 1 {
            "1 concern".to_string()
        } else {
            format!("{n} concerns")
        });
    }
    if row.conflicts {
        parts.push("conflicts with main".to_string());
    }
    parts.extend(reason);
    let text = parts.join(", ");
    match (percent >= DONE, row.running) {
        (true, _) => text,
        (false, true) => format!("{RUNNING} {text}"),
        (false, false) => format!("{WAITING} {text}"),
    }
}
