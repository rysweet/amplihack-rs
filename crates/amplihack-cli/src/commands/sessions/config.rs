//! Days and destination for `sessions archive` / `restore`: the flag, then
//! the user config key, then the default.
//!
//! The keys are `sessions.archive_days` and `sessions.archive_dest` in
//! `<home>/.amplihack/config.toml`, read on every call. The command in a
//! shell and the rotator's daily run (`profile/rotate/sessions_daily.rs`)
//! therefore always use the same values, and a running rotator follows
//! `amplihack config set` without a restart.
//!
//! There are no environment variables for them. The background rotator
//! keeps the environment it was launched with, by autostart or `rotate
//! start`, so a variable exported later in a shell would reach
//! `sessions restore` but not the rotator, and restore would look in a
//! different destination from the one the daily run wrote to (issue #1746,
//! review of #1766).

use crate::user_config::safe::display_path;
use crate::user_config::{
    SESSIONS_ARCHIVE_DAYS, SESSIONS_ARCHIVE_DEST, TEST_HOME_GUARD_ENV, Value, guard_forbids,
    sessions_key,
};
use std::fmt;
use std::path::{Path, PathBuf};

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
    NoCwd,
    /// The user config file cannot give the key a value: `key`, and why.
    /// The reason names the file and the problem, never the value.
    Key {
        key: &'static str,
        why: String,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoHome => f.write_str("HOME is not set"),
            Self::NoCwd => {
                f.write_str("cannot read the current directory for a relative destination")
            }
            Self::Key { key, why } => write!(f, "cannot read {key} ({why})"),
        }
    }
}

/// Resolves the options. A key is read only when its flag is absent, so a
/// flag works even when the file is unusable. A file that cannot give a
/// key a value is an error rather than the default, because the archive
/// removes files and a wrong destination splits the archive.
pub fn resolve(
    days: Option<u32>,
    dest: Option<PathBuf>,
    apply: bool,
    home: Option<PathBuf>,
) -> Result<Opts, ConfigError> {
    let home = home
        .filter(|h| !h.as_os_str().is_empty())
        .ok_or(ConfigError::NoHome)?;
    let days = match days {
        Some(d) => d,
        None => saved(&home, SESSIONS_ARCHIVE_DAYS)?
            .and_then(|v| v.as_int())
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(DEFAULT_DAYS),
    };
    let dest = match dest {
        Some(d) => d,
        None => saved(&home, SESSIONS_ARCHIVE_DEST)?
            .and_then(|v| v.as_text().map(PathBuf::from))
            .unwrap_or_else(|| default_dest(&home)),
    };
    // A key's path is always absolute; only `--dest` can be relative.
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

/// `<home>/.amplihack/session-archive`.
fn default_dest(home: &Path) -> PathBuf {
    home.join(".amplihack/session-archive")
}

/// The key from the user file, or why it cannot be used.
fn saved(home: &Path, key: &'static str) -> Result<Option<Value>, ConfigError> {
    sessions_key(home, key).map_err(|why| ConfigError::Key { key, why })
}

/// The line that refuses to run when the test guard
/// ([`TEST_HOME_GUARD_ENV`], set on every `amplihack` a test starts) is set
/// and `path`, the HOME or DEST named by `what`, is outside the temp dir;
/// `None` otherwise.
///
/// The keys are guarded in [`sessions_key`], but a key is read only when
/// its flag is absent, so `archive --days N --dest DIR` and
/// `restore --dest DIR` never read one. Without this check a test that
/// gives both flags but leaves the real HOME would archive and remove the
/// developer's real `~/.claude/projects` (issues #1542 and #1699, review of
/// #1766).
pub fn outside_test_temp(what: &str, path: &Path) -> Option<String> {
    guard_forbids(path).then(|| {
        format!(
            "{TEST_HOME_GUARD_ENV} is set and {what} ({}) is outside the temp dir; nothing changed",
            display_path(path)
        )
    })
}
