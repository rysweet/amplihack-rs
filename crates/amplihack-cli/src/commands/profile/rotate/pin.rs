//! `<root>/.auto-switch.d/rotate/pin`: the rotation pin (issue #1691).
//! `profile switch NAME` writes it after moving NAME's login into `~/.claude`;
//! `profile switch --none` removes it. While it names a profile, no cycle
//! swaps the login away. This is the only module that reads or writes it.
//!
//! The pin is a separate file, not a ledger field, so a `rotate watch`
//! started before the upgrade still loads the ledger; it ignores the pin
//! until restarted.

use super::ledger::RotateState;
use super::log::msg;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{
    euid, read_bounded_nofollow, write_private_atomic,
};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::pointer::parse_name;
use std::fs;
use std::os::unix::fs::MetadataExt;

/// The longest pin read; a name is far shorter.
const MAX_PIN: u64 = 256;

/// What the pin file says. Never holds the raw bytes of a bad file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinState {
    None,
    Pinned(ProfileName),
    /// The pin names a profile whose directory is gone.
    Missing(ProfileName),
    /// A symlink, a non-file, too large, not this user's, writable by
    /// others, or not a name.
    Unreadable,
}

/// What a cycle does about the pin: go on, go on after logging the line, or
/// stop with the line and record the cycle as held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    Open,
    Ignored(String),
    Held(String),
}

/// Reads the pin. Never fails: anything it cannot trust is `Unreadable`, so
/// a cycle fails closed rather than swapping unexpectedly.
pub fn read(rs: &RotateState) -> PinState {
    let raw = match read_bounded_nofollow(&rs.pin, MAX_PIN) {
        Ok(None) => return PinState::None,
        // Stricter than the ledger: amplihack writes the pin 0600, so any
        // group or other write bit means someone else may have chosen it.
        Ok(Some((raw, m))) if m.uid() == euid() && m.mode() & 0o022 == 0 => raw,
        Ok(Some(_)) | Err(_) => return PinState::Unreadable,
    };
    let Some(name) = parse_name(&raw) else {
        return PinState::Unreadable;
    };
    match fs::symlink_metadata(rs.root.join(name.as_str())) {
        Ok(m) if m.file_type().is_dir() => PinState::Pinned(name),
        _ => PinState::Missing(name),
    }
}

/// Pins rotation to `name`: private, atomic, and replacing a symlink or an
/// empty directory at the path (never what a symlink points to).
pub fn write(rs: &RotateState, name: &ProfileName) -> Result<(), ProfileError> {
    rs.ensure()?;
    if fs::symlink_metadata(&rs.pin).is_ok_and(|m| !m.is_file()) {
        clear(rs)?;
    }
    write_private_atomic(&rs.dir, "pin", format!("{name}\n").as_bytes()).map_err(ProfileError::Io)
}

/// Removes the pin: a file, a symlink (not its target) or an empty
/// directory. Anything else is refused and left alone. `true` when
/// something was removed.
pub fn clear(rs: &RotateState) -> Result<bool, ProfileError> {
    let m = match fs::symlink_metadata(&rs.pin) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    let ft = m.file_type();
    let removed = if ft.is_file() || ft.is_symlink() {
        fs::remove_file(&rs.pin)
    } else if ft.is_dir() {
        fs::remove_dir(&rs.pin)
    } else {
        return Err(not_a_file(rs));
    };
    match removed {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) if ft.is_dir() => Err(not_a_file(rs)),
        Err(e) => Err(e.into()),
    }
}

fn not_a_file(rs: &RotateState) -> ProfileError {
    ProfileError::RotateRefused(format!(
        "pin path {} is not a file; remove it by hand",
        rs.pin.display()
    ))
}

/// The single decision point for the pin: `run_cycle`, `--dry-run` and any
/// other selection path (the credit overflow of #1606) ask it before
/// measuring. A pin wins even over a drifted live login; a pin naming a
/// removed profile is ignored, so a deleted profile cannot freeze rotation.
pub fn gate(state: &PinState) -> Gate {
    match state {
        PinState::None => Gate::Open,
        PinState::Pinned(n) => Gate::Held(msg::held(n)),
        PinState::Unreadable => Gate::Held(msg::PIN_UNREADABLE.to_string()),
        PinState::Missing(n) => Gate::Ignored(msg::pin_missing(n)),
    }
}
