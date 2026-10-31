//! The login check before any move onto an API-key profile (issue #1804,
//! R4): the profile must name a key Claude can use. See "API-key login
//! check" in docs/reference/profile-command.md and `tests_key_check`.
//!
//! There are two checks, one for each place Claude can run:
//!
//! - [`reachable`] is the rotator's. A swap moves only `.credentials.json`
//!   into `~/.claude`, and plain `claude` then reads `~/.claude.json` and
//!   `~/.claude/settings.json`, so a key left in the profile's directory
//!   never reaches it. Only those two files count.
//! - [`configured`] is auto-switch's. A nested launch or rerun runs with
//!   `CLAUDE_CONFIG_DIR=<profile>`, so the profile's own files are the ones
//!   Claude reads.
//!
//! Both read with the evidence rules of kind detection: no symlink, a size
//! limit, a regular file of the current user that group and others cannot
//! write. Neither runs `apiKeyHelper` or makes a network request. No value
//! leaves this module: the detail is a fixed string, and a reason names only
//! the setting or the file, so a key can never reach a log, the ledger or a
//! status line.
//!
//! A home key is host-wide. Claude reads `~/.claude.json` and
//! `~/.claude/settings.json` whatever login is live, and each accepted
//! setting wins over a plan login, so every plan session on the host bills
//! per token, marked or not, and the live `/usage` probe prints cost text
//! the rotator cannot read. Claude 2.1.316 was run to confirm this; the
//! results are under "API-key login check". [`HOME_KEY_EFFECT`] says so at
//! run time: the cycle logs it as an ALERT (`note_home_key`) and `overflow
//! add` prints it.
//!
//! [`home_key`] is the third reader, and it is only for saying that money is
//! being spent. A file that fails the evidence rules must not allow a move,
//! but Claude may still read it, so a warning cannot depend on them: it
//! follows a symlink, ignores owner and mode, and reads each file on its own.
//! It never allows anything and decides nothing, and it returns only the
//! setting's name. The failed-probe stay is a decision, so it uses
//! [`reachable`] (`observe::decide_measured`).

use crate::commands::profile::auto_switch::kit::{
    CLAUDE_JSON, MAX_CLAUDE_JSON, euid, read_bounded_nofollow, writable_by_others,
};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use serde_json::{Map, Value};
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

/// The rotator's check, given the home directory: `Ok(detail)` when a key is
/// where Claude will read it after the swap, else the reason.
/// `cycle::Ctx.key_check` holds one, so tests can replace it.
pub type KeyCheckFn = fn(&Path) -> Result<&'static str, String>;

const SETTINGS: &str = "settings.json";
const MAX_SETTINGS: u64 = 64 * 1024;
const PRIMARY: &str = "primaryApiKey set";
const NONE: &str = "no primaryApiKey, ANTHROPIC_API_KEY or apiKeyHelper setting";

/// What a key in the `$HOME` files does while a plan login is live.
pub const HOME_KEY_EFFECT: &str = "a key in ~/.claude.json or ~/.claude/settings.json is used \
     ahead of a plan login: every session on this host bills per token whether or not a profile \
     is marked, and the rotator cannot read the live login's usage. Remove the key to bill plans";

/// A key plain `claude` can read once a swap has made an API-key profile
/// live: [`names_a_key`] over `<home>/.claude.json` and
/// `<home>/.claude/settings.json`. A key in the profile's directory does not
/// count, since the swap does not move it.
pub fn reachable(home: &Path) -> Result<&'static str, String> {
    names_a_key(
        &home.join(CLAUDE_JSON),
        &home.join(".claude").join(SETTINGS),
    )
    .map_err(|why| {
        if why == NONE {
            format!("{NONE} in ~/.claude.json or ~/.claude/settings.json")
        } else {
            why
        }
    })
}

/// The key setting in `<home>/.claude.json` or `<home>/.claude/settings.json`,
/// for warnings only. This is not evidence for any decision: [`reachable`]
/// is. A symlink is followed, owner and mode are ignored, and a file that
/// cannot be read or parsed, or is over [`MAX_CLAUDE_JSON`], counts as
/// holding no key without hiding the other.
pub fn home_key(home: &Path) -> Option<&'static str> {
    let claude = lenient_object(&home.join(CLAUDE_JSON));
    if claude.is_some_and(|m| in_claude_json(&m)) {
        return Some(PRIMARY);
    }
    lenient_object(&home.join(".claude").join(SETTINGS)).and_then(|m| in_settings(&m))
}

/// The JSON object in the regular file `path`, if there is one. `O_NONBLOCK`
/// keeps a FIFO from hanging the cycle.
fn lenient_object(path: &Path) -> Option<Map<String, Value>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut raw = Vec::new();
    file.take(MAX_CLAUDE_JSON.saturating_add(1))
        .read_to_end(&mut raw)
        .ok()?;
    match serde_json::from_slice::<Value>(&raw) {
        Ok(Value::Object(m)) => Some(m),
        _ => None,
    }
}

/// A key Claude can read when it runs with `CLAUDE_CONFIG_DIR` set to the
/// profile's directory: [`names_a_key`] over the profile's own files.
pub fn configured(store: &ProfileStore, name: &ProfileName) -> Result<&'static str, String> {
    let dir = store.root().join(name.as_str());
    names_a_key(&dir.join(CLAUDE_JSON), &dir.join(SETTINGS))
}

/// `primaryApiKey` in `claude_json`, `env.ANTHROPIC_API_KEY` or `apiKeyHelper`
/// in `settings_json`, each a non-empty string after trimming.
fn names_a_key(claude_json: &Path, settings_json: &Path) -> Result<&'static str, String> {
    let claude = object(claude_json, CLAUDE_JSON, MAX_CLAUDE_JSON)?;
    if claude.as_ref().is_some_and(in_claude_json) {
        return Ok(PRIMARY);
    }
    let settings = object(settings_json, SETTINGS, MAX_SETTINGS)?;
    settings
        .as_ref()
        .and_then(in_settings)
        .ok_or_else(|| NONE.to_string())
}

fn in_claude_json(m: &Map<String, Value>) -> bool {
    set(m.get("primaryApiKey"))
}

fn in_settings(m: &Map<String, Value>) -> Option<&'static str> {
    let env = m.get("env").and_then(Value::as_object);
    if set(env.and_then(|e| e.get("ANTHROPIC_API_KEY"))) {
        return Some("ANTHROPIC_API_KEY set");
    }
    set(m.get("apiKeyHelper")).then_some("apiKeyHelper set")
}

/// A string with something in it besides whitespace.
fn set(v: Option<&Value>) -> bool {
    v.and_then(Value::as_str)
        .is_some_and(|s| !s.trim().is_empty())
}

/// The JSON object in `path`, shown as `file` in a reason. `Ok(None)` when
/// the file is missing; an error when it may not be read as evidence or is
/// not a JSON object. A parse error's own text, which may quote the file, is
/// dropped here.
fn object(path: &Path, file: &str, limit: u64) -> Result<Option<Map<String, Value>>, String> {
    let (raw, meta) = match read_bounded_nofollow(path, limit) {
        Ok(None) => return Ok(None),
        Ok(Some(found)) => found,
        // `is a symlink`, `is not a regular file` or `is too large`.
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
            return Err(format!("{file} {e}"));
        }
        Err(_) => return Err(format!("{file} cannot be read")),
    };
    if meta.uid() != euid() {
        return Err(format!("{file} is not owned by you"));
    }
    if writable_by_others(&meta) || meta.mode() & 0o020 != 0 {
        return Err(format!("{file} is writable by group or others"));
    }
    match serde_json::from_slice::<Value>(&raw) {
        Ok(Value::Object(m)) => Ok(Some(m)),
        _ => Err(format!("{file} cannot be parsed")),
    }
}
