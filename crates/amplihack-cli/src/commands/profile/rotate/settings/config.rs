//! The user file's side of the rotate settings (issue #2007): the
//! `rotator.*` keys read once from `~/.amplihack/config.toml`, failing
//! closed, and the Codex credit floor, which is a key only.

use crate::commands::profile::ProfileError;
use crate::user_config::schema::{ValueKind, kind_requirement};
use crate::user_config::{self, Value};
use amplihack_utils::codex_capacity::DEFAULT_FLOOR;
use std::path::Path;

/// The user file's value for a `rotator.*` key: `Ok(None)` when it is not
/// set, `Err` with the reason when it cannot be used.
pub type Cfg<'a> = dyn Fn(&str) -> Result<Option<u64>, String> + 'a;

/// A [`Cfg`] that sets nothing, for paths that use the defaults only and
/// for tests that must not read a real config file.
pub fn no_config(_: &str) -> Result<Option<u64>, String> {
    Ok(None)
}

/// `rotator.codex_floor`: no flag, no variable, `codex-pick`'s default.
pub(super) const CODEX_FLOOR_KEY: &str = "rotator.codex_floor";
pub(super) const CODEX_FLOOR_MIN: u64 = 0;
pub(super) const CODEX_FLOOR_MAX: u64 = 1_000_000;
pub(super) const CODEX_FLOOR_DEFAULT: u64 = DEFAULT_FLOOR as u64;

/// The `rotator.*` keys [`UserConfig`] reads.
pub(super) const CONFIG_KEYS: [&str; 4] = [
    "rotator.session_at",
    "rotator.week_at",
    "rotator.interval",
    CODEX_FLOOR_KEY,
];

/// Where a value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Flag,
    Env,
    Config,
    Default,
    /// The running rotator's recorded gate (`profile usage` only).
    Rotator,
}

impl Origin {
    /// The lowercase label of `profile usage` and `--summary-json`.
    pub fn label(self) -> &'static str {
        match self {
            Origin::Flag => "flag",
            Origin::Env => "env",
            Origin::Config => "config",
            Origin::Default => "default",
            Origin::Rotator => "rotator",
        }
    }
}

/// A value and where it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Threshold<T> {
    pub value: T,
    pub origin: Origin,
}

/// The thresholds in force, each resolved on its own, so a bad value
/// fails only its own field.
#[derive(Debug)]
pub struct Thresholds {
    pub session_at: Result<Threshold<u8>, ProfileError>,
    pub week_at: Result<Threshold<u8>, ProfileError>,
    pub interval: Result<Threshold<u64>, ProfileError>,
    pub codex_floor: Result<Threshold<u64>, ProfileError>,
}

/// How to fix a problem with the whole user file. `config set` and
/// `config unset` cannot help, so the message points at the file; the path
/// itself is left to `config list --show-origin`.
const WHOLE_FILE_FIX: &str = "; fix the user config file first \
     (`amplihack config list --show-origin` shows its path and the problem)";

/// The error for a `rotator.*` value that cannot be used: the key, the
/// reason and how to fix it. A reason from [`UserConfig`] for a problem
/// with the whole file already ends with [`WHOLE_FILE_FIX`] and is used as
/// it is; any other reason gets the `config set`/`config unset` advice. It
/// never quotes the file's path or lines.
fn config_error(key: &str, why: &str) -> ProfileError {
    if why.ends_with(WHOLE_FILE_FIX) {
        return ProfileError::RotateConfig(format!("invalid rotate setting {key}: {why}"));
    }
    ProfileError::RotateConfig(format!(
        "invalid rotate setting {key}: {why}; fix it with `amplihack config set {key} <value>` \
         or `amplihack config unset {key}`"
    ))
}

/// `key` from `cfg`, checked against `min..=max` again; `Err` is the bare
/// reason.
fn config_value(key: &str, min: u64, max: u64, cfg: &Cfg<'_>) -> Result<Option<u64>, String> {
    match cfg(key)? {
        None => Ok(None),
        Some(v) if v >= min && v <= max => Ok(Some(v)),
        Some(_) => Err(kind_requirement(ValueKind::Int { min, max })),
    }
}

/// [`config_value`], with the error naming the key and the fix.
pub(super) fn from_config(
    key: &str,
    min: u64,
    max: u64,
    cfg: &Cfg<'_>,
) -> Result<Option<u64>, ProfileError> {
    config_value(key, min, max, cfg).map_err(|why| config_error(key, &why))
}

/// The Codex credit floor: `rotator.codex_floor`, else `codex-pick`'s
/// default [`DEFAULT_FLOOR`]. It has no flag and no variable. An account
/// whose rate limits are used up counts only with a balance above it.
pub fn codex_floor(cfg: &Cfg<'_>) -> Result<Threshold<u64>, ProfileError> {
    let at = |value, origin| Threshold { value, origin };
    Ok(
        from_config(CODEX_FLOOR_KEY, CODEX_FLOOR_MIN, CODEX_FLOOR_MAX, cfg)?
            .map_or(at(CODEX_FLOOR_DEFAULT, Origin::Default), |v| {
                at(v, Origin::Config)
            }),
    )
}

/// [`codex_floor`]'s value, with only the bare reason when it cannot be
/// used (`must be an integer from 0 to 1000000`), for the Codex block's
/// `next: not chosen (rotator.codex_floor: <reason>)` line.
pub fn codex_floor_value(cfg: &Cfg<'_>) -> Result<u64, String> {
    Ok(
        config_value(CODEX_FLOOR_KEY, CODEX_FLOOR_MIN, CODEX_FLOOR_MAX, cfg)?
            .unwrap_or(CODEX_FLOOR_DEFAULT),
    )
}

/// A [`Cfg`] over the user file under this process's HOME, read on the
/// first lookup and only then: `rotate` forms that resolve no settings never
/// touch the file. The background rotator gets every value as a flag, so it
/// never reads the file itself.
pub fn user_file_cfg() -> impl Fn(&str) -> Result<Option<u64>, String> {
    let user = std::cell::OnceCell::new();
    move |key| user.get_or_init(UserConfig::read).get(key)
}

/// The `rotator.*` keys of one user config file, read once. Each lookup
/// fails closed: a file that cannot be read or parsed, a user file group or
/// others can write, a value of the wrong type or out of range, or a HOME
/// the test guard forbids gives `Err`, never the default.
#[derive(Debug, Clone)]
pub struct UserConfig {
    values: Vec<(&'static str, Result<Option<u64>, String>)>,
}

impl UserConfig {
    /// The user file under this process's HOME. With no HOME (or one the
    /// test guard discards), there is no user file and nothing is set.
    pub fn read() -> Self {
        match user_config::home_dir() {
            Some(home) => Self::at(&home),
            None => Self {
                values: CONFIG_KEYS.iter().map(|k| (*k, Ok(None))).collect(),
            },
        }
    }

    /// The user file under `home`.
    pub fn at(home: &Path) -> Self {
        let read = user_config::user_keys_detail(home, &CONFIG_KEYS);
        // A fault with the whole file makes every key from it unusable,
        // including keys the file does not set: fail closed, never default.
        // The loader records an unsafe file only against keys it sets, so a
        // file holding no `rotator.*` key is checked here directly.
        let whole_file = read
            .iter()
            .find_map(|r| match r {
                Err(p) if p.whole_file => Some(p.reason.clone()),
                _ => None,
            })
            .or_else(|| {
                user_config::safe::insecure_reason(&user_config::files::user_file(home))
                    .map(str::to_string)
            })
            .map(|why| format!("{why}{WHOLE_FILE_FIX}"));
        let values = CONFIG_KEYS
            .iter()
            .zip(read)
            .map(|(key, r)| {
                let v = match (&whole_file, r) {
                    (Some(why), _) => Err(why.clone()),
                    (None, Ok(None)) => Ok(None),
                    (None, Ok(Some(Value::Int(n)))) => Ok(Some(n)),
                    (None, Ok(Some(_))) => Err("is not an integer".to_string()),
                    (None, Err(p)) => Err(p.reason),
                };
                (*key, v)
            })
            .collect();
        Self { values }
    }

    /// The value of `key`, a [`Cfg`]. Asking for a key this reader did not
    /// load is a programming error and is reported, never treated as unset.
    pub fn get(&self, key: &str) -> Result<Option<u64>, String> {
        match self.values.iter().find(|(k, _)| *k == key) {
            Some((_, v)) => v.clone(),
            None => Err(format!("{key} is not a rotator key")),
        }
    }
}
