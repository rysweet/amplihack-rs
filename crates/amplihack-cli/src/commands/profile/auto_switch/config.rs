//! `<root>/.auto-switch`: the on/off switch, threshold and exclusions, and the
//! `AMPLIHACK_CLAUDE_AUTO_SWITCH` override. Read again on every check; never
//! cached. The grammar and the file read are
//! `amplihack_utils::rotator_status`'s, which the hooks use too.

use super::fsx;
use crate::commands::profile::ProfileError;
use crate::commands::profile::name::ProfileName;
use amplihack_utils::rotator_status::{self as shared, DEFAULT_THRESHOLD, Toggle, ToggleFile};
use std::path::Path;

pub use amplihack_utils::rotator_status::{EnvToggle, TOGGLE_ENV, TOGGLE_FILE as FILE, env_toggle};

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

/// Parses a toggle file into this crate's [`Config`]; [`read`] does the
/// same for the file on disk.
#[cfg(test)]
pub fn parse(raw: &[u8]) -> Result<Config, String> {
    shared::parse_toggle(raw)
        .and_then(Config::from_toggle)
        .map_err(String::from)
}

/// `1`..=`100`, digits only.
pub fn parse_threshold(value: &str) -> Result<u8, String> {
    shared::parse_threshold(value).map_err(String::from)
}

impl Config {
    fn from_toggle(t: Toggle) -> Result<Self, &'static str> {
        let exclude = t
            .exclude
            .iter()
            .map(|n| ProfileName::parse(n).map_err(|_| "invalid profile name in exclude"))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            enabled: t.enabled,
            threshold: t.threshold,
            exclude,
        })
    }
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
    match shared::read_toggle(root) {
        ToggleFile::Missing => FileState::Missing,
        ToggleFile::Valid(t) => Config::from_toggle(t).map_or(FileState::Corrupt, FileState::Valid),
        ToggleFile::Corrupt => FileState::Corrupt,
    }
}

pub fn write(root: &Path, c: &Config) -> Result<(), ProfileError> {
    let fail = |what: String| ProfileError::AutoSwitchConfig(what);
    fsx::check_root_private(root).map_err(|e| fail(format!("profile root {e}")))?;
    fsx::write_private_atomic(root, FILE, render(c).as_bytes())
        .map_err(|e| fail(format!("{FILE}: {e}")))
}
