//! The logins section (issue #1710): one row per Claude profile, from the
//! probes `amplihack profile usage` runs, collected until a deadline.

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Instant;

use serde::Deserialize;

use super::meta;
use super::sources::UsageStream;
use crate::commands::profile::usage_parse::{UsageReport, UsageState, UsageWindow};

/// Settings files larger than this are not read.
const MAX_SETTINGS_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LoginCell {
    /// Session and week percentages, floored and clamped to 0–100.
    Reading {
        session: Option<u8>,
        week: Option<u8>,
        state: String,
        reset: Option<String>,
    },
    /// No usable reading; holds the state (`timeout`, `unparsed`, ...).
    NoReading(String),
    /// An API-key profile: billed per token, no plan limits.
    ApiKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoginRow {
    pub profile: String,
    pub live: bool,
    pub cell: LoginCell,
}

#[derive(Deserialize)]
struct ClaudeJson {
    #[serde(rename = "primaryApiKey")]
    primary_api_key: Option<String>,
}

#[derive(Deserialize)]
struct SettingsJson {
    #[serde(rename = "apiKeyHelper")]
    api_key_helper: Option<serde::de::IgnoredAny>,
    env: Option<SettingsEnv>,
}

#[derive(Deserialize)]
struct SettingsEnv {
    #[serde(rename = "ANTHROPIC_API_KEY")]
    anthropic_api_key: Option<serde::de::IgnoredAny>,
}

/// Whether the profile in `dir` holds an API-key credential: a non-empty
/// `primaryApiKey` in `.claude.json`, or an `apiKeyHelper` or
/// `env.ANTHROPIC_API_KEY` entry in `settings.json`. The profile's name plays
/// no part, and no key is kept.
pub(crate) fn is_api_key_profile(dir: &Path) -> bool {
    let primary = meta::read_private(&dir.join(".claude.json"), MAX_SETTINGS_BYTES)
        .and_then(|b| serde_json::from_slice::<ClaudeJson>(&b).ok())
        .and_then(|c| c.primary_api_key)
        .is_some_and(|k| !k.is_empty());
    if primary {
        return true;
    }
    meta::read_private(&dir.join("settings.json"), MAX_SETTINGS_BYTES)
        .and_then(|b| serde_json::from_slice::<SettingsJson>(&b).ok())
        .is_some_and(|s| {
            s.api_key_helper.is_some() || s.env.is_some_and(|e| e.anthropic_api_key.is_some())
        })
}

/// One probe's report as a cell. A state that can carry numbers, with at
/// least one finite percentage, is a reading; anything else is no reading.
pub(crate) fn cell_from(report: &UsageReport) -> LoginCell {
    let state = report.state.as_str().to_string();
    let numbered = matches!(
        report.state,
        UsageState::Ok
            | UsageState::SessionExhausted
            | UsageState::WeeklyExhausted
            | UsageState::SpendLimit
    );
    let session = report.session.as_ref().and_then(pct);
    let week = report.week_all.as_ref().and_then(pct);
    if !numbered || (session.is_none() && week.is_none()) {
        return LoginCell::NoReading(state);
    }
    let resets: Vec<String> = [("session", &report.session), ("week", &report.week_all)]
        .into_iter()
        .filter_map(|(name, w)| {
            let text = w.as_ref()?.resets_text.as_deref()?;
            Some(format!("{name} resets {text}"))
        })
        .collect();
    LoginCell::Reading {
        session,
        week,
        state,
        reset: (!resets.is_empty()).then(|| resets.join(", ")),
    }
}

fn pct(w: &UsageWindow) -> Option<u8> {
    let p = w.used_percentage;
    p.is_finite().then(|| p.clamp(0.0, 100.0).floor() as u8)
}

/// Wait for the stream's reports until each probed profile has one or the
/// deadline passes, then stop the probes still running. A profile with no
/// report by then shows `no reading (timeout)`. Rows are in the stream's
/// profile order. Never joins a probe.
pub(crate) fn collect(stream: UsageStream, deadline: Instant) -> Vec<LoginRow> {
    let UsageStream {
        profiles,
        results,
        cancel,
    } = stream;
    let index: HashMap<&str, usize> = profiles
        .iter()
        .enumerate()
        .map(|(i, p)| (p.name.as_str(), i))
        .collect();
    let mut cells: Vec<Option<LoginCell>> = profiles
        .iter()
        .map(|p| p.api_key.then_some(LoginCell::ApiKey))
        .collect();
    let mut pending = cells.iter().filter(|c| c.is_none()).count();
    let mut lost = false;
    while pending > 0 {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match results.recv_timeout(left) {
            Ok((name, report)) => {
                if let Some(&i) = index.get(name.as_str())
                    && cells[i].is_none()
                {
                    cells[i] = Some(cell_from(&report));
                    pending -= 1;
                }
            }
            Err(RecvTimeoutError::Timeout) => break,
            Err(RecvTimeoutError::Disconnected) => {
                lost = true;
                break;
            }
        }
    }
    if pending > 0 {
        cancel();
    }
    let missing = if lost { "error" } else { "timeout" };
    profiles
        .into_iter()
        .zip(cells)
        .map(|(p, cell)| LoginRow {
            profile: p.name,
            live: p.live,
            cell: cell.unwrap_or_else(|| LoginCell::NoReading(missing.to_string())),
        })
        .collect()
}
