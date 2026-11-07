//! The Codex block of `amplihack profile usage` (issue #2001): formatting
//! only. The live read is `amplihack_utils::codex_capacity`; this module
//! turns its results into the bar block, the `--table` block and the
//! `codex` rows of `--json` and `--summary-json`.
//!
//! Untrusted text (directory names, the root path, plan types and error
//! reasons) goes through `sanitize_display` before it is printed. JSON keeps
//! names and paths as they are, with JSON escaping only.

use super::usage_cmd::{format_pct, sanitize_display, utc_time};
use super::usage_view::{TimeView, bar_cell, reset_cell};
use amplihack_utils::codex_capacity::{
    AccountResult, Credits, DEFAULT_FLOOR, Outcome, Reading, Window, WindowName, next_choice,
};
use chrono::{DateTime, Local, TimeZone};
use serde::Serialize;
use std::path::PathBuf;

/// Characters of a label shown before it is clipped.
const LABEL_MAX: usize = 24;
/// The state column's width.
const STATE_W: usize = 13;
/// Spaces between fields.
const GAP: &str = "  ";
/// A reset more than this far after the read is shown as received.
const RESET_RANGE_SECS: i64 = 400 * 86_400;
/// The marker on the next choice.
const NEXT_MARK: &str = "<- next";

/// The Codex accounts as read, ready to print.
#[derive(Debug, Clone)]
pub(super) struct CodexView {
    /// The results in listing order.
    pub(super) rows: Vec<AccountResult>,
    /// The row `codex-pick`'s rule picks from these live readings, if any.
    /// `codex-pick` reads session files, which can be older, so the two can
    /// disagree.
    pub(super) next: Option<usize>,
    /// The Codex profiles root, as named in the header.
    pub(super) root: PathBuf,
    /// The deadline that applied, for `timed out` rows.
    pub(super) timeout_secs: u64,
}

/// The view of `rows`, with the next choice worked out at
/// [`DEFAULT_FLOOR`].
pub(super) fn view(root: PathBuf, rows: Vec<AccountResult>, timeout_secs: u64) -> CodexView {
    let next = next_choice(&rows, DEFAULT_FLOOR);
    CodexView {
        rows,
        next,
        root,
        timeout_secs,
    }
}

/// A window's length: `5h`, `7d`, else `<n>d`, `<n>h` or `<n>m`; the field
/// name when there is no length.
pub(super) fn window_label(name: WindowName, minutes: Option<u64>) -> String {
    match minutes {
        None => name.as_str().to_string(),
        Some(m) if m > 0 && m % 1440 == 0 => format!("{}d", m / 1440),
        Some(m) if m > 0 && m % 60 == 0 => format!("{}h", m / 60),
        Some(m) => format!("{m}m"),
    }
}

/// The bar block: the header and one row per account, or the one
/// `no accounts` line.
pub(super) fn bars_block(v: &CodexView, time: &TimeView, colour: bool) -> String {
    block(v, time, &|w| match w.used_percent {
        Some(p) => bar_cell(p, colour),
        None => "-".to_string(),
    })
}

/// The `--table` block: as [`bars_block`] with no bars and the percentage
/// to one decimal where there is one.
pub(super) fn table_block(v: &CodexView, time: &TimeView) -> String {
    block(v, time, &|w| {
        w.used_percent.map_or_else(|| "-".to_string(), format_pct)
    })
}

fn block(v: &CodexView, time: &TimeView, usage: &dyn Fn(&Window) -> String) -> String {
    let root = sanitize_display(&v.root.to_string_lossy(), false);
    if v.rows.is_empty() {
        return format!("Codex: no accounts under {root}\n");
    }
    let labels: Vec<String> = v.rows.iter().map(|r| label(&r.label)).collect();
    let lw = labels.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let mut out = format!("Codex accounts under {root}\n");
    for (i, (row, lab)) in v.rows.iter().zip(&labels).enumerate() {
        let mut line = lab.clone();
        line.extend(std::iter::repeat_n(' ', lw - lab.chars().count()));
        line.push_str(GAP);
        let mut fields = Vec::new();
        match &row.outcome {
            Outcome::Ok(r) => {
                line.push_str(&format!("{:<STATE_W$}", "signed in"));
                fields.extend(reading_fields(r, row.read_at, time, usage));
            }
            Outcome::NotSignedIn => line.push_str(&format!("{:<STATE_W$}", "not signed in")),
            Outcome::Timeout => {
                line.push_str(&format!("{:<STATE_W$}", "timed out"));
                fields.push(format!("after {}s", v.timeout_secs));
            }
            other => line.push_str(&format!(
                "error: {}",
                sanitize_display(&error_text(other), false)
            )),
        }
        fields.push(format!("read {}", clock(row.read_at, time)));
        if v.next == Some(i) {
            fields.push(NEXT_MARK.to_string());
        }
        for f in fields {
            line.push_str(GAP);
            line.push_str(&f);
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// The label: cleaned, and cut to 23 characters and `…` past 24.
fn label(raw: &str) -> String {
    let clean = sanitize_display(raw, false);
    if clean.chars().count() <= LABEL_MAX {
        return clean;
    }
    let mut s: String = clean.chars().take(LABEL_MAX - 1).collect();
    s.push('…');
    s
}

/// The fields of a signed-in row: its windows, credits and plan, or
/// `no windows reported`.
fn reading_fields(
    r: &Reading,
    read_at: i64,
    time: &TimeView,
    usage: &dyn Fn(&Window) -> String,
) -> Vec<String> {
    let mut fields: Vec<String> = r
        .windows
        .iter()
        .map(|w| {
            format!(
                "{} {} {}",
                window_label(w.name, w.window_minutes),
                usage(w),
                reset_text(w.resets_at, read_at, time)
            )
        })
        .collect();
    if let Some(c) = &r.credits {
        fields.push(credits_text(c));
    }
    if let Some(p) = &r.plan_type {
        fields.push(format!("plan {}", sanitize_display(p, false)));
    }
    if r.windows.is_empty() && r.credits.is_none() {
        fields.push("no windows reported".to_string());
    }
    fields
}

fn credits_text(c: &Credits) -> String {
    match c.balance {
        Some(b) => format!("credits {b}"),
        None if c.has_credits => "credits yes".to_string(),
        None => "credits no".to_string(),
    }
}

/// A reset as a time when it is in range (above 0 and no more than 400
/// days after the read).
fn in_range(at: i64, read_at: i64) -> bool {
    at > 0
        && read_at
            .checked_add(RESET_RANGE_SECS)
            .is_some_and(|max| at <= max)
}

/// `MM-DD HH:MM in <h>h <m>m`, `MM-DD HH:MM due`, `reset <raw>` out of
/// range, or `-` with no reset.
fn reset_text(at: Option<i64>, read_at: i64, time: &TimeView) -> String {
    let Some(at) = at else {
        return "-".to_string();
    };
    if !in_range(at, read_at) {
        return format!("reset {at}");
    }
    let cell = reset_cell(Some(at), time);
    let left = at.saturating_sub(read_at);
    if left <= 0 {
        return format!("{cell} due");
    }
    format!("{cell} in {}h {}m", left / 3600, left % 3600 / 60)
}

/// `HH:MM:SS` in the view's zone, with `Z` in UTC.
fn clock(t: i64, time: &TimeView) -> String {
    let shown = match time {
        TimeView::Utc => DateTime::from_timestamp(t, 0).map(|d| d.format("%H:%M:%SZ").to_string()),
        TimeView::Fixed(offset, _) => offset
            .timestamp_opt(t, 0)
            .single()
            .map(|d| d.format("%H:%M:%S").to_string()),
        TimeView::Local => Local
            .timestamp_opt(t, 0)
            .single()
            .map(|d| d.format("%H:%M:%S").to_string()),
    };
    shown.unwrap_or_else(|| "-".to_string())
}

/// The reason of a row that is neither signed in, not signed in, nor
/// timed out.
fn error_text(o: &Outcome) -> String {
    match o {
        Outcome::Error(e) => e.clone(),
        _ => "unknown outcome".to_string(),
    }
}

/// One `codex` row of `--json` and `--summary-json`, keys in this order.
#[derive(Debug, Clone, Serialize)]
pub(super) struct CodexJsonRow {
    account: String,
    path: String,
    state: &'static str,
    error: Option<String>,
    windows: Vec<CodexJsonWindow>,
    credits: Option<CodexJsonCredits>,
    plan_type: Option<String>,
    rate_limit_reached_type: Option<String>,
    ordinary_usage_allowed: Option<bool>,
    read_at: String,
    next_choice: bool,
}

#[derive(Debug, Clone, Serialize)]
struct CodexJsonWindow {
    name: WindowName,
    used_percent: Option<f64>,
    window_minutes: Option<u64>,
    resets_at: Option<i64>,
    resets_at_rfc3339: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct CodexJsonCredits {
    has_credits: bool,
    balance: Option<f64>,
}

/// The rows as JSON, in display order.
pub(super) fn json_rows(v: &CodexView) -> Vec<CodexJsonRow> {
    v.rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut row = CodexJsonRow {
                account: r.label.clone(),
                path: r.path.to_string_lossy().into_owned(),
                state: "error",
                error: None,
                windows: Vec::new(),
                credits: None,
                plan_type: None,
                rate_limit_reached_type: None,
                ordinary_usage_allowed: None,
                read_at: utc_time(r.read_at),
                next_choice: v.next == Some(i),
            };
            match &r.outcome {
                Outcome::Ok(reading) => {
                    row.state = "ok";
                    row.windows = reading
                        .windows
                        .iter()
                        .map(|w| json_window(w, r.read_at))
                        .collect();
                    row.credits = reading.credits.as_ref().map(|c| CodexJsonCredits {
                        has_credits: c.has_credits,
                        balance: c.balance,
                    });
                    row.plan_type = reading.plan_type.clone();
                    row.rate_limit_reached_type = reading.rate_limit_reached_type.clone();
                    row.ordinary_usage_allowed = reading.ordinary_usage_allowed;
                }
                Outcome::NotSignedIn => row.state = "not_signed_in",
                Outcome::Timeout => {
                    row.state = "timeout";
                    row.error = Some(format!("timed out after {}s", v.timeout_secs));
                }
                other => row.error = Some(error_text(other)),
            }
            row
        })
        .collect()
}

fn json_window(w: &Window, read_at: i64) -> CodexJsonWindow {
    CodexJsonWindow {
        name: w.name,
        used_percent: w.used_percent,
        window_minutes: w.window_minutes,
        resets_at: w.resets_at,
        resets_at_rfc3339: w
            .resets_at
            .filter(|at| in_range(*at, read_at))
            .map(utc_time),
    }
}
