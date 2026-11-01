//! The kill switch, the `governor.*` user-config keys and the wait grammar
//! (D1, D10). `AMPLIHACK_GOVERNOR` and `AMPLIHACK_GOVERNOR_WAIT` are read
//! here, not by the config schema, and outrank the keys.

use crate::user_config::schema::Value;

/// The kill switch: `off`, `0` or `false` turn the governor off; `on`, `1`
/// or `true` turn it on whatever `governor.enabled` says.
pub const GOVERNOR_ENV: &str = "AMPLIHACK_GOVERNOR";
/// How long a start may wait: `--governor-wait` grammar, outranks
/// `governor.wait_secs`.
pub const GOVERNOR_WAIT_ENV: &str = "AMPLIHACK_GOVERNOR_WAIT";
/// How often a waiting start asks again.
pub const GOVERNOR_POLL_SECS: u64 = 15;
/// The longest wait.
pub const GOVERNOR_WAIT_CAP_SECS: u64 = 86_400;

/// The resolved settings. Ranges are those of the schema keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub enabled: bool,
    pub max_concurrency: u32,
    pub reserve_points: u32,
    pub step: u32,
    pub band_pct: u32,
    pub eval_interval_secs: u64,
    pub wait_secs: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            max_concurrency: 4,
            reserve_points: 10,
            step: 1,
            band_pct: 10,
            eval_interval_secs: 60,
            wait_secs: 600,
        }
    }
}

/// `Some(true)` for on, `Some(false)` for off, `None` for anything else.
fn switch(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "on" | "1" | "true" => Some(true),
        "off" | "0" | "false" => Some(false),
        _ => None,
    }
}

/// The settings from the environment and the user-scope `governor.*`
/// values. A value of the wrong type is ignored; an integer outside its
/// range is clamped into it.
pub fn resolve(
    env: &dyn Fn(&str) -> Option<String>,
    user: &dyn Fn(&str) -> Option<Value>,
) -> Settings {
    let d = Settings::default();
    let int = |key: &str, min: u64, max: u64, default: u64| -> u64 {
        user(key)
            .and_then(|v| v.as_int())
            .map_or(default, |v| v.clamp(min, max))
    };
    let small = |key: &str, min: u64, max: u64, default: u32| -> u32 {
        u32::try_from(int(key, min, max, u64::from(default))).unwrap_or(default)
    };
    let enabled = env(GOVERNOR_ENV)
        .and_then(|raw| switch(&raw))
        .or_else(|| user("governor.enabled").and_then(|v| v.as_bool()))
        .unwrap_or(d.enabled);
    Settings {
        enabled,
        max_concurrency: small("governor.max_concurrency", 1, 64, d.max_concurrency),
        reserve_points: small("governor.reserve_points", 0, 100, d.reserve_points),
        step: small("governor.step", 1, 8, d.step),
        band_pct: small("governor.band_pct", 0, 50, d.band_pct),
        eval_interval_secs: int("governor.eval_interval_secs", 1, 3600, d.eval_interval_secs),
        wait_secs: int("governor.wait_secs", 0, GOVERNOR_WAIT_CAP_SECS, d.wait_secs),
    }
}

/// A wait: one to six ASCII digits and an optional `s`, `m` or `h`, in
/// seconds, capped at 24 hours.
pub fn parse_wait(raw: &str) -> Result<u64, String> {
    let bad = || format!("invalid wait {raw:?}: expected 1 to 6 digits and an optional s, m or h");
    let (digits, unit) = match raw.char_indices().last() {
        Some((i, 's')) => (&raw[..i], 1u64),
        Some((i, 'm')) => (&raw[..i], 60),
        Some((i, 'h')) => (&raw[..i], 3600),
        _ => (raw, 1),
    };
    if digits.is_empty() || digits.len() > 6 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad());
    }
    let n: u64 = digits.parse().map_err(|_| bad())?;
    Ok(n.checked_mul(unit)
        .map_or(GOVERNOR_WAIT_CAP_SECS, |s| s.min(GOVERNOR_WAIT_CAP_SECS)))
}

/// The wait for this start: the flag, then [`GOVERNOR_WAIT_ENV`] when it
/// parses, then the setting.
pub fn wait_secs(flag: Option<u64>, env: &dyn Fn(&str) -> Option<String>, s: &Settings) -> u64 {
    if let Some(secs) = flag {
        return secs.min(GOVERNOR_WAIT_CAP_SECS);
    }
    match env(GOVERNOR_WAIT_ENV).map(|raw| parse_wait(&raw)) {
        Some(Ok(secs)) => secs,
        Some(Err(_)) => {
            tracing::warn!("{GOVERNOR_WAIT_ENV} is not a valid wait; using governor.wait_secs");
            s.wait_secs
        }
        None => s.wait_secs,
    }
}
