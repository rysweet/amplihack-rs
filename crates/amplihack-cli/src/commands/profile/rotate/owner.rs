//! The rotation owner (issue #1803): `owner.json`, which a rotator run by a
//! service manager writes so that launches leave rotation to that service,
//! even while it is stopped. See "Rotation owner" in
//! docs/reference/profile-command.md.
//!
//! The marker is advisory. Nothing here signals a process or removes the
//! file, and its pid is never trusted by itself: the owner is running only
//! when the verified rotator has that pid. Anything that cannot be trusted is
//! `Damaged`, which starts nothing and prints no text from the file.

use super::daemon::{DaemonState, state_at};
use super::ledger::RotateState;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{euid, read_bounded_nofollow};
use crate::commands::profile::usage_cmd::sanitize_display;
use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, OsString};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// The unit named when no better name is known.
pub const DEFAULT_UNIT: &str = "amplihack-rotator.service";
const FILE: &str = "owner.json";
const VERSION: u32 = 1;
const MAX_UNIT: usize = 256;
/// How many characters of a rejected `--supervised` value an error shows.
const MAX_SHOWN: usize = 256;
const MAX_CGROUP: u64 = 64 * 1024;
const MAX_MARKER: u64 = 64 * 1024;
const SERVICE_SUFFIX: &str = ".service";
const INVOCATION_ENV: &str = "INVOCATION_ID";

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Marker {
    version: u32,
    unit: String,
    pid: u32,
}

/// What `owner.json` says about rotation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerState {
    /// No marker.
    None,
    /// The verified rotator is the marker's pid.
    Running { unit: String, pid: u32 },
    /// The marker is good, but its rotator is not running.
    Down { unit: String, pid: u32 },
    /// The marker exists and cannot be trusted.
    Damaged,
}

impl OwnerState {
    /// The unit a launch warns about, when it warns at all. A damaged marker
    /// gives the fixed default, never text from the file.
    pub fn warn_unit(&self) -> Option<&str> {
        match self {
            Self::None | Self::Running { .. } => None,
            Self::Down { unit, .. } => Some(unit),
            Self::Damaged => Some(DEFAULT_UNIT),
        }
    }
}

fn marker_path(rs: &RotateState) -> PathBuf {
    rs.dir.join(FILE)
}

/// At most 256 bytes of ASCII letters, digits and `-_.@:`: a non-empty name
/// followed by `.service`.
pub fn valid_unit(unit: &str) -> bool {
    unit.len() > SERVICE_SUFFIX.len()
        && unit.len() <= MAX_UNIT
        && unit.ends_with(SERVICE_SUFFIX)
        && unit
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.@:".contains(&b))
}

/// The last valid `*.service` path part of a `/proc/self/cgroup` text.
pub fn unit_from_cgroup(raw: &[u8]) -> Option<String> {
    String::from_utf8_lossy(raw)
        .lines()
        .filter_map(|line| line.splitn(3, ':').nth(2))
        .flat_map(|path| path.split('/'))
        .rfind(|part| valid_unit(part))
        .map(String::from)
}

/// The unit that `watch` should record, or `None` when neither the flag
/// (`supervised`: `None` without it, `Some(None)` bare, `Some(Some(u))` with
/// a value) nor a non-empty `INVOCATION_ID` asks for ownership. Only the
/// variable's presence is read. Writes nothing.
pub fn claim(
    supervised: Option<Option<&str>>,
    env: &dyn Fn(&str) -> Option<OsString>,
    proc_root: &Path,
) -> Result<Option<String>, ProfileError> {
    if let Some(Some(unit)) = supervised {
        if !valid_unit(unit) {
            let head: String = unit.chars().take(MAX_SHOWN).collect();
            return Err(ProfileError::RotateConfig(format!(
                "invalid rotate setting --supervised {head:?}: \
                 expected a unit name ending in .service"
            )));
        }
        return Ok(Some(unit.to_string()));
    }
    if supervised.is_none() && !env(INVOCATION_ENV).is_some_and(|v| !v.is_empty()) {
        return Ok(None);
    }
    // An unreadable cgroup only costs the name, so it falls back.
    let unit = read_bounded_nofollow(&proc_root.join("self").join("cgroup"), MAX_CGROUP)
        .ok()
        .flatten()
        .and_then(|(raw, _)| unit_from_cgroup(&raw));
    Ok(Some(unit.unwrap_or_else(|| DEFAULT_UNIT.to_string())))
}

/// Records `unit` and `pid` as the owner: private and atomic, replacing a
/// symlink rather than following it.
pub fn write(rs: &RotateState, unit: &str, pid: u32) -> Result<(), ProfileError> {
    let marker = Marker {
        version: VERSION,
        unit: unit.to_string(),
        pid,
    };
    let bytes = serde_json::to_vec_pretty(&marker)
        .map_err(|e| ProfileError::RotateInconsistent(format!("rotation owner: {e}")))?;
    rs.write(FILE, &bytes)
}

/// The marker's state. Never fails: a marker that cannot be trusted is
/// `Damaged`. A rotator that cannot be verified counts as not running.
pub fn check(rs: &RotateState, proc_root: &Path, exe_name: &OsStr) -> OwnerState {
    // Stricter than `RotateState::read`, which accepts group write for a
    // private group: anyone else who can write this file can stop autostart.
    let raw = match read_bounded_nofollow(&marker_path(rs), MAX_MARKER) {
        Ok(None) => return OwnerState::None,
        Ok(Some((raw, m))) if m.uid() == euid() && m.mode() & 0o022 == 0 => raw,
        _ => return OwnerState::Damaged,
    };
    let marker = match serde_json::from_slice::<Marker>(&raw) {
        Ok(m) if m.version == VERSION && m.pid != 0 && valid_unit(&m.unit) => m,
        _ => return OwnerState::Damaged,
    };
    let Marker { unit, pid, .. } = marker;
    match state_at(rs, proc_root, exe_name) {
        DaemonState::Running(p) if p == pid => OwnerState::Running { unit, pid },
        _ => OwnerState::Down { unit, pid },
    }
}

/// The `owner:` and `deferring:` lines of `rotate status`; none without a
/// marker. A launch defers to any marker, so only autostart being off says no.
pub fn status_lines(rs: &RotateState, state: &OwnerState, autostart_on: bool) -> Vec<String> {
    let owner = match state {
        OwnerState::None => return Vec::new(),
        OwnerState::Running { unit, pid } => format!("owner: {unit} (pid {pid}, running)"),
        OwnerState::Down { unit, pid } => format!("owner: {unit} (pid {pid}, not running)"),
        OwnerState::Damaged => format!(
            "owner: unreadable marker {}",
            sanitize_display(&marker_path(rs).display().to_string(), false)
        ),
    };
    let deferring = if autostart_on {
        "deferring: yes"
    } else {
        "deferring: no (autostart off)"
    };
    vec![owner, deferring.to_string()]
}
