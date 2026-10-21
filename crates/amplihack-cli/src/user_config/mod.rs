//! Persistent user defaults (issue #1606): `~/.amplihack/config.toml` and a
//! project's `.amplihack/config.toml`.
//!
//! Precedence, per key, highest first: a command-line flag (applied by the
//! consumer) > the key's environment variable > the project file > the user
//! file > the legacy `~/.amplihack/config` JSON (only while its migration has
//! not completed) > the built-in default. The contract is
//! `docs/reference/user-config.md`.
//!
//! The getters below return what the files say and never fail: every consumer
//! keeps parsing its own environment variable exactly as before, so an
//! existing variable always wins and keeps its old meaning.

pub(crate) mod edit;
pub(crate) mod files;
pub(crate) mod load;
pub(crate) mod migrate;
pub(crate) mod safe;
pub(crate) mod schema;

#[cfg(test)]
mod tests_load;
#[cfg(test)]
mod tests_schema;

use std::fmt;
use std::path::{Path, PathBuf};

pub(crate) use load::{Effective, Entry, LoadCtx, load};
use safe::{display_path, display_safe};
pub(crate) use schema::Value;

/// An error from `amplihack config`, with the exit code it maps to.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ConfigError {
    #[error("{0}: unknown key")]
    UnknownKey(String),
    #[error("{key}: {reason}")]
    InvalidValue { key: String, reason: String },
    #[error("{0}: can only be set in the user file")]
    UserOnly(String),
    #[error("--project: not in a git repository")]
    NoProject,
    #[error("{path}: {reason}")]
    File { path: String, reason: String },
    #[error("{0}")]
    Runtime(String),
}

impl ConfigError {
    /// `2` for a usage error, `1` for a runtime or state error.
    pub(crate) fn exit_code(&self) -> i32 {
        match self {
            ConfigError::UnknownKey(_)
            | ConfigError::InvalidValue { .. }
            | ConfigError::UserOnly(_)
            | ConfigError::NoProject => 2,
            ConfigError::File { .. } | ConfigError::Runtime(_) => 1,
        }
    }
}

/// A problem found in a config file. Every part is escaped for printing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Diagnostic {
    pub path: String,
    pub key: Option<String>,
    pub reason: String,
}

impl Diagnostic {
    pub(crate) fn file(path: &Path, reason: String) -> Self {
        Self {
            path: display_path(path),
            key: None,
            reason: display_safe(&reason),
        }
    }

    pub(crate) fn key(path: &Path, key: &str, reason: String) -> Self {
        Self {
            path: display_path(path),
            key: Some(display_safe(key)),
            reason: display_safe(&reason),
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.key {
            Some(key) => write!(f, "{}: {key}: {}", self.path, self.reason),
            None => write!(f, "{}: {}", self.path, self.reason),
        }
    }
}

/// Where an effective value came from. A command-line flag outranks all of
/// these, but it is applied by the consuming command and never loaded here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Origin {
    Env(&'static str),
    /// The variable is set but invalid; the value shown is the layers below.
    EnvInvalid {
        var: &'static str,
        reason: String,
    },
    Project(PathBuf),
    User(PathBuf),
    Legacy(PathBuf),
    Default,
}

impl Origin {
    /// The short label: `env`, `project`, `user`, `legacy`, `default`.
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Origin::Env(_) | Origin::EnvInvalid { .. } => "env",
            Origin::Project(_) => "project",
            Origin::User(_) => "user",
            Origin::Legacy(_) => "legacy",
            Origin::Default => "default",
        }
    }
}

/// As printed after `# ` by `config list --show-origin`.
impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Origin::Default => f.write_str(self.label()),
            Origin::Env(var) => write!(f, "env ({var})"),
            Origin::EnvInvalid { reason, .. } => write!(f, "env (invalid: {reason})"),
            Origin::Project(path) | Origin::User(path) | Origin::Legacy(path) => {
                write!(f, "{} ({})", self.label(), display_path(path))
            }
        }
    }
}

/// Set (to anything non-empty) by the test helper
/// `tests/support/isolated_amplihack.rs` on every `amplihack` a test starts,
/// and never anywhere else.
pub(crate) const TEST_HOME_GUARD_ENV: &str = "AMPLIHACK_TEST_HOME_GUARD";

/// Whether this process is a test, or was started by one: unit tests always,
/// and anything with [`TEST_HOME_GUARD_ENV`] set.
fn test_guard() -> bool {
    cfg!(test) || std::env::var_os(TEST_HOME_GUARD_ENV).is_some_and(|v| !v.is_empty())
}

/// `$HOME`, when set and non-empty.
///
/// Under the test guard only a HOME inside the temp dir is used: a test must
/// never read or migrate the developer's real config (issue #1542, review of
/// #1647). A test that forgets to give its process a temp HOME then sees no
/// user config at all.
pub(crate) fn home_dir() -> Option<PathBuf> {
    match home_or_reason() {
        Ok(home) => Some(home),
        Err(reason) => {
            if test_guard() {
                // Said once per process, on stderr: a guarded run must never
                // look like a missing HOME.
                static SAID: std::sync::Once = std::sync::Once::new();
                SAID.call_once(|| eprintln!("amplihack: {reason}"));
            }
            None
        }
    }
}

/// `$HOME`, or why there is no user config: HOME is unset, or the test guard
/// discarded it.
fn home_or_reason() -> Result<PathBuf, String> {
    let Some(home) = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
    else {
        return Err("HOME is not set".to_string());
    };
    if test_guard() && !under_temp_dir(&home) {
        return Err(format!(
            "{TEST_HOME_GUARD_ENV} is set and HOME ({}) is outside the temp dir ({}); the user \
             config is not read or written",
            display_path(&home),
            display_path(&std::env::temp_dir())
        ));
    }
    Ok(home)
}

/// Why no user file applies, for an error message.
pub(crate) fn home_unavailable() -> String {
    home_or_reason()
        .err()
        .unwrap_or_else(|| "HOME is not set".to_string())
}

/// Whether the test guard forbids touching files under `path`: set, and
/// `path` outside the temp dir.
pub(crate) fn guard_forbids(path: &Path) -> bool {
    test_guard() && !under_temp_dir(path)
}

fn under_temp_dir(path: &Path) -> bool {
    let temp = std::env::temp_dir();
    let temp = temp.canonicalize().unwrap_or(temp);
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    path.starts_with(&temp)
}

/// Load the config the way this process sees it: `$HOME`, the process
/// environment, the project file for `cwd`, and the migration enabled.
pub(crate) fn load_for(cwd: &Path) -> Effective {
    let read_project = !test_guard() || under_temp_dir(cwd);
    load(&LoadCtx {
        home: home_dir(),
        cwd: cwd.to_path_buf(),
        read_project,
        env: &|name| std::env::var(name).ok(),
        migrate: true,
    })
}

/// [`load_for`] for keys that only the user file can set.
fn load_user_only() -> Effective {
    load(&LoadCtx {
        home: home_dir(),
        cwd: PathBuf::from("/"),
        read_project: false,
        env: &|name| std::env::var(name).ok(),
        migrate: true,
    })
}

/// What the user config says about `agent.binary` for a run in `cwd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentBinaryConfig {
    /// No user config file could be read.
    NotRead,
    /// The user config file was read and does not set `agent.binary`.
    Unset,
    Set(String),
}

/// The configured `agent.binary` for a run in `cwd`, from the files only.
pub(crate) fn configured_agent_binary(cwd: &Path) -> AgentBinaryConfig {
    let effective = load_for(cwd);
    if let Some(value) = effective
        .file_value("agent.binary")
        .and_then(Value::as_text)
    {
        return AgentBinaryConfig::Set(value.to_string());
    }
    let readable = effective
        .user_file
        .as_deref()
        .is_some_and(|path| matches!(safe::read_bounded(path), Ok(Some(_))));
    if readable {
        AgentBinaryConfig::Unset
    } else {
        AgentBinaryConfig::NotRead
    }
}

/// The configured `paths.home`, from the user file only. The caller checks
/// that it is still an existing directory this user owns.
pub(crate) fn configured_home() -> Option<PathBuf> {
    load_user_only()
        .file_value("paths.home")
        .and_then(Value::as_text)
        .map(PathBuf::from)
}

/// The saved NODE_OPTIONS consent (`node.heap_consent`, or the legacy file's
/// while its migration has not completed).
pub(crate) fn heap_consent() -> Option<bool> {
    load_user_only()
        .file_value("node.heap_consent")
        .and_then(Value::as_bool)
}

/// The configured `node.max_old_space_mb` override.
pub(crate) fn max_old_space_mb() -> Option<u64> {
    load_user_only()
        .file_value("node.max_old_space_mb")
        .and_then(Value::as_int)
}

/// `recipe.verbose` from the files (default `false`).
pub(crate) fn recipe_verbose(cwd: &Path) -> bool {
    load_for(cwd)
        .file_value("recipe.verbose")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// `recipe.step_timeout` from the files.
pub(crate) fn recipe_step_timeout(cwd: &Path) -> Option<u64> {
    load_for(cwd)
        .file_value("recipe.step_timeout")
        .and_then(Value::as_int)
}

/// `recipe.detach = "tmux"` from the files: whether `recipe run` detaches
/// when neither `--detach` nor `--no-detach` is given.
pub(crate) fn recipe_detach(cwd: &Path) -> bool {
    load_for(cwd)
        .entry("recipe.detach")
        .value
        .as_ref()
        .and_then(Value::as_text)
        == Some("tmux")
}

/// The effective `recipe.tmux_session_pattern` (the default when unset).
pub(crate) fn tmux_session_pattern(cwd: &Path) -> String {
    load_for(cwd)
        .entry("recipe.tmux_session_pattern")
        .value
        .as_ref()
        .and_then(Value::as_text)
        .unwrap_or(schema::DEFAULT_SESSION_PATTERN)
        .to_string()
}

/// The configured `recipe.log_dir`, from the user file only.
pub(crate) fn recipe_log_dir() -> Option<PathBuf> {
    load_user_only()
        .file_value("recipe.log_dir")
        .and_then(Value::as_text)
        .map(PathBuf::from)
}

/// `orchestrator.max_depth` from the files. Consumers apply it only where the
/// built-in default applied before, below `AMPLIHACK_MAX_DEPTH` and any
/// sealed tree ceiling.
pub(crate) fn max_depth(cwd: &Path) -> Option<u32> {
    load_for(cwd)
        .file_value("orchestrator.max_depth")
        .and_then(Value::as_int)
        .and_then(|depth| u32::try_from(depth).ok())
}

/// Save `value` for a user-only or user-scoped key in the user file.
pub(crate) fn set_user_value(name: &str, value: &Value) -> Result<PathBuf, ConfigError> {
    let def = schema::known_key(name)?;
    let home = home_dir().ok_or_else(|| {
        ConfigError::Runtime(format!("{}; cannot find the user file", home_unavailable()))
    })?;
    let path = files::user_file(&home);
    edit::set_value(&path, def, value)?;
    edit::tighten_owned_dir(&home.join(".amplihack"));
    Ok(path)
}
