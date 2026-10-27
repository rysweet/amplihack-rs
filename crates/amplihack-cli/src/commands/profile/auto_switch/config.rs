//! `<root>/.auto-switch`: the on/off switch, threshold, exclusions and
//! credit-backed overflow profiles, and the `AMPLIHACK_CLAUDE_AUTO_SWITCH`
//! override. Read again on every check; never
//! cached.

use super::fsx;
use crate::commands::profile::ProfileError;
use crate::commands::profile::name::ProfileName;
use std::ffi::OsStr;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::Duration;

pub const FILE: &str = ".auto-switch";
pub const TOGGLE_ENV: &str = "AMPLIHACK_CLAUDE_AUTO_SWITCH";
const MAX_BYTES: u64 = 4096;
const DEFAULT_THRESHOLD: u8 = 90;
/// How long a writer waits for the auto-switch state lock, which a rotator
/// cycle or a swap may hold, before giving up with a retry message.
pub const CONFIG_LOCK_WAIT: Duration = Duration::from_secs(10);

/// What a writer starts from when the file is missing or corrupt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadMode {
    /// The defaults in both cases (`auto-switch on` and `off`, which exist
    /// to recover from a damaged file).
    DefaultOnInvalid,
    /// The defaults when missing; a corrupt file is refused and left alone
    /// (threshold, exclude, overflow add and remove).
    Editable,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub enabled: bool,
    pub threshold: u8,
    pub exclude: Vec<ProfileName>,
    /// Credit-backed overflow profiles (issue #1606), in config order, which
    /// is their ranking. Only the rotator ever moves onto one.
    pub overflow: Vec<ProfileName>,
}

impl Config {
    /// The profiles auto-switch never picks: `exclude`, then the overflow
    /// profiles not already in it. Auto-switch has no reset or wait inputs,
    /// so it must never spend credits (D7).
    pub fn selection_exclude(&self) -> Vec<ProfileName> {
        let mut out = self.exclude.clone();
        for n in &self.overflow {
            if !out.contains(n) {
                out.push(n.clone());
            }
        }
        out
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold: DEFAULT_THRESHOLD,
            exclude: Vec::new(),
            overflow: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum FileState {
    Missing,
    Valid(Config),
    Corrupt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvToggle {
    Unset,
    On,
    Off,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Env,
    File,
    Default,
    Corrupt,
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Env => "env",
            Self::File => "file",
            Self::Default => "default",
            Self::Corrupt => "corrupt",
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Effective {
    pub on: bool,
    pub source: Source,
    /// Threshold, exclusions and overflow profiles: always the file's, or the
    /// defaults (so a corrupt file gives no overflow profiles).
    pub config: Config,
}

pub fn parse(raw: &[u8]) -> Result<Config, String> {
    if raw.len() as u64 > MAX_BYTES {
        return Err("file too large".into());
    }
    let text = std::str::from_utf8(raw).map_err(|_| "not valid UTF-8".to_string())?;
    let body = text.strip_suffix('\n').unwrap_or(text);
    let mut enabled = None;
    let mut threshold = None;
    let mut exclude = None;
    let mut overflow = None;
    for line in body.split('\n') {
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| "line without '='".to_string())?;
        let repeated = match key {
            "enabled" => enabled
                .replace(match value {
                    "on" => true,
                    "off" => false,
                    _ => return Err("enabled must be on or off".into()),
                })
                .is_some(),
            "threshold" => threshold.replace(parse_threshold(value)?).is_some(),
            "exclude" => exclude
                .replace(parse_names(value, "exclude", false)?)
                .is_some(),
            "overflow" => overflow
                .replace(parse_names(value, "overflow", true)?)
                .is_some(),
            _ => return Err("unknown key".into()),
        };
        if repeated {
            return Err("repeated key".into());
        }
    }
    Ok(Config {
        enabled: enabled.ok_or_else(|| "missing enabled".to_string())?,
        threshold: threshold.unwrap_or(DEFAULT_THRESHOLD),
        exclude: exclude.unwrap_or_default(),
        overflow: overflow.unwrap_or_default(),
    })
}

/// `1`..=`100`, digits only.
pub fn parse_threshold(value: &str) -> Result<u8, String> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err("threshold must be a whole number from 1 to 100".into());
    }
    match value.parse::<u8>() {
        Ok(n @ 1..=100) => Ok(n),
        _ => Err("threshold must be a whole number from 1 to 100".into()),
    }
}

/// A comma-separated list of profile names; empty is an empty list. With
/// `reject_dupes`, a name given twice is an error.
fn parse_names(value: &str, key: &str, reject_dupes: bool) -> Result<Vec<ProfileName>, String> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let mut out: Vec<ProfileName> = Vec::new();
    for raw in value.split(',') {
        let name = ProfileName::parse(raw).map_err(|_| format!("invalid profile name in {key}"))?;
        if reject_dupes && out.contains(&name) {
            return Err(format!("repeated profile name in {key}"));
        }
        out.push(name);
    }
    Ok(out)
}

/// The file's text. `overflow=` comes last and only when the list is not
/// empty, so a file without overflow profiles is unchanged by this key.
pub fn render(c: &Config) -> String {
    let join = |names: &[ProfileName]| {
        names
            .iter()
            .map(ProfileName::as_str)
            .collect::<Vec<_>>()
            .join(",")
    };
    let mut out = format!(
        "enabled={}\nthreshold={}\nexclude={}\n",
        if c.enabled { "on" } else { "off" },
        c.threshold,
        join(&c.exclude)
    );
    if !c.overflow.is_empty() {
        out.push_str(&format!("overflow={}\n", join(&c.overflow)));
    }
    out
}

pub fn env_toggle(raw: Option<&OsStr>) -> EnvToggle {
    match raw.map(|v| v.as_encoded_bytes()) {
        None | Some(b"") => EnvToggle::Unset,
        Some(b"1") => EnvToggle::On,
        Some(b"0") => EnvToggle::Off,
        Some(_) => EnvToggle::Invalid,
    }
}

pub fn effective_from(env: EnvToggle, file: &FileState) -> Effective {
    let config = match file {
        FileState::Valid(c) => c.clone(),
        _ => Config::default(),
    };
    let (on, source) = match (env, file) {
        (EnvToggle::On, _) => (true, Source::Env),
        (EnvToggle::Off, _) => (false, Source::Env),
        (_, FileState::Valid(c)) => (c.enabled, Source::File),
        (_, FileState::Missing) => (false, Source::Default),
        (_, FileState::Corrupt) => (false, Source::Corrupt),
    };
    Effective { on, source, config }
}

/// The effective setting for this process: the environment first, then the
/// file. An unrecognised environment value warns once per process.
pub fn effective(root: &Path) -> Effective {
    let env = env_toggle(std::env::var_os(TOGGLE_ENV).as_deref());
    if env == EnvToggle::Invalid {
        static WARNED: std::sync::Once = std::sync::Once::new();
        WARNED.call_once(|| {
            eprintln!("warning: {TOGGLE_ENV} must be 1 or 0; using <root>/{FILE}");
        });
    }
    effective_from(env, &read(root))
}

/// The file as it is. One not owned by the current user, or writable by
/// group or others, reads as `Corrupt`, like the rotate ledger: anyone who
/// can write it could otherwise mark a profile as overflow and spend credits.
pub fn read(root: &Path) -> FileState {
    match fsx::read_bounded_nofollow(&root.join(FILE), MAX_BYTES) {
        Ok(None) => FileState::Missing,
        Ok(Some((raw, meta))) if meta.uid() == fsx::euid() && !fsx::writable_by_others(&meta) => {
            parse(&raw).map_or(FileState::Corrupt, FileState::Valid)
        }
        _ => FileState::Corrupt,
    }
}

/// The only writer of `.auto-switch` outside tests: a read-modify-write
/// under the auto-switch state lock, so two writers (or a writer and a
/// rotator cycle) cannot lose each other's change. Waits at most
/// `CONFIG_LOCK_WAIT`. Returns the resulting settings; nothing is written
/// when `f` leaves them unchanged or fails.
pub fn update(
    root: &Path,
    mode: ReadMode,
    f: impl FnOnce(&mut Config) -> Result<(), ProfileError>,
) -> Result<Config, ProfileError> {
    update_within(root, mode, CONFIG_LOCK_WAIT, f)
}

/// `update` with the lock wait given.
pub fn update_within(
    root: &Path,
    mode: ReadMode,
    wait: Duration,
    f: impl FnOnce(&mut Config) -> Result<(), ProfileError>,
) -> Result<Config, ProfileError> {
    let fail = |what: String| ProfileError::AutoSwitchConfig(what);
    let state = super::state::State::new(root)?;
    let Some(_lock) = state.try_lock(wait)? else {
        return Err(fail(format!(
            "{FILE} is being changed by another process (the rotator or auto-switch); retry"
        )));
    };
    // `before` is what an unchanged result needs no write for. `on` and
    // `off` always write over a missing or corrupt file, so the file then
    // says what they set.
    let (before, mut c) = match (read(root), mode) {
        (FileState::Valid(c), _) => (Some(c.clone()), c),
        (FileState::Missing, ReadMode::Editable) => (Some(Config::default()), Config::default()),
        (FileState::Missing | FileState::Corrupt, ReadMode::DefaultOnInvalid) => {
            (None, Config::default())
        }
        (FileState::Corrupt, ReadMode::Editable) => {
            return Err(fail(format!(
                "{FILE} is unreadable or invalid; not changed"
            )));
        }
    };
    f(&mut c)?;
    if before.as_ref() == Some(&c) {
        return Ok(c);
    }
    if render(&c).len() as u64 > MAX_BYTES {
        return Err(fail(format!(
            "{FILE} would be larger than {MAX_BYTES} bytes; not changed"
        )));
    }
    write(root, &c)?;
    Ok(c)
}

/// Writes the file unlocked; callers go through `update`.
pub(super) fn write(root: &Path, c: &Config) -> Result<(), ProfileError> {
    let fail = |what: String| ProfileError::AutoSwitchConfig(what);
    fsx::check_root_private(root).map_err(|e| fail(format!("profile root {e}")))?;
    fsx::write_private_atomic(root, FILE, render(c).as_bytes())
        .map_err(|e| fail(format!("{FILE}: {e}")))
}
