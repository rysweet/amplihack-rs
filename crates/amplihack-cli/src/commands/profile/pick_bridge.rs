//! Claude candidates for `amplihack account pick` (issue #2006).
//!
//! The candidates are the subscription login profiles in the store, except
//! the one whose login is live in `~/.claude` (the rotator manages that
//! one) and any whose directory or login cannot be used. Each is probed as
//! `profile rotate` probes a candidate, with the `profile usage` method,
//! deadline and bound. Only derived figures leave this module: live
//! headroom is 100 minus the fuller of the session and weekly windows, and
//! a profile is usable now when its week is below 100% and its session is
//! below the usable-session threshold.

use super::name::ProfileName;
use super::rotate::decide::{Probed, Reading};
use super::store::ProfileStore;
use super::{capacity_weight, default_live, kind, rotate, run};
use std::path::{Path, PathBuf};

/// One Claude candidate as read.
pub(crate) struct ClaudeRow {
    pub name: String,
    /// The directory `amplihack claude --profile <name>` sets as
    /// `CLAUDE_CONFIG_DIR`.
    pub dir: PathBuf,
    pub live_headroom: Option<f64>,
    pub usable: bool,
    pub note: String,
    pub read_at: i64,
}

/// `p` as a percentage without a needless `.0`.
fn pct(p: f64) -> String {
    if p.fract() == 0.0 {
        format!("{p:.0}%")
    } else {
        format!("{p:.1}%")
    }
}

fn row(name: &ProfileName, dir: PathBuf, p: &Probed, threshold: u8, read_at: i64) -> ClaudeRow {
    let (live_headroom, usable, note) = match p.reading {
        Reading::Measured { session, week } => {
            let w = &p.windows;
            let session_ok = match w.session {
                Some(s) => s.used < f64::from(threshold),
                None => w.no_session_limit,
            };
            let week_ok = w.week.is_some_and(|wk| wk.used < 100.0);
            (
                Some(100.0 - session.max(week)),
                session_ok && week_ok,
                format!("session {}, week {}", pct(session), pct(week)),
            )
        }
        Reading::Exhausted { .. } => (Some(0.0), false, "limit reached".to_string()),
        Reading::Failed if p.unknown => (None, false, "timed out or failed".to_string()),
        Reading::Failed => (None, false, "unreadable usage".to_string()),
    };
    ClaudeRow {
        name: name.as_str().to_string(),
        dir,
        live_headroom,
        usable,
        note,
        read_at,
    }
}

/// Every Claude candidate, probed now. `home` is `$HOME`.
pub(crate) fn claude_rows(home: Option<&Path>) -> Result<Vec<ClaudeRow>, String> {
    let store = ProfileStore::from_env().map_err(|e| e.to_string())?;
    let (live, warning) = default_live::detect(&store, home);
    if let Some(w) = warning {
        eprintln!("{w}");
    }
    let live = live.map(|l| l.name);
    let kinds = kind::Ctx::load(&store, home);
    let mut names = Vec::new();
    for raw in store.list().map_err(|e| e.to_string())? {
        let Ok(name) = ProfileName::parse(&raw) else {
            continue;
        };
        if live.as_ref() == Some(&name) || store.verify_profile_dir(&name).is_err() {
            continue;
        }
        let resolved = kinds.resolve_kind(&store, &name);
        if resolved.kind == kind::Kind::Platform || rotate::login_unusable(&store, &name, &resolved)
        {
            continue;
        }
        names.push(name);
    }
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let binary = run::resolve_claude_binary().map_err(|e| format!("{e:#}"))?;
    let (threshold, warning) =
        capacity_weight::usable_threshold(None, std::env::var_os(capacity_weight::ENV).as_deref());
    if let Some(w) = warning {
        eprintln!("{w}");
    }
    let now = chrono::Utc::now().timestamp();
    let (_, probed) = rotate::measure::measure(
        &store,
        &binary,
        rotate::measure::Live::Platform,
        &names,
        now,
    );
    let read_at = chrono::Utc::now().timestamp();
    let mut rows = Vec::new();
    for (name, p) in &probed {
        match store.resolve(name) {
            Ok(dir) => rows.push(row(name, dir, p, threshold, read_at)),
            Err(e) => eprintln!("warning: account pick: profile {name} skipped: {e}"),
        }
    }
    Ok(rows)
}
