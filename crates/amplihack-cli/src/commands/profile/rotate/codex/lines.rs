//! The lines the Codex rotation and `codex-resume` write. Every value from
//! outside (labels, paths, reasons, ids) is cleaned first, so it cannot
//! start a second line or forge a field. Nothing from an environment, an
//! events file or the extra `codex resume` arguments is ever written.

use super::super::change_log::redact_text;
use crate::commands::profile::usage_cmd::sanitize_display;
use crate::commands::profile::usage_codex::window_label;
use amplihack_utils::codex_capacity::{AccountResult, Credits, Outcome, Reading, headroom};

/// The longest label written.
const MAX_LABEL: usize = 64;
/// The longest error reason written.
const MAX_REASON: usize = 64;
/// The longest path written.
const MAX_PATH: usize = 512;
/// Resets later than this after the read are not usable times.
const RESET_RANGE_SECS: i64 = 400 * 86_400;

fn unprintable(c: char) -> bool {
    super::super::ledger::unprintable(c)
}

/// Whitespace as `_`, `(`, `)` and `,` as `_`, control and
/// bidirectional-override characters removed, at most 64 characters.
pub(super) fn clean_label(s: &str) -> String {
    s.chars()
        .filter_map(|c| match c {
            c if c.is_whitespace() => Some('_'),
            '(' | ')' | ',' => Some('_'),
            c if unprintable(c) => None,
            c => Some(c),
        })
        .take(MAX_LABEL)
        .collect()
}

/// A reason: secrets redacted, then cleaned as a label, at most 64
/// characters.
fn clean_reason(s: &str) -> String {
    let red = redact_text(&sanitize_display(s, false));
    clean_label(&red).chars().take(MAX_REASON).collect()
}

/// A path: control characters removed, whitespace as `_`.
pub(super) fn clean_path(p: &std::path::Path) -> String {
    p.display()
        .to_string()
        .chars()
        .filter_map(|c| match c {
            c if c.is_whitespace() => Some('_'),
            c if unprintable(c) => None,
            c => Some(c),
        })
        .take(MAX_PATH)
        .collect()
}

/// A number with no trailing zeros: `59`, `12.5`, `0.5`, `-1`.
fn num(v: f64) -> String {
    format!("{v}")
}

/// `MM-DDTHH:MMZ`, `-` with none, `?` when it is not a usable time.
fn reset(at: Option<i64>, read_at: i64) -> String {
    let Some(at) = at else {
        return "-".to_string();
    };
    let usable = at > 0
        && read_at
            .checked_add(RESET_RANGE_SECS)
            .is_some_and(|m| at <= m);
    match chrono::DateTime::from_timestamp(at, 0) {
        Some(t) if usable => t.format("%m-%dT%H:%MZ").to_string(),
        _ => "?".to_string(),
    }
}

fn credits(c: &Credits) -> String {
    match c.balance {
        Some(b) => num(b),
        None if c.has_credits => "yes".to_string(),
        None => "no".to_string(),
    }
}

/// `<label>:<window>=<p>%@<reset>,...,credits=<c>,room=<h>`.
fn entry(label: &str, r: &Reading, read_at: i64, floor: f64) -> String {
    let mut parts: Vec<String> = r
        .windows
        .iter()
        .map(|w| {
            let p = w
                .used_percent
                .map_or_else(|| "-".to_string(), |p| format!("{}", p.floor() as i64));
            format!(
                "{}={p}%@{}",
                window_label(w.name, w.window_minutes),
                reset(w.resets_at, read_at)
            )
        })
        .collect();
    if let Some(c) = &r.credits {
        parts.push(format!("credits={}", credits(c)));
    }
    parts.push(format!("room={}", num(headroom(r, floor))));
    format!("{}:{}", clean_label(label), parts.join(","))
}

fn why(o: &Outcome) -> String {
    match o {
        Outcome::NotSignedIn => "not-signed-in".to_string(),
        Outcome::Timeout => "timeout".to_string(),
        Outcome::Error(e) => format!("error:{}", clean_reason(e)),
        // `Outcome` may gain variants; an unknown one is an error.
        _ => "error:unknown".to_string(),
    }
}

/// The `codex cycle` line: the live account first, then the others in
/// order; unread accounts in `unread=`; the latest read time; the decision.
pub(super) fn cycle(
    live: Option<&str>,
    rows: &[AccountResult],
    floor: f64,
    now: i64,
    decision: &str,
) -> String {
    let mut ordered: Vec<&AccountResult> = rows
        .iter()
        .filter(|r| Some(r.label.as_str()) == live)
        .collect();
    ordered.extend(rows.iter().filter(|r| Some(r.label.as_str()) != live));
    let mut fields = vec![format!(
        "codex cycle live={}",
        live.map_or_else(|| "-".to_string(), clean_label)
    )];
    let mut unread = Vec::new();
    for r in ordered {
        match &r.outcome {
            Outcome::Ok(reading) => fields.push(entry(&r.label, reading, r.read_at, floor)),
            other => unread.push(format!("{}({})", clean_label(&r.label), why(other))),
        }
    }
    if !unread.is_empty() {
        fields.push(format!("unread={}", unread.join(",")));
    }
    let read = rows.iter().map(|r| r.read_at).max().unwrap_or(now);
    fields.push(format!("read={}", super::super::log::timestamp(read)));
    fields.push(format!("-> {decision}"));
    fields.join(" ")
}

pub(super) fn skip(reason: &str) -> String {
    format!("codex skip reason={reason}")
}

pub(super) fn swap(from: &str, to: &str, link: &std::path::Path) -> String {
    format!(
        "codex swap {}->{} link={} reason=room",
        clean_label(from),
        clean_label(to),
        clean_path(link)
    )
}

pub(super) fn refuse(code: &str, detail: Option<&str>) -> String {
    match detail {
        Some(d) => format!("codex refuse reason={code} detail={}", clean_label(d)),
        None => format!("codex refuse reason={code}"),
    }
}

pub(super) fn recover(from: &str, to: &str, rolled_back: bool) -> String {
    let reason = if rolled_back {
        "rolled-back"
    } else {
        "recovered"
    };
    format!(
        "codex recover {}->{} reason={reason}",
        clean_label(from),
        clean_label(to)
    )
}

pub(super) fn would_swap(from: &str, to: &str) -> String {
    format!(
        "dry-run would-swap {}->{}",
        clean_label(from),
        clean_label(to)
    )
}

pub(super) fn would_recover(from: &str, to: &str, phase: &str) -> String {
    format!(
        "dry-run would-recover {}->{} phase={phase}",
        clean_label(from),
        clean_label(to)
    )
}

/// `codex resume session=<id> <from>-><to> reason=<r> args=<n>`; the
/// arguments themselves are never written, only their number.
pub(super) fn resume(id: &str, from: &str, to: &str, after_reset: bool, args: usize) -> String {
    let reason = if after_reset { "after-reset" } else { "room" };
    format!(
        "codex resume session={} {}->{} reason={reason} args={args}",
        clean_label(id),
        clean_label(from),
        clean_label(to)
    )
}

pub(super) fn resume_none(reason: &str, id: &str) -> String {
    format!(
        "codex resume none reason={reason} session={}",
        clean_label(id)
    )
}
