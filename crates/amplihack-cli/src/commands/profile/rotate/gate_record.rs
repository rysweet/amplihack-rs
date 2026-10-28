//! The running rotator's overflow gate, for auto-switch (issue #1606).
//!
//! `rotate watch` writes `<root>/.auto-switch.d/rotate/gate.json` when it
//! starts: its pid and the [`Gate`] it decides with (`--overflow-wait`,
//! `--overflow-unknown-reset`, `--session-at`, `--week-at`, as resolved from
//! its flags). Auto-switch reads the gate from there, so a rotator started
//! with `--overflow-wait 7200` gives every Claude session the same two-hour
//! wait, and nothing has to be kept in step by hand.
//!
//! Where auto-switch takes its gate from ([`effective`]):
//! - a rotator runs and `gate.json` names its pid: the recorded gate;
//! - a rotator runs and `gate.json` is missing, damaged, out of range or
//!   names another pid (for example a rotator from an older amplihack,
//!   which never uses overflow): no gate, so auto-switch spends nothing;
//! - the rotator's state cannot be verified: no gate;
//! - no rotator runs: the `AMPLIHACK_ROTATE_OVERFLOW_*`, `..._SESSION_AT`
//!   and `..._WEEK_AT` variables, or their defaults. An invalid one of
//!   these four means no gate.
//!
//! `auto-switch status` prints which of these applies (`overflow gate:`).

use super::daemon::{DaemonState, current_state};
use super::ledger::RotateState;
use super::settings::{Settings, gate_from_env, gate_in_range};
use crate::commands::profile::ProfileError;
use crate::commands::profile::overflow::Gate;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::Path;

const VERSION: u32 = 1;
pub const FILE: &str = "gate.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateRecord {
    pub version: u32,
    /// The rotator that wrote the record.
    pub pid: u32,
    pub overflow_wait: u64,
    pub overflow_unknown_reset: bool,
    pub session_at: u8,
    pub week_at: u8,
}

impl GateRecord {
    pub fn new(pid: u32, s: &Settings) -> Self {
        let g = super::overflow::gate(s);
        Self {
            version: VERSION,
            pid,
            overflow_wait: g.wait,
            overflow_unknown_reset: g.unknown_reset,
            session_at: g.session_at,
            week_at: g.week_at,
        }
    }

    pub fn gate(&self) -> Gate {
        Gate {
            wait: self.overflow_wait,
            unknown_reset: self.overflow_unknown_reset,
            session_at: self.session_at,
            week_at: self.week_at,
        }
    }
}

/// Writes the record for the rotator `pid` running with `s`.
pub fn write(rs: &RotateState, pid: u32, s: &Settings) -> Result<(), ProfileError> {
    let bytes = serde_json::to_vec_pretty(&GateRecord::new(pid, s))
        .map_err(|e| ProfileError::RotateInconsistent(e.to_string()))?;
    rs.write(FILE, &bytes)
}

/// The record: `None` when missing; an error when it is not a private
/// file, does not parse, has another version or holds values out of range.
pub fn load(rs: &RotateState) -> Result<Option<GateRecord>, ProfileError> {
    let bad = |m: &str| ProfileError::RotateInconsistent(format!("rotate gate record: {m}"));
    let Some(raw) = rs.read(&rs.dir.join(FILE), "rotate gate record")? else {
        return Ok(None);
    };
    let r: GateRecord = serde_json::from_slice(&raw).map_err(|_| bad("invalid JSON"))?;
    if r.version != VERSION {
        return Err(bad("unknown version"));
    }
    if !gate_in_range(&r.gate()) {
        return Err(bad("value out of range"));
    }
    Ok(Some(r))
}

/// Where auto-switch's gate came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateSource {
    /// The running rotator's record.
    Rotator(u32),
    /// No rotator runs: the variables or their defaults.
    Environment,
    /// No gate, so no overflow profile is used; the reason.
    Closed(String),
}

/// The gate auto-switch uses under `root`, and where it came from.
pub fn effective(root: &Path) -> (Option<Gate>, GateSource) {
    let rs = RotateState::new(root);
    effective_with(&rs, current_state(&rs), &|k| std::env::var_os(k))
}

/// [`effective`] against another `/proc` and environment, for tests.
#[cfg(test)]
pub fn effective_at(
    root: &Path,
    proc_root: &Path,
    exe_name: &std::ffi::OsStr,
    env: &dyn Fn(&str) -> Option<OsString>,
) -> (Option<Gate>, GateSource) {
    let rs = RotateState::new(root);
    let state = super::daemon::state_at(&rs, proc_root, exe_name);
    effective_with(&rs, state, env)
}

fn effective_with(
    rs: &RotateState,
    state: DaemonState,
    env: &dyn Fn(&str) -> Option<OsString>,
) -> (Option<Gate>, GateSource) {
    let closed = |why: String| (None, GateSource::Closed(why));
    match state {
        DaemonState::Running(pid) => match load(rs) {
            Ok(Some(r)) if r.pid == pid => (Some(r.gate()), GateSource::Rotator(pid)),
            Ok(Some(_)) | Ok(None) => closed(format!(
                "the running rotator (pid {pid}) has not recorded its overflow settings; restart it"
            )),
            Err(e) => closed(e.to_string()),
        },
        DaemonState::Unverifiable => closed("cannot verify the rotator on this host".into()),
        DaemonState::Stopped | DaemonState::Stale => match gate_from_env(env) {
            Ok(g) => (Some(g), GateSource::Environment),
            Err(e) => closed(e.to_string()),
        },
    }
}

/// The `overflow gate:` line of `auto-switch status`.
pub fn status_line(gate: Option<Gate>, source: &GateSource) -> String {
    let values = |g: Gate| {
        format!(
            "wait {}s, unknown reset {}, plan capacity below {}% session and {}% week",
            g.wait,
            if g.unknown_reset { "spends" } else { "waits" },
            g.session_at,
            g.week_at
        )
    };
    match (gate, source) {
        (Some(g), GateSource::Rotator(pid)) => {
            format!("overflow gate: {} (from the rotator, pid {pid})", values(g))
        }
        (Some(g), _) => format!(
            "overflow gate: {} (no rotator runs; from AMPLIHACK_ROTATE_* or defaults)",
            values(g)
        ),
        (None, GateSource::Closed(why)) => format!("overflow gate: closed ({why})"),
        (None, _) => "overflow gate: closed".to_string(),
    }
}
