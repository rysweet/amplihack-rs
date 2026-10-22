//! Whether auto-switch is on, with the meaning of the CLI's
//! `auto_switch::config::effective(root).on`: `AMPLIHACK_CLAUDE_AUTO_SWITCH`
//! set to `1` or `0` wins; otherwise `<root>/.auto-switch` decides, and a
//! missing, unsafe or malformed file means off. Also the profile-name rule
//! (the CLI's `ProfileName::parse`) used to check names read from disk.

use std::ffi::OsStr;
use std::path::Path;

/// The variable that overrides the toggle file.
pub const TOGGLE_ENV: &str = "AMPLIHACK_CLAUDE_AUTO_SWITCH";
/// The toggle file, in the profile root.
const TOGGLE_FILE: &str = ".auto-switch";
const MAX_TOGGLE_BYTES: u64 = 4096;
const MAX_NAME: usize = 64;
const RESERVED: [&str; 6] = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"];

/// Whether auto-switch is on for `root` in this process's environment.
pub fn auto_switch_on(root: &Path) -> bool {
    auto_switch_on_from(std::env::var_os(TOGGLE_ENV).as_deref(), root)
}

/// [`auto_switch_on`] with the variable's value passed in. An unrecognised
/// value is ignored, as in the CLI (which also warns).
pub fn auto_switch_on_from(env: Option<&OsStr>, root: &Path) -> bool {
    match env.map(OsStr::as_encoded_bytes) {
        Some(b"1") => true,
        Some(b"0") => false,
        _ => read_toggle(root).unwrap_or(false),
    }
}

/// `Some(enabled)` for a valid toggle file owned by the current user;
/// `None` when it is missing, unsafe or malformed.
#[cfg(unix)]
fn read_toggle(root: &Path) -> Option<bool> {
    use crate::secure_files::{owned_by_euid, read_bounded_nofollow};
    match read_bounded_nofollow(&root.join(TOGGLE_FILE), MAX_TOGGLE_BYTES) {
        Ok(Some((raw, meta))) if owned_by_euid(&meta) => parse_enabled(&raw),
        _ => None,
    }
}

#[cfg(not(unix))]
fn read_toggle(_root: &Path) -> Option<bool> {
    None
}

/// The `enabled` value of a toggle file, checked with the CLI grammar:
/// `key=value` lines, keys `enabled` (required, `on`/`off`), `threshold`
/// (`1`..=`100`) and `exclude` (comma-separated profile names), each at most
/// once, one optional final newline.
fn parse_enabled(raw: &[u8]) -> Option<bool> {
    if raw.len() as u64 > MAX_TOGGLE_BYTES {
        return None;
    }
    let text = std::str::from_utf8(raw).ok()?;
    let body = text.strip_suffix('\n').unwrap_or(text);
    let (mut enabled, mut threshold, mut exclude) = (None, false, false);
    for line in body.split('\n') {
        let (key, value) = line.split_once('=')?;
        let repeated = match key {
            "enabled" => enabled
                .replace(match value {
                    "on" => true,
                    "off" => false,
                    _ => return None,
                })
                .is_some(),
            "threshold" => {
                if !valid_threshold(value) {
                    return None;
                }
                std::mem::replace(&mut threshold, true)
            }
            "exclude" => {
                if !valid_exclude(value) {
                    return None;
                }
                std::mem::replace(&mut exclude, true)
            }
            _ => return None,
        };
        if repeated {
            return None;
        }
    }
    enabled
}

fn valid_threshold(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|b| b.is_ascii_digit())
        && matches!(value.parse::<u8>(), Ok(1..=100))
}

fn valid_exclude(value: &str) -> bool {
    value.is_empty() || value.split(',').all(valid_live_name)
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
