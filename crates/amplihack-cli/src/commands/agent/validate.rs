//! Every field rule for `amplihack agent` (R2.5, S7, S8, S8b), and `entry`,
//! the whole-entry check that `load` (C1) and `update` (C2) apply. No I/O
//! apart from the launch directory's existence check. A rule never repeats
//! the value it refuses; callers put the subject (`--dir`, `NAME`,
//! `discovered --model`) in front of the rule text.

use super::env_record;
use super::registry::{Entry, parse_ts};
use amplihack_utils::agent_marker::is_conversation_id;
use std::path::{Path, PathBuf};

const NAME_RULE: &str =
    "must be 1 to 64 letters, digits, '.', '_' or '-', starting with a letter or digit";
const ID_RULE: &str = "must be 1 to 64 letters, digits or '-', starting with a letter or digit";
const PROFILE_RULE: &str =
    "names match [A-Za-z0-9][A-Za-z0-9._-]{0,63}, without '..' or a trailing '.'";
const ENV_RULE: &str = "value must be at most 4096 bytes, with no newline or NUL";
const MAX_SOCKET_BYTES: usize = 107;
const MAX_OPTION_BYTES: usize = 256;
const MAX_ENV_BYTES: usize = 4096;

/// `^[A-Za-z0-9][A-Za-z0-9_.-]{0,63}$`.
pub fn agent_name(s: &str) -> Result<(), String> {
    leading_alnum(s, 64, |c| {
        c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')
    })
    .then_some(())
    .ok_or_else(|| NAME_RULE.to_string())
}

/// [`is_conversation_id`] (S7), the rule the SessionEnd hook applies too,
/// with its rule text.
pub fn conversation_id(s: &str) -> Result<(), String> {
    is_conversation_id(s)
        .then_some(())
        .ok_or_else(|| ID_RULE.to_string())
}

fn leading_alnum(s: &str, max: usize, rest: impl Fn(char) -> bool) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric()) && s.len() <= max && chars.all(rest)
}

/// 1 to 64 characters without `:`, `.`, `#` or a control character: tmux
/// rewrites `:` and `.` in session names and expands `#` formats (D11).
pub fn session_name(s: &str) -> Result<(), String> {
    tmux_name(s, &[':', '.', '#']).map_err(|()| {
        "must be 1 to 64 characters, without ':', '.', '#' or control characters".into()
    })
}

/// 1 to 64 characters without `#` or a control character.
pub fn window_name(s: &str) -> Result<(), String> {
    tmux_name(s, &['#'])
        .map_err(|()| "must be 1 to 64 characters, without '#' or control characters".into())
}

fn tmux_name(s: &str, banned: &[char]) -> Result<(), ()> {
    let n = s.chars().count();
    let ok = (1..=64).contains(&n) && !s.chars().any(|c| c.is_control() || banned.contains(&c));
    if ok { Ok(()) } else { Err(()) }
}

/// The profile-name rule of `amplihack profile` (S8).
pub fn profile_name(s: &str) -> Result<(), String> {
    if amplihack_utils::rotator_status::valid_live_name(s) {
        Ok(())
    } else {
        Err(PROFILE_RULE.to_string())
    }
}

/// Model, effort and permission mode: 1 to 256 bytes, no control
/// character, and no leading `-`, so the value can never become a flag.
pub fn option_value(v: &str) -> Result<(), String> {
    let ok = !v.is_empty()
        && v.len() <= MAX_OPTION_BYTES
        && !v.starts_with('-')
        && !v.chars().any(char::is_control);
    if ok {
        Ok(())
    } else {
        Err(
            "must be 1 to 256 bytes, without control characters, and must not start with '-'"
                .into(),
        )
    }
}

/// An allowlisted environment value: at most 4096 bytes, no newline or NUL.
pub fn env_value(v: &str) -> Result<(), String> {
    if v.len() <= MAX_ENV_BYTES && !v.contains(['\n', '\0']) {
        Ok(())
    } else {
        Err(ENV_RULE.to_string())
    }
}

/// 32 lowercase hexadecimal characters (A14).
pub fn launch_id(s: &str) -> Result<(), String> {
    if s.len() == 32
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err("must be 32 lowercase hexadecimal characters".into())
    }
}

/// Absolute, UTF-8, no control character, at most 107 bytes (the size of a
/// Unix socket address).
pub fn socket_path(p: &Path) -> Result<(), String> {
    let rule = || {
        "must be an absolute UTF-8 path of at most 107 bytes, without control characters"
            .to_string()
    };
    let s = p.to_str().ok_or_else(rule)?;
    if p.is_absolute() && s.len() <= MAX_SOCKET_BYTES && !s.chars().any(char::is_control) {
        Ok(())
    } else {
        Err(rule())
    }
}

/// The lexical launch-directory rule: absolute, UTF-8, no `#` (tmux expands
/// formats in `-c`) and no control character.
fn launch_dir_lexical(p: &Path) -> Result<(), String> {
    let rule = || "must be an absolute UTF-8 path, without '#' or control characters".to_string();
    let s = p.to_str().ok_or_else(rule)?;
    if p.is_absolute() && !s.chars().any(|c| c == '#' || c.is_control()) {
        Ok(())
    } else {
        Err(rule())
    }
}

/// The lexical rule, plus "exists and is a directory".
pub fn launch_dir(p: &Path) -> Result<(), String> {
    launch_dir_lexical(p)?;
    if p.is_dir() {
        Ok(())
    } else {
        Err("must be an existing directory".into())
    }
}

/// An `--add-dir` value: joined to `base` when relative; no control
/// character; UTF-8.
pub fn add_dir(p: &Path, base: &Path) -> Result<PathBuf, String> {
    let full = if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    };
    add_dir_lexical(&full)?;
    Ok(full)
}

fn add_dir_lexical(p: &Path) -> Result<(), String> {
    let rule = || "must be a UTF-8 path without control characters".to_string();
    let s = p.to_str().ok_or_else(rule)?;
    if p.is_absolute() && !s.chars().any(char::is_control) {
        Ok(())
    } else {
        Err(rule())
    }
}

/// The field of an entry that failed its rule. Holds the field name only,
/// so no message can quote a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldError {
    pub field: &'static str,
}

impl std::fmt::Display for FieldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "field {} is not valid", self.field)
    }
}

impl std::error::Error for FieldError {}

/// Every lexical rule of data model section 4.1 for one entry. Rules that
/// need the filesystem are left to `register` and the wrapper.
pub fn entry(name: &str, e: &Entry) -> Result<(), FieldError> {
    fn need(ok: bool, field: &'static str) -> Result<(), FieldError> {
        if ok {
            Ok(())
        } else {
            Err(FieldError { field })
        }
    }
    fn opt<T: ?Sized>(v: Option<&T>, rule: impl Fn(&T) -> bool) -> bool {
        v.is_none_or(rule)
    }
    let ts = |s: &str| parse_ts(s).is_some();
    need(agent_name(name).is_ok(), "name")?;
    need(session_name(&e.tmux_session).is_ok(), "tmux_session")?;
    need(window_name(&e.tmux_window).is_ok(), "tmux_window")?;
    let sock = e.tmux_socket.as_deref();
    need(
        opt(sock, |s: &str| socket_path(Path::new(s)).is_ok()),
        "tmux_socket",
    )?;
    need(
        launch_dir_lexical(Path::new(&e.launch_dir)).is_ok(),
        "launch_dir",
    )?;
    let id_ok = |s: &str| conversation_id(s).is_ok();
    need(opt(e.conversation_id.as_deref(), id_ok), "conversation_id")?;
    let option_ok = |s: &str| option_value(s).is_ok();
    need(opt(e.model.as_deref(), option_ok), "model")?;
    need(opt(e.effort.as_deref(), option_ok), "effort")?;
    need(
        opt(e.permission_mode.as_deref(), option_ok),
        "permission_mode",
    )?;
    let profile = e.amplihack_flags.profile.as_deref();
    need(
        opt(profile, |s: &str| profile_name(s).is_ok()),
        "amplihack_flags.profile",
    )?;
    let dirs_ok = e
        .add_dirs
        .iter()
        .all(|d| add_dir_lexical(Path::new(d)).is_ok());
    need(dirs_ok, "add_dirs")?;
    need(e.env.iter().all(|(k, v)| env_record::value_ok(k, v)), "env")?;
    need(ts(&e.registered_at), "registered_at")?;
    need(ts(&e.launched_at), "launched_at")?;
    need(launch_id(&e.launch_id).is_ok(), "launch_id")?;
    need(opt(e.lost_at.as_deref(), ts), "lost_at")?;
    need(opt(e.ended_at.as_deref(), ts), "ended_at")?;
    need(e.restarts.iter().all(|r| ts(r)), "restarts")?;
    if let Some(r) = &e.last_recovery {
        need(ts(&r.logged_at), "last_recovery.logged_at")?;
        let conv = r.conversation_id.as_deref();
        need(opt(conv, id_ok), "last_recovery.conversation_id")?;
        need(opt(r.lost.as_deref(), ts), "last_recovery.lost")?;
        need(opt(r.resumed.as_deref(), ts), "last_recovery.resumed")?;
    }
    Ok(())
}
