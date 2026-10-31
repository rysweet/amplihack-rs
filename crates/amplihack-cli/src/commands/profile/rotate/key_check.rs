//! The login check before any move onto an API-key profile (issue #1804,
//! R4): the profile must name a key Claude can use. See "API-key login
//! check" in docs/reference/profile-command.md and `tests_key_check`.
//!
//! The check reads the profile's own `.claude.json` and `settings.json`
//! with the evidence rules of kind detection: no symlink, a size limit, a
//! regular file of the current user that group and others cannot write. It
//! never runs `apiKeyHelper` and makes no network request. No value leaves
//! this module: the detail is a fixed string, and a reason names only the
//! setting or the file, so a key can never reach a log, the ledger or a
//! status line.

use crate::commands::profile::auto_switch::kit::{
    CLAUDE_JSON, MAX_CLAUDE_JSON, euid, read_bounded_nofollow, writable_by_others,
};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use serde_json::{Map, Value};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// The check: `Ok(detail)` when the profile names a key, else the reason.
/// `cycle::Ctx.key_check` holds one, so tests can replace it.
pub type KeyCheckFn = fn(&ProfileStore, &ProfileName) -> Result<&'static str, String>;

const SETTINGS: &str = "settings.json";
const MAX_SETTINGS: u64 = 64 * 1024;
const NONE: &str = "no primaryApiKey, ANTHROPIC_API_KEY or apiKeyHelper setting";

/// The production check: `primaryApiKey` in `.claude.json`,
/// `env.ANTHROPIC_API_KEY` or `apiKeyHelper` in `settings.json`, each a
/// non-empty string after trimming.
pub fn configured(store: &ProfileStore, name: &ProfileName) -> Result<&'static str, String> {
    let dir = store.root().join(name.as_str());
    let claude = object(&dir.join(CLAUDE_JSON), CLAUDE_JSON, MAX_CLAUDE_JSON)?;
    if claude.as_ref().is_some_and(|m| set(m.get("primaryApiKey"))) {
        return Ok("primaryApiKey set");
    }
    let settings = object(&dir.join(SETTINGS), SETTINGS, MAX_SETTINGS)?;
    if let Some(m) = &settings {
        let env = m.get("env").and_then(Value::as_object);
        if set(env.and_then(|e| e.get("ANTHROPIC_API_KEY"))) {
            return Ok("ANTHROPIC_API_KEY set");
        }
        if set(m.get("apiKeyHelper")) {
            return Ok("apiKeyHelper set");
        }
    }
    Err(NONE.to_string())
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
