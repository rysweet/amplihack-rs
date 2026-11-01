//! The rotation owner marker (issue #1803): `<root>/.auto-switch.d/rotate/owner`,
//! written by a supervised `rotate watch` so that launches defer to the
//! rotator a service manager runs instead of starting a second one. This is
//! the only module that reads, writes or removes the marker. See "Rotate
//! owner" in docs/reference/profile-command.md.
//!
//! The marker is trusted only as a regular file of this user that neither
//! group nor others can write. A damaged or foreign one still counts as an
//! owner (`Unreadable`), so it can never let a stray rotator start. Its
//! bytes are never printed.

use super::daemon;
use super::ledger::{RotateState, trusted};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{check_root_private, read_bounded_nofollow};
use crate::commands::profile::store::ProfileStore;
use crate::commands::profile::usage_cmd::sanitize_display;
use amplihack_utils::rotator_status::is_rotator;
use amplihack_utils::secure_files::owned_by_euid;
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// The marker's file name in the rotate state directory.
pub const FILE: &str = "owner";
/// The unit name when neither the flag nor the cgroup names one.
pub const DEFAULT_UNIT: &str = "amplihack-rotator.service";
const MAX_MARKER: u64 = 4 * 1024;
const MAX_CGROUP: u64 = 4 * 1024;
const MAX_CMDLINE: u64 = 64 * 1024;
const MAX_UNIT: usize = 256;
const CLEAR: &str = "'amplihack profile rotate owner clear'";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    pub unit: String,
    pub pid: u32,
    /// Epoch seconds when it was written; informational only.
    pub since: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerState {
    None,
    Present(Marker),
    /// A marker exists but failed a check; it still counts as an owner.
    Unreadable,
}

/// A unit name the marker may hold: non-empty, at most 256 bytes, no `=`,
/// no control character and no bidi override or isolate.
pub fn valid_unit(unit: &str) -> bool {
    !unit.is_empty()
        && unit.len() <= MAX_UNIT
        && !unit.contains('=')
        && !unit.chars().any(|c| {
            c.is_control() || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
        })
}

/// `unit` and `pid` exactly once each, `pid` a positive decimal without a
/// sign. Unknown keys are ignored and `since` is kept only when it parses.
fn parse(raw: &[u8]) -> Option<Marker> {
    let text = std::str::from_utf8(raw).ok()?;
    let (mut unit, mut pid, mut since) = (None, None, Vec::new());
    for line in text.lines().filter(|l| !l.is_empty()) {
        let (k, v) = line.split_once('=')?;
        match k {
            "unit" if unit.is_none() => unit = Some(v),
            "pid" if pid.is_none() => pid = Some(v),
            "unit" | "pid" => return None,
            "since" => since.push(v),
            _ => {}
        }
    }
    let (unit, pid) = (unit?, pid?);
    if !valid_unit(unit) || pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let pid: u32 = pid.parse().ok().filter(|p| *p > 0)?;
    let since = match since.as_slice() {
        [s] => s.parse().ok(),
        _ => None,
    };
    Some(Marker {
        unit: unit.to_string(),
        pid,
        since,
    })
}

/// The marker under `root`. Creates nothing.
pub fn read(root: &Path) -> OwnerState {
    let path = RotateState::new(root).dir.join(FILE);
    match read_bounded_nofollow(&path, MAX_MARKER) {
        Ok(None) => OwnerState::None,
        Ok(Some((raw, meta))) if trusted(&meta) && meta.mode() & 0o022 == 0 => {
            parse(&raw).map_or(OwnerState::Unreadable, OwnerState::Present)
        }
        _ => OwnerState::Unreadable,
    }
}

/// Writes the marker, private and atomically, into a state directory that
/// has already passed `RotateState::ensure`.
pub fn write(root: &Path, m: &Marker) -> io::Result<()> {
    let mut body = format!("unit={}\npid={}\n", m.unit, m.pid);
    if let Some(since) = m.since {
        body.push_str(&format!("since={since}\n"));
    }
    let rs = RotateState::new(root);
    amplihack_utils::secure_files::write_private_atomic(&rs.dir, FILE, body.as_bytes())
}

/// Removes the marker path itself (a symlink is unlinked, never followed).
/// Returns what was there, or `None` when nothing was.
pub fn clear(root: &Path) -> io::Result<Option<OwnerState>> {
    let was = read(root);
    match std::fs::remove_file(RotateState::new(root).dir.join(FILE)) {
        Ok(()) if was == OwnerState::None => Ok(Some(OwnerState::Unreadable)),
        Ok(()) => Ok(Some(was)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Whether the marker's pid is a verified rotator of this user running the
/// binary named `exe_name`, per `proc_root`.
pub fn alive(m: &Marker, proc_root: &Path, exe_name: &OsStr) -> bool {
    let dir = proc_root.join(m.pid.to_string());
    std::fs::symlink_metadata(&dir).is_ok_and(|meta| meta.is_dir() && owned_by_euid(&meta))
        && matches!(read_bounded_nofollow(&dir.join("cmdline"), MAX_CMDLINE),
            Ok(Some((raw, _))) if is_rotator(&raw, Some(exe_name)))
}

/// The last `*.service` component of a cgroup path line, if it is a valid
/// unit name.
fn service_in(path: &str) -> Option<&str> {
    path.rsplit('/')
        .find(|c| c.ends_with(".service") && valid_unit(c))
}

/// The unit name: `flag`, then the unit in `<proc_root>/self/cgroup`
/// (preferring the cgroup v2 and `name=systemd` lines), then
/// [`DEFAULT_UNIT`].
pub fn unit_name(flag: Option<&str>, proc_root: &Path) -> String {
    if let Some(f) = flag.filter(|f| valid_unit(f)) {
        return f.to_string();
    }
    let raw = match read_bounded_nofollow(&proc_root.join("self").join("cgroup"), MAX_CGROUP) {
        Ok(Some((raw, _))) => raw,
        _ => return DEFAULT_UNIT.into(),
    };
    let text = String::from_utf8_lossy(&raw);
    let mut fallback = None;
    for line in text.lines() {
        let mut parts = line.splitn(3, ':');
        let (Some(_), Some(controllers), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let Some(unit) = service_in(path) else {
            continue;
        };
        if controllers.is_empty() || controllers == "name=systemd" {
            return unit.to_string();
        }
        fallback.get_or_insert_with(|| unit.to_string());
    }
    fallback.unwrap_or_else(|| DEFAULT_UNIT.into())
}

/// Whether a (non-`--spawned`) `watch` records itself as the owner: given
/// `--supervised[=UNIT]`, or run by systemd (`INVOCATION_ID` non-empty).
pub fn supervised(flag: &Option<Option<String>>, env: &dyn Fn(&str) -> Option<OsString>) -> bool {
    flag.is_some() || env("INVOCATION_ID").is_some_and(|v| !v.is_empty())
}

fn shown(unit: &str) -> String {
    sanitize_display(unit, false)
}

/// The whole `owner:` status line. Reads the marker only under a private
/// root; creates nothing.
pub fn describe(root: &Path, proc_root: &Path, exe_name: &OsStr) -> String {
    if check_root_private(root).is_err() {
        return "owner: not read (root is not private)".into();
    }
    let hint = format!("run {CLEAR} to release");
    match read(root) {
        OwnerState::None => "owner: none".into(),
        OwnerState::Present(m) if alive(&m, proc_root, exe_name) => format!(
            "owner: {} pid {} (running); autostart defers to it",
            shown(&m.unit),
            m.pid
        ),
        OwnerState::Present(m) => format!(
            "owner: {} pid {} (not running); autostart defers to it; {hint}",
            shown(&m.unit),
            m.pid
        ),
        OwnerState::Unreadable => format!("owner: unreadable; autostart defers to it; {hint}"),
    }
}

/// What `autostart_line` appends for the marker, or for a root that is not
/// private (whose marker is not read).
pub fn deferral_suffix(root: &Path) -> String {
    if check_root_private(root).is_err() {
        return " (skipped: root is not private)".into();
    }
    match read(root) {
        OwnerState::None => String::new(),
        OwnerState::Present(m) => format!(" (deferring to {})", shown(&m.unit)),
        OwnerState::Unreadable => " (deferring to an unreadable owner marker)".into(),
    }
}

/// The autostart owner gate: `Some(running)` when a marker exists, after one
/// line to `err` unless its owner is running. The caller has checked the
/// root and the euid. Creates nothing.
pub fn defer(root: &Path, proc_root: &Path, exe_name: &OsStr, err: &mut dyn Write) -> Option<bool> {
    let tail = format!("run {CLEAR} to let launches start the rotator");
    // The launch goes on whether or not its stderr takes the line.
    match read(root) {
        OwnerState::None => None,
        OwnerState::Present(m) if alive(&m, proc_root, exe_name) => Some(true),
        OwnerState::Present(m) => {
            let unit = shown(&m.unit);
            let _ = writeln!(
                err,
                "amplihack: rotation is owned by {unit}, which is not running; {tail}"
            );
            Some(false)
        }
        OwnerState::Unreadable => {
            let _ = writeln!(
                err,
                "amplihack: the rotation owner marker is unreadable; {tail}"
            );
            Some(false)
        }
    }
}

/// The note an unsupervised `watch` prints when a marker exists.
pub fn watch_note(root: &Path) -> Option<String> {
    let rest = "this watch runs anyway and leaves the owner marker in place";
    match read(root) {
        OwnerState::None => None,
        OwnerState::Present(m) => Some(format!(
            "rotate: rotation is owned by {}; {rest}",
            shown(&m.unit)
        )),
        OwnerState::Unreadable => {
            Some(format!("rotate: an unreadable owner marker exists; {rest}"))
        }
    }
}

/// `--supervised=UNIT` must name a unit the marker can hold.
pub fn check_flag(flag: &Option<Option<String>>) -> Result<(), ProfileError> {
    match flag {
        Some(Some(u)) if !valid_unit(u) => Err(ProfileError::RotateRefused(
            "--supervised=UNIT needs a unit name of 1 to 256 bytes without '=', control or bidi characters"
                .into(),
        )),
        _ => Ok(()),
    }
}

/// `rotate owner clear` without the printing: refuses under a root that is
/// not private, since another user could then swap the state directory for
/// a symlink, and otherwise [`clear`]s.
pub fn clear_checked(root: &Path) -> Result<Option<OwnerState>, ProfileError> {
    check_root_private(root).map_err(|e| {
        ProfileError::RotateRefused(format!(
            "profile root {e}; the owner marker was left in place"
        ))
    })?;
    clear(root)
        .map_err(|e| ProfileError::RotateRefused(format!("could not remove the owner marker: {e}")))
}

/// `rotate owner [clear]`: prints the owner line, or removes the marker.
pub fn cli(store: &ProfileStore, clear_it: bool) -> Result<(), ProfileError> {
    let root = store.root();
    if !clear_it {
        let exe = daemon::exe_name().unwrap_or_default();
        println!("{}", describe(root, Path::new("/proc"), &exe));
        return Ok(());
    }
    match clear_checked(root)? {
        None | Some(OwnerState::None) => println!("owner: none"),
        Some(OwnerState::Present(m)) => println!("owner: cleared (was {})", shown(&m.unit)),
        Some(OwnerState::Unreadable) => println!("owner: cleared (was unreadable)"),
    }
    Ok(())
}
