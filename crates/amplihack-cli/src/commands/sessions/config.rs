//! Days and destination for `sessions archive` / `restore`: the flag, then
//! the variable, then the default. The only reader of the
//! `AMPLIHACK_SESSIONS_ARCHIVE_DAYS` and `_DEST` variables, and it reads
//! them only through the `env` it is given.

use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;

pub const DAYS_ENV: &str = "AMPLIHACK_SESSIONS_ARCHIVE_DAYS";
pub const DEST_ENV: &str = "AMPLIHACK_SESSIONS_ARCHIVE_DEST";
pub const DEFAULT_DAYS: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opts {
    pub home: PathBuf,
    pub days: u32,
    /// Always absolute.
    pub dest: PathBuf,
    pub apply: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    NoHome,
    BadDays,
    NoCwd,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoHome => "HOME is not set",
            Self::BadDays => "AMPLIHACK_SESSIONS_ARCHIVE_DAYS must be a whole number of at least 1",
            Self::NoCwd => "cannot read the current directory for a relative destination",
        })
    }
}

/// Resolves the options. An empty variable counts as unset; an invalid
/// days value is an error that never repeats the value.
pub fn resolve(
    days: Option<u32>,
    dest: Option<PathBuf>,
    apply: bool,
    env: &dyn Fn(&str) -> Option<OsString>,
    home: Option<PathBuf>,
) -> Result<Opts, ConfigError> {
    let home = home
        .filter(|h| !h.as_os_str().is_empty())
        .ok_or(ConfigError::NoHome)?;
    let set = |k: &str| env(k).filter(|v| !v.is_empty());
    let days = match days {
        Some(d) => d,
        None => match set(DAYS_ENV) {
            Some(raw) => parse_days(&raw).ok_or(ConfigError::BadDays)?,
            None => DEFAULT_DAYS,
        },
    };
    let dest = dest
        .or_else(|| set(DEST_ENV).map(PathBuf::from))
        .unwrap_or_else(|| home.join(".amplihack/session-archive"));
    let dest = if dest.is_absolute() {
        dest
    } else {
        std::env::current_dir()
            .map_err(|_| ConfigError::NoCwd)?
            .join(dest)
    };
    Ok(Opts {
        home,
        days,
        dest,
        apply,
    })
}

/// ASCII digits only (no sign or space), at least 1, fits in `u32`.
fn parse_days(raw: &std::ffi::OsStr) -> Option<u32> {
    let s = raw.to_str()?;
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<u32>().ok().filter(|&d| d >= 1)
}
