//! Issue #2013: helpers shared by the `tests_issue_2013_*` modules.
//!
//! Time comes only from the injected clock (`Ctx.clock`, D12). A test sets
//! the clock with [`set_now`] and installs [`clock`] in its context; the
//! clock is thread-local, so tests on parallel threads never see each
//! other's time, and nothing sleeps or waits on the wall clock.
//!
//! Every fixture is a temporary HOME and profile root (`tests_swap::env`)
//! whose login files carry a synthetic identity
//! (`tests_fixture_identity`); no real credential, `~/.claude`,
//! `~/.claude.json` or `~/.claude-profiles` is ever read.

use super::cycle::{Ctx, MeasureFn};
use super::decide::{Measured, Probed, Reading, Win, Windows};
use super::settings::Settings;
use super::tests_revalidate_cause::ctx_with;
use super::tests_swap::{Env, n, private};
use crate::commands::profile::auto_switch::kit::State;
use crate::commands::profile::expected;
use std::cell::Cell;
use std::fs;
use std::path::Path;

thread_local! {
    static NOW: Cell<i64> = const { Cell::new(0) };
}

/// Sets the time [`clock`] returns on this thread.
pub(super) fn set_now(t: i64) {
    NOW.with(|c| c.set(t));
}

/// The injected clock: the time last given to [`set_now`] on this thread.
pub(super) fn clock() -> i64 {
    NOW.with(Cell::get)
}

/// The defaults of `rotate`, written out so a test can see them.
pub(super) fn settings() -> Settings {
    Settings {
        session_at: 85,
        week_at: 90,
        margin: 5,
        upgrade_margin: 30,
        hold: 3600,
        interval: 600,
        overflow_wait: 1800,
        overflow_unknown_reset: false,
    }
}

fn windows(session: f64, week: f64) -> Windows {
    Windows {
        session: Some(Win {
            used: session,
            resets_at: None,
        }),
        week: Some(Win {
            used: week,
            resets_at: None,
        }),
        no_session_limit: false,
    }
}

/// A measured probe read at `observed_at`, or in this cycle (`None`).
/// Built on `Probed::failed()` so fields added by #2013 take their defaults.
pub(super) fn rd(session: f64, week: f64, observed_at: Option<i64>) -> Probed {
    Probed {
        reading: Reading::Measured { session, week },
        windows: windows(session, week),
        unknown: false,
        observed_at,
        ..Probed::failed()
    }
}

/// An exhausted probe that keeps both figures, read in this cycle.
pub(super) fn exh(session: f64, week: f64) -> Probed {
    Probed {
        reading: Reading::Exhausted {
            session: Some(session),
            week: Some(week),
        },
        windows: windows(session, week),
        unknown: false,
        ..Probed::failed()
    }
}

/// A probe that failed for `reason` (error kind and status only).
pub(super) fn failed(reason: &str) -> Probed {
    let mut p = Probed::failed();
    p.failure = Some(reason.to_string());
    p
}

/// A read started at `started_at` that had not finished.
pub(super) fn running(started_at: i64) -> Probed {
    let mut p = Probed::failed();
    p.still_running = true;
    p.started_at = Some(started_at);
    p
}

/// A measured reading for the pure ranking functions.
pub(super) fn m(name: &str, session: f64, week: f64) -> Measured {
    Measured::new(n(name), &rd(session, week, None))
}

/// A watch context on `e` with `measure`, the injected clock set to `t`,
/// and settings `s`.
pub(super) fn ctx_at(e: &Env, measure: MeasureFn, s: Settings, t: i64) -> Ctx {
    set_now(t);
    let mut c = ctx_with(e, measure);
    c.settings = s;
    c.clock = clock;
    c
}

/// Records `name` as holding the account `{uuid, email}`, as an accepted
/// `profile login` would.
pub(super) fn record(root: &Path, name: &str, uuid: &str, email: &str) {
    let account = super::identify::Account::from_block(
        &serde_json::json!({"accountUuid": uuid, "emailAddress": email}),
    )
    .unwrap();
    expected::record(&State::new(root).unwrap(), &n(name), &account).unwrap();
}

/// Writes `home/.claude.json` with `oauthAccount` naming `{uuid, email}`
/// (or no block for `None`), keeping the fixture's other keys.
pub(super) fn home_block(home: &Path, block: Option<(&str, &str)>) {
    let path = home.join(".claude.json");
    let mut v: serde_json::Value = fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    match block {
        Some((uuid, email)) => {
            v["oauthAccount"] = serde_json::json!({"accountUuid": uuid, "emailAddress": email});
        }
        None => {
            v.as_object_mut().unwrap().remove("oauthAccount");
        }
    }
    private(&path, &v.to_string());
}

/// The `oauthAccount.accountUuid` named by `home/.claude.json`.
pub(super) fn home_uuid(home: &Path) -> Option<String> {
    let v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.join(".claude.json")).ok()?).ok()?;
    v["oauthAccount"]["accountUuid"]
        .as_str()
        .map(str::to_string)
}

/// The rotate log's lines without their timestamps.
pub(super) fn lines(e: &Env) -> Vec<String> {
    fs::read_to_string(&e.rs.log)
        .unwrap_or_default()
        .lines()
        .map(|l| l.split_once(' ').map_or(l, |(_, rest)| rest).to_string())
        .collect()
}
