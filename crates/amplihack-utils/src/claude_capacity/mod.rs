//! Reader side of the no-capacity notice: `<root>/.auto-switch.d/capacity.json`.
//!
//! The file is written by amplihack-cli (the status-line recorder,
//! `profile usage` and the rotator) and read here, so the hooks crate can show
//! the notice without depending on amplihack-cli. Every reader fails closed:
//! anything unexpected means "no notice", never an error. Nothing here prints,
//! spawns a process or touches the network.
//!
//! See "When no account has capacity" in docs/reference/profile-command.md.

use chrono::{DateTime, Local, TimeDelta, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub mod dedupe;
mod fs;

#[cfg(all(test, unix))]
mod tests;

/// The profile-root variable (same as amplihack-cli's `ROOT_ENV`).
pub const ROOT_ENV: &str = "AMPLIHACK_CLAUDE_PROFILE_ROOT";
/// The auto-switch override: exactly `1` or `0` wins over the file.
pub const TOGGLE_ENV: &str = "AMPLIHACK_CLAUDE_AUTO_SWITCH";
/// The auto-switch settings file under the root.
pub const TOGGLE_FILE: &str = ".auto-switch";
/// The auto-switch state directory under the root.
pub const STATE_DIR: &str = ".auto-switch.d";
/// The capacity file under [`STATE_DIR`].
pub const FILE: &str = "capacity.json";
pub const VERSION: u32 = 1;
/// A capacity file larger than this is not read.
pub const MAX_BYTES: u64 = 16 * 1024;
/// How long a state with an unknown reset time is trusted.
pub const UNKNOWN_RESET_TTL_SECS: i64 = 5 * 3600;
/// How far in the future `written_at` may be before the file is refused.
pub const MAX_FUTURE_SKEW_SECS: i64 = 5 * 60;
/// An `earliest_reset` further than this after `written_at` counts as unknown.
pub const MAX_RESET_AHEAD_SECS: i64 = 8 * 86_400;
/// The command every notice tells the user to run.
pub const INSPECT: &str = "amplihack profile usage";

/// The saved-root file and the toggle file are refused above this size.
const MAX_SMALL_FILE: u64 = 4096;
const DEPTH_ENV: &str = "AMPLIHACK_SESSION_DEPTH";

/// Ordered `Ok < Low < Exhausted`, so "worse" is `>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Ok,
    /// Every eligible account is at or above the auto-switch threshold.
    Low,
    /// Every eligible account is at its hard limit.
    Exhausted,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Low => "low",
            Self::Exhausted => "exhausted",
        }
    }
}

/// `capacity.json`, version 1. Unknown fields are ignored so a newer writer
/// does not silence an older reader.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapacityFile {
    pub version: u32,
    pub state: State,
    pub threshold: u8,
    /// Profile names only; never shown to the user or the model.
    pub over: Vec<String>,
    pub earliest_reset: Option<DateTime<Utc>>,
    pub written_at: DateTime<Utc>,
    /// `statusline`, `usage` or `rotate`; never shown.
    pub writer: String,
}

/// `<root>/.auto-switch.d/capacity.json`.
pub fn path(root: &Path) -> PathBuf {
    root.join(STATE_DIR).join(FILE)
}

/// The profile root for this process's environment and `HOME`, or `None`
/// when it cannot be resolved safely.
pub fn resolve_root() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|h| !h.is_empty())
        .map(PathBuf::from);
    resolve_root_with(|k| std::env::var_os(k), home.as_deref())
}

/// [`resolve_root`] with the environment and `HOME` passed in. Matches
/// amplihack-cli's `root_config::resolve_with`, except that every error is
/// `None` (fail closed, never the default root).
pub fn resolve_root_with(
    env: impl Fn(&str) -> Option<OsString>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(raw) = env(ROOT_ENV).filter(|v| !v.is_empty()) {
        let raw = raw.into_string().ok()?;
        return check_root_value(&raw).then(|| PathBuf::from(raw));
    }
    let config = home?.join(".amplihack");
    match fs::read_small(&config.join("profile-root"), false) {
        Read::Missing => Some(config.join("claude-profiles")),
        Read::Refused => None,
        Read::Bytes(bytes) => {
            if bytes.len() as u64 > MAX_SMALL_FILE {
                return None;
            }
            let raw = std::str::from_utf8(&bytes).ok()?.trim();
            check_root_value(raw).then(|| PathBuf::from(raw))
        }
    }
}

/// The content rule amplihack-cli's `root_config::check_value` applies.
fn check_root_value(raw: &str) -> bool {
    !raw.is_empty()
        && !raw.chars().any(char::is_control)
        && Path::new(raw).is_absolute()
        && !Path::new(raw)
            .components()
            .any(|c| c == std::path::Component::ParentDir)
}

/// Whether auto-switch is on for `root`: the same answer as amplihack-cli's
/// `config::effective(root).on`, without printing anything.
pub fn auto_switch_on(root: &Path) -> bool {
    match std::env::var_os(TOGGLE_ENV)
        .as_deref()
        .map(|v| v.as_encoded_bytes())
    {
        Some(b"1") => return true,
        Some(b"0") => return false,
        // Unset, empty or anything else: the file decides.
        _ => {}
    }
    match fs::read_small(&root.join(TOGGLE_FILE), true) {
        Read::Bytes(raw) => toggle_enabled(&raw).unwrap_or(false),
        Read::Missing | Read::Refused => false,
    }
}

/// `enabled` from a `.auto-switch` file, or `None` when amplihack-cli's
/// `config::parse` would call it corrupt.
fn toggle_enabled(raw: &[u8]) -> Option<bool> {
    if raw.len() as u64 > MAX_SMALL_FILE {
        return None;
    }
    let text = std::str::from_utf8(raw).ok()?;
    let body = text.strip_suffix('\n').unwrap_or(text);
    let (mut enabled, mut threshold, mut exclude) = (None, false, false);
    for line in body.split('\n') {
        let (key, value) = line.split_once('=')?;
        match key {
            "enabled" => {
                let on = match value {
                    "on" => true,
                    "off" => false,
                    _ => return None,
                };
                if enabled.replace(on).is_some() {
                    return None;
                }
            }
            "threshold" => {
                let valid = !value.is_empty()
                    && value.bytes().all(|b| b.is_ascii_digit())
                    && matches!(value.parse::<u8>(), Ok(1..=100));
                if !valid || std::mem::replace(&mut threshold, true) {
                    return None;
                }
            }
            "exclude" => {
                let valid = value.is_empty() || value.split(',').all(profile_name_ok);
                if !valid || std::mem::replace(&mut exclude, true) {
                    return None;
                }
            }
            _ => return None,
        }
    }
    enabled
}

/// amplihack-cli's `ProfileName::parse` rule.
fn profile_name_ok(raw: &str) -> bool {
    let mut chars = raw.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    let stem = raw.split('.').next().unwrap_or(raw).to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&stem.as_str())
        || ["COM", "LPT"].iter().any(|p| {
            stem.strip_prefix(p)
                .is_some_and(|d| d.len() == 1 && matches!(d.as_bytes()[0], b'1'..=b'9'))
        });
    raw.len() <= 64
        && first.is_ascii_alphanumeric()
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && !raw.contains("..")
        && !raw.ends_with('.')
        && !reserved
}

/// The capacity file, checked against the current time. A convenience
/// wrapper over [`read_at`] for callers and tests that use the current time;
/// production readers go through [`current_now`].
pub fn read(root: &Path) -> Option<CapacityFile> {
    read_at(root, Utc::now())
}

/// The capacity file under `root`, or `None` when it is missing, not a
/// regular file owned by the current user, a symlink, larger than
/// [`MAX_BYTES`], not JSON, the wrong version, has a threshold outside
/// 1..=100, or a `written_at` more than [`MAX_FUTURE_SKEW_SECS`] after `now`.
/// An `earliest_reset` more than [`MAX_RESET_AHEAD_SECS`] after `written_at`
/// is returned as `None`.
pub fn read_at(root: &Path, now: DateTime<Utc>) -> Option<CapacityFile> {
    let bytes = fs::read_owned(&path(root), MAX_BYTES)?;
    let mut f: CapacityFile = serde_json::from_slice(&bytes).ok()?;
    if f.version != VERSION || !(1..=100).contains(&f.threshold) {
        return None;
    }
    // An overflow means `now` is at the end of time: nothing is after it.
    if let Some(latest) = now.checked_add_signed(TimeDelta::seconds(MAX_FUTURE_SKEW_SECS))
        && f.written_at > latest
    {
        return None;
    }
    if let Some(reset) = f.earliest_reset {
        let plausible = f
            .written_at
            .checked_add_signed(TimeDelta::seconds(MAX_RESET_AHEAD_SECS))
            .is_some_and(|limit| reset <= limit);
        if !plausible {
            f.earliest_reset = None;
        }
    }
    Some(f)
}

/// The state to act on at `now`: `Ok` once the earliest reset has passed, or
/// [`UNKNOWN_RESET_TTL_SECS`] after `written_at` when the reset is unknown.
pub fn effective_state(file: &CapacityFile, now: DateTime<Utc>) -> State {
    if file.state == State::Ok {
        return State::Ok;
    }
    let expires = match file.earliest_reset {
        Some(reset) => Some(reset),
        None => file
            .written_at
            .checked_add_signed(TimeDelta::seconds(UNKNOWN_RESET_TTL_SECS)),
    };
    match expires {
        Some(at) if now < at => file.state,
        // Past the reset, past the bound, or a bound that overflowed.
        _ => State::Ok,
    }
}

/// The effective state and the file, when auto-switch is on and the file
/// reads cleanly. `None` otherwise.
pub fn current(root: &Path, now: DateTime<Utc>) -> Option<(State, CapacityFile)> {
    if !auto_switch_on(root) {
        return None;
    }
    let file = read_at(root, now)?;
    Some((effective_state(&file, now), file))
}

/// [`current`] at the present time.
pub fn current_now(root: &Path) -> Option<(State, CapacityFile)> {
    current(root, Utc::now())
}

/// A reset time in local time: `Oct 22 2am`, `Oct 22 2:30am`, or `unknown`.
pub fn format_reset(reset: Option<DateTime<Utc>>) -> String {
    let Some(reset) = reset else {
        return "unknown".to_string();
    };
    let local = reset.with_timezone(&Local);
    let pattern = if local.minute() == 0 {
        "%b %-d %-I%P"
    } else {
        "%b %-d %-I:%M%P"
    };
    local.format(pattern).to_string()
}

/// The one-line warning, or `None` for `Ok`:
/// `⚠ all Claude accounts ≥90% · next reset Oct 22 2am` or
/// `⚠ all Claude accounts at limit · next reset Oct 22 2am`.
pub fn format_line(state: State, threshold: u8, reset: Option<DateTime<Utc>>) -> Option<String> {
    let level = match state {
        State::Ok => return None,
        State::Low => format!("≥{threshold}%"),
        State::Exhausted => "at limit".to_string(),
    };
    Some(format!(
        "⚠ all Claude accounts {level} · next reset {}",
        format_reset(reset)
    ))
}

/// Whether this process runs inside a recipe (`AMPLIHACK_SESSION_DEPTH` > 0).
pub fn nested_session() -> bool {
    std::env::var(DEPTH_ENV)
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .is_some_and(|d| d > 0)
}

/// What a bounded small-file read found.
#[cfg_attr(not(unix), allow(dead_code))]
enum Read {
    Missing,
    /// A symlink, not a regular file, someone else's, writable by others, or
    /// unreadable.
    Refused,
    /// At most the limit plus one byte, so the caller can tell it was cut.
    Bytes(Vec<u8>),
}
