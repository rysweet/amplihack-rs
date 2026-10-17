//! `<root>/.auto-switch`: the on/off switch, threshold and exclusions, and the
//! `AMPLIHACK_CLAUDE_AUTO_SWITCH` override. Read again on every check; never
//! cached.

use super::fsx;
use crate::commands::profile::ProfileError;
use crate::commands::profile::name::ProfileName;
use std::ffi::OsStr;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

pub const FILE: &str = ".auto-switch";
pub const TOGGLE_ENV: &str = "AMPLIHACK_CLAUDE_AUTO_SWITCH";
const MAX_BYTES: u64 = 4096;
const DEFAULT_THRESHOLD: u8 = 90;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub enabled: bool,
    pub threshold: u8,
    pub exclude: Vec<ProfileName>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold: DEFAULT_THRESHOLD,
            exclude: Vec::new(),
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
    /// Threshold and exclusions: always the file's, or the defaults.
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
            "exclude" => exclude.replace(parse_exclude(value)?).is_some(),
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

fn parse_exclude(value: &str) -> Result<Vec<ProfileName>, String> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    value
        .split(',')
        .map(|n| ProfileName::parse(n).map_err(|_| "invalid profile name in exclude".to_string()))
        .collect()
}

pub fn render(c: &Config) -> String {
    let exclude: Vec<&str> = c.exclude.iter().map(ProfileName::as_str).collect();
    format!(
        "enabled={}\nthreshold={}\nexclude={}\n",
        if c.enabled { "on" } else { "off" },
        c.threshold,
        exclude.join(",")
    )
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

pub fn read(root: &Path) -> FileState {
    match fsx::read_bounded_nofollow(&root.join(FILE), MAX_BYTES) {
        Ok(None) => FileState::Missing,
        Ok(Some((raw, meta))) if meta.uid() == fsx::euid() => {
            parse(&raw).map_or(FileState::Corrupt, FileState::Valid)
        }
        _ => FileState::Corrupt,
    }
}

pub fn write(root: &Path, c: &Config) -> Result<(), ProfileError> {
    let fail = |what: String| ProfileError::AutoSwitchConfig(what);
    fsx::check_root_private(root).map_err(|e| fail(format!("profile root {e}")))?;
    fsx::write_private_atomic(root, FILE, render(c).as_bytes())
        .map_err(|e| fail(format!("{FILE}: {e}")))
}
