//! The table form of `governor status`, and the sanitiser for every string
//! the governor prints.

use super::status::{StatusView, WindowView};
use std::fmt::Write;

/// The most characters of one printed string.
const MAX_DISPLAY: usize = 64;

/// Control characters become `?`; at most 64 characters are kept.
pub fn display_safe(raw: &str) -> String {
    raw.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .take(MAX_DISPLAY)
        .collect()
}

fn num(v: Option<f64>) -> String {
    v.map_or_else(|| "-".to_string(), |v| format!("{v:.1}"))
}

fn time(v: Option<i64>) -> String {
    v.and_then(|t| chrono::DateTime::from_timestamp(t, 0))
        .map_or_else(
            || "-".to_string(),
            |t| t.format("%Y-%m-%d %H:%M UTC").to_string(),
        )
}

fn window(w: Option<&WindowView>, missing: &str) -> String {
    match w {
        None => missing.to_string(),
        Some(w) => format!(
            "{:.1}% (resets {}; burn {}/h, sustainable {}/h)",
            w.used_pct,
            time(w.resets_at),
            num(w.burn_rate),
            num(w.sustainable_rate)
        ),
    }
}

/// The human-readable form of `v`.
pub fn render_table(v: &StatusView) -> String {
    let mut out = String::new();
    if !v.enabled {
        let _ = writeln!(
            out,
            "Capacity governor: off (AMPLIHACK_GOVERNOR or governor.enabled)"
        );
        return out;
    }
    let limit = v.limit.map_or_else(|| "-".to_string(), |l| l.to_string());
    let _ = writeln!(
        out,
        "Capacity governor: on; concurrency limit {limit} of {}",
        v.max
    );
    if let Some(r) = &v.fail_open_reason {
        let _ = writeln!(
            out,
            "Last fail-open: {} ({})",
            display_safe(&r.code),
            display_safe(&r.message)
        );
    }
    let _ = writeln!(out, "\nProfiles:");
    if v.profiles.is_empty() {
        let _ = writeln!(out, "  (none)");
    }
    for p in &v.profiles {
        let mut marks = Vec::new();
        if p.active {
            marks.push("active");
        }
        if !p.eligible {
            marks.push("not in pool");
        }
        let marks = if marks.is_empty() {
            String::new()
        } else {
            format!(" [{}]", marks.join(", "))
        };
        let _ = writeln!(out, "  {}{marks}", display_safe(&p.name));
        let session = if p.weekly.is_none() {
            "no reading".to_string()
        } else {
            window(p.session.as_ref(), "no session limit")
        };
        let _ = writeln!(out, "    session: {session}");
        let _ = writeln!(
            out,
            "    weekly:  {}",
            window(p.weekly.as_ref(), "no reading")
        );
    }
    let r = &v.reserve;
    let _ = writeln!(
        out,
        "\nReserve: {} points; best weekly remainder {}; ordinary work {}",
        r.points,
        num(r.best_weekly_remaining),
        if r.ordinary_held { "held" } else { "admitted" }
    );
    let _ = writeln!(out, "\nSlots:");
    if v.slots.is_empty() {
        let _ = writeln!(out, "  (none)");
    }
    for s in &v.slots {
        let stale = if s.stale { " [stale]" } else { "" };
        let _ = writeln!(
            out,
            "  pid {} {} {} since {}; {} nested{stale}",
            s.pid,
            display_safe(&s.workstream),
            s.class.as_str(),
            time(Some(s.admitted_at)),
            s.nested_children
        );
    }
    let _ = writeln!(out, "\nQueue:");
    if v.queue.is_empty() {
        let _ = writeln!(out, "  (empty)");
    }
    for q in &v.queue {
        let stale = if q.stale { " [stale]" } else { "" };
        let _ = writeln!(
            out,
            "  #{} {} {} pid {} since {}{stale}",
            q.seq,
            display_safe(&q.workstream),
            q.class.as_str(),
            q.pid,
            time(Some(q.enqueued_at))
        );
    }
    let _ = writeln!(out, "\nWorkstreams:");
    if v.workstreams.is_empty() {
        let _ = writeln!(out, "  (none)");
    }
    for w in &v.workstreams {
        let share = w
            .token_share
            .map_or_else(|| "-".to_string(), |s| format!("{:.0}%", s * 100.0));
        let _ = writeln!(
            out,
            "  {}: {} admissions in 24 h, {} slots, {} waiting, token share {share}",
            display_safe(&w.name),
            w.admissions,
            w.slots,
            w.waiting
        );
    }
    out
}
