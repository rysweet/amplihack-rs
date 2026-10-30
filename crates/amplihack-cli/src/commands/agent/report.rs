//! What the supervisor writes and `agent status` prints: the recovery and
//! ALERT lines (R7.1, R7.2), the wrapper's WARNING line, the closed set of
//! result texts, the repeat rule (D13), the log append (S20) and both status
//! forms (R7.4, R7.5, B7).
//! Everything shown or logged passes through [`sanitize`].

use super::registry::{EndedBy, Entry, LastRecovery, Registry, State, fmt_ts, restarts_within};
use super::validate;
use amplihack_utils::secure_files::open_private_nofollow;
use serde_json::{Value, json};
use std::fmt;
use std::io::Write;
use std::path::Path;

/// The longest text kept from outside, such as a tmux stderr line.
const MAX_TEXT: usize = 200;
/// Longer than any line the supervisor builds; only a bound on hostile text.
const MAX_LINE: usize = 4096;

/// Control characters and the format characters of
/// [`validate::unsafe_char`] become `?`; at most `max` characters, cut on a
/// character boundary.
pub fn sanitize(s: &str, max: usize) -> String {
    s.chars()
        .take(max)
        .map(|c| if validate::unsafe_char(c) { '?' } else { c })
        .collect()
}

/// The result of one agent in a pass. `Display` is the result text.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Resumed,
    AlreadyRunning,
    LivenessUnknown(String),
    SessionExists,
    TmuxUnknown(String),
    NoConversation,
    NoLaunchDir,
    NoTranscript,
    Tmux(String),
    RestartLimit,
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resumed => f.write_str("resumed"),
            Self::AlreadyRunning => f.write_str("skipped: already running"),
            Self::LivenessUnknown(r) => {
                write!(f, "skipped: already running (liveness unknown: {r})")
            }
            Self::SessionExists => f.write_str("skipped: tmux session exists"),
            Self::TmuxUnknown(r) => {
                write!(f, "skipped: tmux session exists (tmux state unknown: {r})")
            }
            Self::NoConversation => f.write_str("failed: no conversation id"),
            Self::NoLaunchDir => f.write_str("failed: launch directory missing"),
            Self::NoTranscript => f.write_str("failed: no transcript"),
            Self::Tmux(line) => write!(f, "failed: tmux: {}", sanitize(line, MAX_TEXT)),
            Self::RestartLimit => f.write_str("stopped: restart limit"),
        }
    }
}

/// `<ts> agent=<name> conversation=<id|-> lost=<ts|-> resumed=<ts|-> result=<result>`.
pub fn recovery_line(
    now: i64,
    name: &str,
    conv: Option<&str>,
    lost: Option<&str>,
    resumed: Option<&str>,
    result: &str,
) -> String {
    let line = format!(
        "{} agent={name} conversation={} lost={} resumed={} result={result}",
        fmt_ts(now),
        conv.unwrap_or("-"),
        lost.unwrap_or("-"),
        resumed.unwrap_or("-"),
    );
    sanitize(&line, MAX_LINE)
}

/// The text stored in `alert`, after `ALERT: agent <name> ` in the line.
pub fn alert_text(n: usize) -> String {
    format!("stopped: restart limit ({n} restarts in the last hour)")
}

/// `<ts> ALERT: agent <name> stopped: restart limit (<n> restarts in the last hour)`.
pub fn alert_line(now: i64, name: &str, n: usize) -> String {
    let line = format!("{} ALERT: agent {name} {}", fmt_ts(now), alert_text(n));
    sanitize(&line, MAX_LINE)
}

/// `<ts> WARNING: agent <name> conversation not recorded: <reason>`, logged
/// by the wrapper when its id thread gives up or cannot record what it found.
pub fn not_recorded_line(now: i64, name: &str, reason: &str) -> String {
    let line = format!(
        "{} WARNING: agent {name} conversation not recorded: {reason}",
        fmt_ts(now)
    );
    sanitize(&line, MAX_LINE)
}

/// The `last_recovery` value holding the fields of the matching
/// [`recovery_line`] (B8).
pub fn last_recovery(
    now: i64,
    conv: Option<&str>,
    lost: Option<&str>,
    resumed: Option<&str>,
    result: &str,
) -> LastRecovery {
    LastRecovery {
        logged_at: fmt_ts(now),
        conversation_id: conv.map(str::to_string),
        lost: lost.map(str::to_string),
        resumed: resumed.map(str::to_string),
        result: sanitize(result, MAX_LINE),
    }
}

/// D13: a line repeating the agent's last conversation and result is not
/// written again, except a resume.
pub fn suppressed(last: Option<&LastRecovery>, conv: Option<&str>, result: &str) -> bool {
    last.is_some_and(|l| {
        result != "resumed" && l.conversation_id.as_deref() == conv && l.result == result
    })
}

/// Whether a stored result is cleared once its agent is seen healthy: a
/// skip or a failure, which would otherwise suppress its own recurrence and
/// stay in `status` after the agent recovered.
pub fn clears_when_healthy(result: &str) -> bool {
    result.starts_with("skipped:") || result.starts_with("failed:")
}

/// Appends `line` and a newline to the log in one write. The log is opened
/// without following a symlink and must be a private regular file with one
/// link (S20).
pub fn append(log: &Path, line: &str) -> std::io::Result<()> {
    let mut file = open_private_nofollow(log)?;
    let mut bytes = Vec::with_capacity(line.len() + 1);
    bytes.extend_from_slice(line.as_bytes());
    bytes.push(b'\n');
    file.write_all(&bytes)
}

fn state_text(e: &Entry) -> String {
    match e.state {
        State::Active => "active".to_string(),
        State::Stopped => "stopped".to_string(),
        State::Ended => format!(
            "ended {} ({})",
            e.ended_at.as_deref().unwrap_or("-"),
            e.ended_by.map_or("-", |b| match b {
                EndedBy::Wrapper => "wrapper",
                EndedBy::SessionEnd => "session-end",
            })
        ),
    }
}

/// The text form: a padded table sorted by name, then one `ALERT` line per
/// agent with an alert. Ends with a newline.
pub fn status_text(reg: &Registry) -> String {
    if reg.agents.is_empty() {
        return "no agents registered\n".to_string();
    }
    let header = [
        "NAME",
        "STATE",
        "CONVERSATION",
        "DIRECTORY",
        "LAST RECOVERY",
    ];
    let mut rows: Vec<[String; 5]> = vec![header.map(str::to_string)];
    for (name, e) in &reg.agents {
        let last = e
            .last_recovery
            .as_ref()
            .map_or("-".to_string(), |l| format!("{} {}", l.logged_at, l.result));
        let cells = [
            name.clone(),
            state_text(e),
            e.conversation_id.clone().unwrap_or_else(|| "-".into()),
            e.launch_dir.clone(),
            last,
        ];
        rows.push(cells.map(|c| sanitize(&c, MAX_LINE)));
    }
    let mut widths = [0usize; 5];
    for row in &rows {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for row in &rows {
        let mut line = String::new();
        for (i, cell) in row.iter().enumerate() {
            if i + 1 < row.len() {
                line.push_str(&format!("{cell:<w$}  ", w = widths[i]));
            } else {
                line.push_str(cell);
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    for (name, e) in &reg.agents {
        if let Some(alert) = &e.alert {
            let line = format!("ALERT {name}: {alert}");
            out.push_str(&sanitize(&line, MAX_LINE));
            out.push('\n');
        }
    }
    out
}

/// The JSON form: every key present, absent values `null` (B7).
pub fn status_json(reg: &Registry, now: i64) -> Value {
    let agents: Vec<Value> = reg
        .agents
        .iter()
        .map(|(name, e)| {
            let last = e.last_recovery.as_ref().map(|l| {
                json!({
                    "logged_at": l.logged_at,
                    "conversation_id": l.conversation_id,
                    "lost": l.lost,
                    "resumed": l.resumed,
                    "result": sanitize(&l.result, MAX_LINE),
                })
            });
            json!({
                "name": name,
                "state": e.state,
                "conversation_id": e.conversation_id,
                "launch_dir": e.launch_dir,
                "tmux_session": e.tmux_session,
                "tmux_window": e.tmux_window,
                "restarts_last_hour": restarts_within(&e.restarts, now, 3600),
                "last_recovery": last,
                "alert": e.alert.as_deref().map(|a| sanitize(a, MAX_LINE)),
                "ended_at": e.ended_at,
                "ended_by": e.ended_by,
            })
        })
        .collect();
    json!({ "agents": agents })
}
