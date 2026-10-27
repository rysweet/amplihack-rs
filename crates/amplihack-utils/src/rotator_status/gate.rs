//! The auto-switch toggle, `<root>/.auto-switch`, and the
//! `AMPLIHACK_CLAUDE_AUTO_SWITCH` override: its grammar, how the file is
//! read, and whether auto-switch is on. Also the profile-name rule. The CLI's
//! `auto_switch::config` and `ProfileName::parse` call these, so the CLI,
//! the limit-resume hook and the capacity notice read the switch one way.

use std::ffi::OsStr;
use std::path::Path;

/// The variable that overrides the toggle file.
pub const TOGGLE_ENV: &str = "AMPLIHACK_CLAUDE_AUTO_SWITCH";
/// The toggle file, in the profile root.
pub const TOGGLE_FILE: &str = ".auto-switch";
/// A larger toggle file is corrupt.
pub const MAX_TOGGLE_BYTES: u64 = 4096;
/// The threshold when the file names none.
pub const DEFAULT_THRESHOLD: u8 = 90;
const MAX_NAME: usize = 64;
const RESERVED: [&str; 6] = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"];

/// A toggle file that parsed. Every name in `exclude` passes
/// [`valid_live_name`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toggle {
    pub enabled: bool,
    pub threshold: u8,
    pub exclude: Vec<String>,
}

/// The toggle file as read from disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToggleFile {
    Missing,
    Valid(Toggle),
    /// Unsafe (a symlink, not a regular file, someone else's), too large,
    /// or not in the grammar.
    Corrupt,
}

/// The override variable's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvToggle {
    /// Unset or empty.
    Unset,
    On,
    Off,
    /// Anything other than `1` or `0`: ignored.
    Invalid,
}

/// Reads the override variable's value: `1` on, `0` off, unset or empty
/// [`EnvToggle::Unset`], anything else [`EnvToggle::Invalid`].
pub fn env_toggle(raw: Option<&OsStr>) -> EnvToggle {
    match raw.map(OsStr::as_encoded_bytes) {
        None | Some(b"") => EnvToggle::Unset,
        Some(b"1") => EnvToggle::On,
        Some(b"0") => EnvToggle::Off,
        Some(_) => EnvToggle::Invalid,
    }
}

/// Whether auto-switch is on for `root` in this process's environment.
pub fn auto_switch_on(root: &Path) -> bool {
    auto_switch_on_from(std::env::var_os(TOGGLE_ENV).as_deref(), root)
}

/// [`auto_switch_on`] with the variable's value passed in: `1` or `0` wins,
/// otherwise a valid file decides and anything else is off.
pub fn auto_switch_on_from(env: Option<&OsStr>, root: &Path) -> bool {
    match env_toggle(env) {
        EnvToggle::On => true,
        EnvToggle::Off => false,
        EnvToggle::Unset | EnvToggle::Invalid => {
            matches!(read_toggle(root), ToggleFile::Valid(t) if t.enabled)
        }
    }
}

/// The toggle file under `root`: read without following a symlink, and
/// only when it is a regular file of the current user.
#[cfg(unix)]
pub fn read_toggle(root: &Path) -> ToggleFile {
    use crate::secure_files::{owned_by_euid, read_bounded_nofollow};
    match read_bounded_nofollow(&root.join(TOGGLE_FILE), MAX_TOGGLE_BYTES) {
        Ok(None) => ToggleFile::Missing,
        Ok(Some((raw, meta))) if owned_by_euid(&meta) => {
            parse_toggle(&raw).map_or(ToggleFile::Corrupt, ToggleFile::Valid)
        }
        _ => ToggleFile::Corrupt,
    }
}

/// Off Unix there is no auto-switch.
#[cfg(not(unix))]
pub fn read_toggle(_root: &Path) -> ToggleFile {
    ToggleFile::Missing
}

/// Parses a toggle file: `key=value` lines, keys `enabled` (required,
/// `on`/`off`), `threshold` (`1`..=`100`) and `exclude` (comma-separated
/// profile names), each at most once, one optional final newline. The
/// error is a short reason that never quotes the input.
pub fn parse_toggle(raw: &[u8]) -> Result<Toggle, &'static str> {
    if raw.len() as u64 > MAX_TOGGLE_BYTES {
        return Err("file too large");
    }
    let text = std::str::from_utf8(raw).map_err(|_| "not valid UTF-8")?;
    let body = text.strip_suffix('\n').unwrap_or(text);
    let (mut enabled, mut threshold, mut exclude) = (None, None, None);
    for line in body.split('\n') {
        let (key, value) = line.split_once('=').ok_or("line without '='")?;
        let repeated = match key {
            "enabled" => enabled
                .replace(match value {
                    "on" => true,
                    "off" => false,
                    _ => return Err("enabled must be on or off"),
                })
                .is_some(),
            "threshold" => threshold.replace(parse_threshold(value)?).is_some(),
            "exclude" => exclude.replace(parse_exclude(value)?).is_some(),
            _ => return Err("unknown key"),
        };
        if repeated {
            return Err("repeated key");
        }
    }
    Ok(Toggle {
        enabled: enabled.ok_or("missing enabled")?,
        threshold: threshold.unwrap_or(DEFAULT_THRESHOLD),
        exclude: exclude.unwrap_or_default(),
    })
}

/// `1`..=`100`, digits only.
pub fn parse_threshold(value: &str) -> Result<u8, &'static str> {
    const WHY: &str = "threshold must be a whole number from 1 to 100";
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(WHY);
    }
    match value.parse::<u8>() {
        Ok(n @ 1..=100) => Ok(n),
        _ => Err(WHY),
    }
}

fn parse_exclude(value: &str) -> Result<Vec<String>, &'static str> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    value
        .split(',')
        .map(|n| {
            valid_live_name(n)
                .then(|| n.to_string())
                .ok_or("invalid profile name in exclude")
        })
        .collect()
}

/// The profile-name rule: 1 to 64 ASCII letters, digits, `.`, `_` or `-`,
/// starting with a letter or digit, no `..`, no trailing `.`, and not a
/// Windows device name.
pub fn valid_live_name(raw: &str) -> bool {
    let mut chars = raw.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    raw.len() <= MAX_NAME
        && first.is_ascii_alphanumeric()
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && !raw.contains("..")
        && !raw.ends_with('.')
        && !is_windows_reserved(raw)
}

fn is_windows_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        return true;
    }
    ["COM", "LPT"].iter().any(|p| {
        stem.strip_prefix(p)
            .is_some_and(|d| d.len() == 1 && matches!(d.as_bytes()[0], b'1'..=b'9'))
    })
}
