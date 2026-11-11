//! The logins section (D11), read from `amplihack profile usage --json` and
//! drawn by `bars.rs`.
//!
//! `profile usage` owns every rule about a login: which profiles exist,
//! which are API-key (Claude Platform) profiles (#1700, `kind`), which are
//! credit-backed overflow (`overflow`), which login is live in `~/.claude`
//! (`live_in`), and the probe itself. This module only reads its stable
//! JSON interface, so the two commands cannot disagree about a login.

use serde::Deserialize;

use super::bars::{LoginRow, Reading, without_zone};

/// `kind` of an API-key profile in `profile usage --json`.
const API_KEY_KIND: &str = "api-key";
/// Shown in place of rows when the output cannot be read. Fixed text: it
/// never holds a path or the output itself.
pub(crate) const UNREADABLE: &str =
    "amplihack profile usage --json gave output that is not a row list";

/// One element of the `profile usage --json` array. Fields this section
/// does not use are ignored.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct UsageRow {
    pub profile: String,
    /// `~/.claude` when the login is live there, else `null`.
    #[serde(default)]
    pub live_in: Option<String>,
    #[serde(default)]
    pub overflow: bool,
    /// `subscription` or `api-key`.
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub api_key_blocked: Option<ApiKeyBlock>,
    #[serde(default)]
    pub overflow_blocked_until: Option<i64>,
    pub state: String,
    #[serde(default)]
    pub session: Option<Window>,
    #[serde(default)]
    pub week_all: Option<Window>,
}

/// An active API-key block from the rotator.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ApiKeyBlock {
    pub reason: String,
    pub until: i64,
}

/// A usage window as `profile usage --json` prints it.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Window {
    pub used_percentage: f64,
    #[serde(default)]
    pub resets_text: Option<String>,
}

/// The login rows of `profile usage --json` output, in its order (by
/// capacity, the `~/.claude` row included). `Err` is [`UNREADABLE`].
pub(crate) fn parse(json: &[u8]) -> Result<Vec<LoginRow>, String> {
    let rows: Vec<UsageRow> = serde_json::from_slice(json).map_err(|_| UNREADABLE.to_string())?;
    Ok(rows.iter().map(login_row).collect())
}

/// One login row: API-key and overflow profiles have no plan bars; a
/// probe with no numbers shows its state.
pub(crate) fn login_row(row: &UsageRow) -> LoginRow {
    LoginRow {
        profile: row.profile.clone(),
        live: row.live_in.is_some(),
        reading: reading(row),
    }
}

fn reading(row: &UsageRow) -> Reading {
    if row.kind == API_KEY_KIND {
        return match &row.api_key_blocked {
            Some(block) => Reading::Unavailable {
                kind: "API key",
                state: block.reason.clone(),
                blocked_until: Some(block.until),
            },
            None if !matches!(row.state.as_str(), "ok" | "api_key") => Reading::Unavailable {
                kind: "API key",
                state: row.state.clone(),
                blocked_until: None,
            },
            None => Reading::ApiKey,
        };
    }
    if row.overflow {
        return match row.overflow_blocked_until {
            Some(until) => Reading::Unavailable {
                kind: "overflow",
                state: if row.state == "ok" {
                    "spend limit".into()
                } else {
                    row.state.clone()
                },
                blocked_until: Some(until),
            },
            None if !matches!(
                row.state.as_str(),
                "ok" | "session_exhausted" | "weekly_exhausted"
            ) =>
            {
                Reading::Unavailable {
                    kind: "overflow",
                    state: row.state.clone(),
                    blocked_until: None,
                }
            }
            None => Reading::Overflow,
        };
    }
    let no_numbers = row.session.is_none() && row.week_all.is_none();
    if matches!(row.state.as_str(), "timeout" | "unparsed") || no_numbers {
        return Reading::NoReading {
            state: row.state.clone(),
        };
    }
    let pct = |w: &Option<Window>| w.as_ref().map(|w| w.used_percentage);
    let resets = |w: &Option<Window>| {
        w.as_ref()
            .and_then(|w| w.resets_text.as_deref())
            .map(without_zone)
    };
    Reading::Bars {
        session: pct(&row.session),
        week: pct(&row.week_all),
        state: row.state.clone(),
        session_resets: resets(&row.session),
        week_resets: resets(&row.week_all),
    }
}
