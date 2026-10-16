//! The usage meter: readings parsed from Claude Code's status-line input,
//! attributed to the account live in the directory, merged into
//! `usage.json`, plus the reset time from Claude's usage-limit message. Passive
//! only: nothing here makes a request.

use super::state::HostEntry;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Status-line input larger than this is not parsed.
pub const MAX_INPUT: usize = 1 << 20;
/// Readings from a directory are ignored for this long after a swap.
pub const COOLDOWN_SECS: i64 = 300;
pub const LIMIT_MARKER: &str = "Claude AI usage limit reached";
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
    pub observed_at: i64,
    pub source: ReadingSource,
}

impl Reading {
    fn worst(&self) -> f64 {
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
    };
    (rl.five_hour.is_some() || rl.seven_day.is_some()).then_some(rl)
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
    if resets_at < now.saturating_sub(DAY) || resets_at > now.saturating_add(8 * DAY) {
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
        five_hour: rl.five_hour,
        seven_day: rl.seven_day,
        observed_at: now,
        source: ReadingSource::Statusline,
    }
}

/// The newer reading wins; on a tie, the one with the higher usage.
pub fn merge(old: Option<&Reading>, new: Reading) -> Reading {
    match old {
        Some(o) if o.observed_at > new.observed_at => o.clone(),
        Some(o) if o.observed_at == new.observed_at && o.worst() > new.worst() => o.clone(),
        _ => new,
    }
}

/// The reset time Claude's own usage-limit message states, or `None` when no
/// line starts with the marker. This only reads the time: whether an attempt
/// hit its limit is decided by the recipe runner's classifier. The last
/// marker line in stderr wins, then the last in stdout. The reset is never
/// later than five hours from now.
pub fn usage_limit_reset(stderr: &[u8], stdout: &[u8], now: i64) -> Option<i64> {
    let line = last_marker_line(stderr).or_else(|| last_marker_line(stdout))?;
    let rest = &line[LIMIT_MARKER.len()..];
    let latest = now + FIVE_HOURS;
    let epoch = rest.strip_prefix(b"|").and_then(|r| {
        let digits: Vec<u8> = r.iter().copied().take_while(u8::is_ascii_digit).collect();
        std::str::from_utf8(&digits).ok()?.parse::<i64>().ok()
    });
    Some(epoch.map_or(latest, |e| e.min(latest)))
}

/// The reset to record for an attempt that hit its usage limit: the stated
/// time, or five hours from now.
pub fn reset_or_default(stderr: &[u8], stdout: &[u8], now: i64) -> i64 {
    usage_limit_reset(stderr, stdout, now).unwrap_or(now + FIVE_HOURS)
}

fn last_marker_line(bytes: &[u8]) -> Option<&[u8]> {
    bytes
        .split(|&b| b == b'\n')
        .rfind(|l| l.starts_with(LIMIT_MARKER.as_bytes()))
}
