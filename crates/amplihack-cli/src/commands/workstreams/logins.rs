//! The logins section (D11), built from the rows `profile usage` reads.

use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use serde_json::Value;

use super::render::{LoginRow, Reading};
use crate::commands::profile::usage_cmd::Row;
use crate::commands::profile::usage_parse::{UsageState, UsageWindow};
use crate::commands::recipe::run_record::is_owned_file;

/// Largest profile file inspected for an API-key sign.
const MAX_PROFILE_FILE: u64 = 4 * 1024 * 1024;
/// Mode bits a profile file may not have.
const LOOSE_BITS: u32 = 0o077;
/// The name of the `~/.claude` row.
const HOME_LABEL: &str = "~/.claude";

/// Whether the profile at `dir` uses an API key (Claude Platform) rather
/// than a plan login: `settings.json` has `apiKeyHelper` or sets
/// `env.ANTHROPIC_API_KEY`, or `.claude.json` has `primaryApiKey`. Only the
/// answer leaves this function: never a key, a path or a parse error.
pub(crate) fn is_api_key_profile(dir: &Path) -> bool {
    let has = |value: Option<&Value>| value.is_some_and(|v| !v.is_null());
    let settings = read_json(&dir.join("settings.json"));
    if settings.as_ref().is_some_and(|s| {
        has(s.get("apiKeyHelper")) || has(s.get("env").and_then(|e| e.get("ANTHROPIC_API_KEY")))
    }) {
        return true;
    }
    read_json(&dir.join(".claude.json")).is_some_and(|c| has(c.get("primaryApiKey")))
}

/// The JSON in `path` when it is a private regular file of this user of at
/// most [`MAX_PROFILE_FILE`] bytes.
fn read_json(path: &Path) -> Option<Value> {
    if !is_owned_file(path, LOOSE_BITS) {
        return None;
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_PROFILE_FILE {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_PROFILE_FILE + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_PROFILE_FILE {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

/// One row per profile, in the order of `profiles` (the bool marks an
/// API-key profile), then the `~/.claude` row when `rows` has one. `rows` is
/// `None` when the probe did not finish by the deadline.
pub(crate) fn login_rows(profiles: &[(String, bool)], rows: Option<&[Row]>) -> Vec<LoginRow> {
    let rows = rows.unwrap_or_default();
    let mut out: Vec<LoginRow> = profiles
        .iter()
        .map(|(name, api_key)| {
            let found = rows.iter().find(|r| !r.live_in_home && r.profile == *name);
            LoginRow {
                profile: name.clone(),
                live: found.is_some_and(|r| r.live_in),
                reading: if *api_key {
                    Reading::ApiKey
                } else {
                    found.map_or(
                        Reading::NoReading {
                            state: UsageState::Timeout.as_str().to_string(),
                        },
                        reading,
                    )
                },
            }
        })
        .collect();
    if let Some(home) = rows.iter().find(|r| r.live_in_home) {
        out.push(LoginRow {
            profile: HOME_LABEL.to_string(),
            live: true,
            reading: reading(home),
        });
    }
    out
}

/// Bars for a reading with numbers; `no reading` otherwise.
fn reading(row: &Row) -> Reading {
    let r = &row.report;
    let state = r.state.as_str().to_string();
    let no_numbers = r.session.is_none() && r.week_all.is_none();
    if matches!(r.state, UsageState::Timeout | UsageState::Unparsed) || no_numbers {
        return Reading::NoReading { state };
    }
    let pct = |w: &Option<UsageWindow>| w.as_ref().map(|w| w.used_percentage);
    let resets = [&r.session, &r.week_all]
        .into_iter()
        .flatten()
        .find_map(|w| w.resets_text.as_deref())
        .map(without_zone);
    Reading::Bars {
        session: pct(&r.session),
        week: pct(&r.week_all),
        state,
        resets,
    }
}

/// `4:50pm (Europe/London)` without ` (Europe/London)`.
fn without_zone(text: &str) -> String {
    let text = text.trim();
    match text.rfind(" (") {
        Some(at) if text.ends_with(')') => text[..at].trim_end().to_string(),
        _ => text.to_string(),
    }
}
