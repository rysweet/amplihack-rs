//! The thresholds in force, for `profile usage` (issue #2007): one text line
//! after the capacity summary and the `thresholds` object of
//! `--summary-json`. The contract is docs/reference/profile-command.md,
//! "Thresholds in force".
//!
//! Each value comes from `rotate::settings::thresholds`: the
//! `AMPLIHACK_ROTATE_*` variable, the `rotator.*` key or the default. When a
//! rotator runs with a valid record, its `session_at` and `week_at` are the
//! values in use and are shown with origin `rotator`; the record holds no
//! interval and no Codex floor. A value that cannot be used is shown with
//! its message and never fails the command.

use super::overflow::Gate;
use super::rotate::gate_record;
use super::rotate::settings::{self, Cfg, Env, Origin, Threshold, Thresholds, UserConfig};
use super::usage_cmd::sanitize_ascii;
use crate::commands::profile::ProfileError;
use serde_json::{Map, Value, json};
use std::path::Path;

/// The thresholds for this run, and the Codex floor for the next choice.
pub(super) struct Current {
    pub(super) thresholds: Thresholds,
    /// The floor `next_choice` uses, or the bare reason it cannot be used.
    pub(super) codex_credit_floor: Result<u64, String>,
}

/// The thresholds under `root`, from this process's environment, the user
/// config file and the running rotator, if any.
pub(super) fn current(root: &Path) -> Current {
    let user = UserConfig::read();
    let cfg = |k: &str| user.get(k);
    let env = |k: &str| std::env::var_os(k);
    Current {
        thresholds: resolve(gate_record::running(root), &env, &cfg),
        codex_credit_floor: settings::codex_credit_floor_value(&cfg),
    }
}

/// [`settings::thresholds`], with the running rotator's recorded
/// `session_at` and `week_at` over it, even where `cfg` is bad for them.
pub(super) fn resolve(rotator: Option<Gate>, env: &Env<'_>, cfg: &Cfg<'_>) -> Thresholds {
    let mut t = settings::thresholds(env, cfg);
    if let Some(g) = rotator {
        let at = |value| {
            Ok(Threshold {
                value,
                origin: Origin::Rotator,
            })
        };
        t.session_at = at(g.session_at);
        t.week_at = at(g.week_at);
    }
    t
}

/// A field of the text line: its value and origin, or `invalid (<why>)`.
fn field<T>(
    name: &str,
    r: &Result<Threshold<T>, ProfileError>,
    shown: impl Fn(&Threshold<T>) -> String,
) -> String {
    match r {
        Ok(t) => format!("{name} {}", shown(t)),
        Err(e) => format!("{name} invalid ({})", sanitize_ascii(&e.to_string())),
    }
}

/// The one line, without its newline. Values and messages are ASCII; the
/// only other characters are the fixed ` · ` separators (decision D10).
pub(super) fn text_line(t: &Thresholds) -> String {
    let session = field("session-at", &t.session_at, |s| {
        format!("{}% ({})", s.value, s.origin.label())
    });
    let week = field("week-at", &t.week_at, |w| {
        format!(
            "{}% ({}, review reserve {}%)",
            w.value,
            w.origin.label(),
            100u8.saturating_sub(w.value)
        )
    });
    let interval = field("interval", &t.interval, |i| {
        format!("{}s ({})", i.value, i.origin.label())
    });
    let floor = field("codex credit floor", &t.codex_credit_floor, |c| {
        format!("{} ({})", c.value, c.origin.label())
    });
    format!("Rotator thresholds: {session} · {week} · {interval} · {floor}")
}

/// One key of the JSON object: `{value, origin}`, or both `null` and the
/// uncleaned `error`.
fn json_field<T: Copy + Into<u64>>(r: &Result<Threshold<T>, ProfileError>) -> Map<String, Value> {
    let mut m = Map::new();
    match r {
        Ok(t) => {
            m.insert("value".into(), json!(t.value.into()));
            m.insert("origin".into(), json!(t.origin.label()));
        }
        Err(e) => {
            m.insert("value".into(), Value::Null);
            m.insert("origin".into(), Value::Null);
            m.insert("error".into(), json!(e.to_string()));
        }
    }
    m
}

/// The `thresholds` object of `--summary-json`: always all four keys and
/// `review_reserve` at the top level (decision D10), `null` when `week_at`
/// is bad.
pub(super) fn json(t: &Thresholds) -> Value {
    let reserve = t
        .week_at
        .as_ref()
        .map_or(Value::Null, |w| json!(100u8.saturating_sub(w.value)));
    json!({
        "session_at": json_field(&t.session_at),
        "week_at": json_field(&t.week_at),
        "review_reserve": reserve,
        "interval": json_field(&t.interval),
        "codex_credit_floor": json_field(&t.codex_credit_floor),
    })
}
