//! The usage meter: readings parsed from Claude Code's status-line input,
//! attributed to the account live in the directory, merged into
//! `usage.json`, plus the reset time from Claude's usage-limit message. Passive
//! only: nothing here makes a request.

use super::state::HostEntry;
use crate::commands::recipe::run::limit_patterns;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Status-line input larger than this is not parsed.
pub const MAX_INPUT: usize = 1 << 20;
/// Readings from a directory are ignored for this long after a swap.
pub const COOLDOWN_SECS: i64 = 300;
const FIVE_HOURS: i64 = 5 * 3600;
const DAY: i64 = 86_400;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Window {
    pub used_percentage: f64,
    pub resets_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RateLimits {
    pub five_hour: Option<Window>,
    pub seven_day: Option<Window>,
    pub week_models: Vec<ModelWeek>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadingSource {
    Statusline,
    UsageLimitFailure,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reading {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub five_hour: Option<Window>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seven_day: Option<Window>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub week_models: Vec<ModelWeek>,
    pub observed_at: i64,
    pub source: ReadingSource,
}

/// One model's last observed week. Its age is independent of other windows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelWeek {
    pub label: String,
    pub used_percentage: f64,
    pub resets_at: Option<i64>,
    pub observed_at: i64,
}

impl Reading {
    /// Issue #1818: the reading has a week window and no session window.
    ///
    /// This is a fact about the stored reading, not about the plan: the
    /// usage file has no field for "a session limit applies, but its window
    /// is unknown". A plan with no session limit gives this, and so do a
    /// status-line `five_hour` that was absent or rejected by
    /// `parse_window` or `plausible`, and a `profile usage` session
    /// window that `plausible` rejected when the stored one had already
    /// reset. Callers label it `no session window`, never `no session
    /// limit`.
    pub fn no_session_window(&self) -> bool {
        self.seven_day.is_some() && self.five_hour.is_none()
    }

    pub(crate) fn worst(&self) -> f64 {
        let pct = |w: &Option<Window>| w.map_or(0.0, |w| w.used_percentage);
        pct(&self.five_hour).max(pct(&self.seven_day))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IgnoreReason {
    Cooldown,
    Fingerprint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attribution {
    Record(String),
    Ignore(IgnoreReason),
}

/// The `rate_limits` object of Claude Code's status-line input. `None` when
/// the input is too large, not JSON, or has no usable window.
pub fn parse_statusline_input(raw: &[u8], now: i64) -> Option<RateLimits> {
    if raw.len() > MAX_INPUT {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(raw).ok()?;
    let limits = value.get("rate_limits")?.as_object()?;
    let window = |key: &str| limits.get(key).and_then(|w| parse_window(w, now));
    let rl = RateLimits {
        five_hour: window("five_hour"),
        seven_day: window("seven_day"),
        // Status-line windows use used_percentage, not the request probe's
        // utilization. Keep the same window bounds as the other meters.
        week_models: limits
            .get("model_scoped")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let label = entry.get("display_name")?.as_str()?;
                if !(1..=64).contains(&label.chars().count())
                    || super::super::usage_cmd::sanitize_display(label, false) != label
                {
                    return None;
                }
                let w = parse_window(entry, now)?;
                Some(ModelWeek {
                    label: label.to_string(),
                    used_percentage: w.used_percentage,
                    resets_at: Some(w.resets_at),
                    observed_at: now,
                })
            })
            .collect(),
    };
    (rl.five_hour.is_some() || rl.seven_day.is_some() || !rl.week_models.is_empty()).then_some(rl)
}

fn parse_window(w: &serde_json::Value, now: i64) -> Option<Window> {
    let pct = w
        .get("used_percentage")?
        .as_f64()
        .filter(|p| p.is_finite())?;
    let resets_at = match w.get("resets_at")? {
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) => i,
            None => {
                let f = n.as_f64().filter(|f| f.is_finite())?;
                if f.abs() > 1e15 {
                    return None;
                }
                f.floor() as i64
            }
        },
        serde_json::Value::String(s) => chrono::DateTime::parse_from_rfc3339(s).ok()?.timestamp(),
        _ => return None,
    };
    plausible(pct, resets_at, now)
}

/// A window worth recording: a finite percentage (clamped to 0–100) whose
/// reset is between one day ago and eight days from now. The one bounds rule
/// for status-line input and `profile usage` readings.
pub fn plausible(pct: f64, resets_at: i64, now: i64) -> Option<Window> {
    if !pct.is_finite()
        || resets_at < now.saturating_sub(DAY)
        || resets_at > now.saturating_add(8 * DAY)
    {
        return None;
    }
    Some(Window {
        used_percentage: pct.clamp(0.0, 100.0),
        resets_at,
    })
}

/// SHA-256 (lowercase hex) of `five_pct|five_reset|seven_pct|seven_reset`,
/// with `-|-` for a missing window.
pub fn fingerprint(rl: &RateLimits) -> String {
    let part = |w: &Option<Window>| match w {
        Some(w) => format!("{:?}|{}", w.used_percentage, w.resets_at),
        None => "-|-".to_string(),
    };
    let text = format!("{}|{}", part(&rl.five_hour), part(&rl.seven_day));
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Which account a reading from `owner`'s directory belongs to.
pub fn attribute(owner: &str, host: Option<&HostEntry>, rl: &RateLimits, now: i64) -> Attribution {
    let Some(host) = host else {
        return Attribution::Record(owner.to_string());
    };
    if now - host.swapped_at < COOLDOWN_SECS {
        return Attribution::Ignore(IgnoreReason::Cooldown);
    }
    if host.pre_swap_fingerprint.as_deref() == Some(fingerprint(rl).as_str()) {
        return Attribution::Ignore(IgnoreReason::Fingerprint);
    }
    Attribution::Record(host.live.clone())
}

pub fn reading_from(rl: &RateLimits, now: i64) -> Reading {
    Reading {
        week_models: rl.week_models.clone(),
        five_hour: rl.five_hour,
        seven_day: rl.seven_day,
        observed_at: now,
        source: ReadingSource::Statusline,
    }
}

/// The newer reading wins; on a tie, the one with the higher usage.
pub fn merge(old: Option<&Reading>, mut new: Reading) -> Reading {
    let mut models = std::collections::BTreeMap::<String, ModelWeek>::new();
    let now = old.map_or(new.observed_at, |o| o.observed_at.max(new.observed_at));
    for m in old
        .into_iter()
        .flat_map(|o| &o.week_models)
        .chain(&new.week_models)
    {
        let replace = models.get(&m.label).is_none_or(|previous| {
            m.observed_at > previous.observed_at
                || (m.observed_at == previous.observed_at
                    && m.used_percentage >= previous.used_percentage)
        });
        if replace {
            models.insert(m.label.clone(), m.clone());
        }
    }
    if let Some(o) = old
        && (o.observed_at > new.observed_at
            || (o.observed_at == new.observed_at && o.worst() > new.worst()))
    {
        new = o.clone();
    }
    new.week_models = models
        .into_values()
        .filter(|m| m.resets_at.is_some_and(|reset| reset > now))
        .collect();
    new
}

/// The reset time Claude's own usage-limit message states, or `None` when no
/// line starts with its epoch marker. This only reads the time: whether an
/// attempt hit its limit is decided by the recipe runner's classifier. The
/// last marker line in stderr wins, then the last in stdout. The reset is
/// never later than five hours from now. The patterns live in
/// `recipe::run::limit_patterns` (#1618).
pub fn usage_limit_reset(stderr: &[u8], stdout: &[u8], now: i64) -> Option<i64> {
    let epoch = limit_patterns::epoch_marker(stderr, stdout)?;
    let latest = now.saturating_add(FIVE_HOURS);
    Some(epoch.map_or(latest, |e| e.min(latest)))
}

/// The reset to record for an attempt that hit its usage limit: the stated
/// time, or five hours from now.
pub fn reset_or_default(stderr: &[u8], stdout: &[u8], now: i64) -> i64 {
    usage_limit_reset(stderr, stdout, now).unwrap_or(now + FIVE_HOURS)
}
