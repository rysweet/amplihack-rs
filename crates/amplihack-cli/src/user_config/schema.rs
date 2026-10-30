//! The typed schema of the user config: every key, its type, where it may be
//! set, and the environment variable that outranks it.
//!
//! Every key here has a consumer. A key that would need a new subsystem is not
//! defined until that subsystem exists (issue #1606: no dead config).

use std::fmt;
use std::path::{Component, Path};

use amplihack_utils::agent_binary::validate_binary_name;

use super::ConfigError;
use crate::commands::session_tree::state::MAX_DEPTH_CEILING;

/// Longest accepted path value, in bytes.
pub(crate) const MAX_PATH_BYTES: usize = 4096;

/// Which files may set a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    /// Only `~/.amplihack/config.toml`. A project file arrives with a
    /// repository someone else wrote, so anything that names a path or is
    /// personal is user-only.
    UserOnly,
    /// The user file and the project file.
    UserAndProject,
}

/// The type of a key's value, with its validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValueKind {
    /// One of the allowlisted agent CLIs, canonicalised to lowercase.
    AgentBinary,
    /// An absolute path, without `..`, control characters or NUL, at most
    /// [`MAX_PATH_BYTES`].
    AbsPath,
    /// `true` or `false`.
    Bool,
    /// A decimal integer in `min..=max`.
    Int { min: u64, max: u64 },
    /// One of a fixed list of words.
    Choice(&'static [&'static str]),
    /// A tmux session-name pattern: see [`validate_session_pattern`].
    SessionPattern,
}

/// The values of `recipe.detach`.
pub(crate) const DETACH_MODES: &[&str] = &["tmux", "none"];

/// The placeholders a `recipe.tmux_session_pattern` may use.
pub(crate) const SESSION_PLACEHOLDERS: &[&str] = &["{recipe}", "{repo}", "{ts}"];

/// The default `recipe.tmux_session_pattern`.
pub(crate) const DEFAULT_SESSION_PATTERN: &str = "amp-{recipe}-{repo}-{ts}";

/// The default `orchestrator.max_depth`, as text; a test pins it to
/// `session_tree::state::DEFAULT_MAX_DEPTH`.
const DEFAULT_MAX_DEPTH: &str = "3";

/// Longest accepted session pattern, in characters.
pub(crate) const MAX_SESSION_PATTERN_CHARS: usize = 64;

/// A validated value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Value {
    Bool(bool),
    Int(u64),
    Text(String),
}

impl Value {
    /// The value as `config get` prints it: strings bare, without quotes.
    pub(crate) fn bare(&self) -> String {
        match self {
            Value::Bool(value) => value.to_string(),
            Value::Int(value) => value.to_string(),
            Value::Text(value) => value.clone(),
        }
    }

    pub(crate) fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub(crate) fn as_int(&self) -> Option<u64> {
        match self {
            Value::Int(value) => Some(*value),
            _ => None,
        }
    }

    pub(crate) fn as_text(&self) -> Option<&str> {
        match self {
            Value::Text(value) => Some(value),
            _ => None,
        }
    }
}

/// TOML syntax: strings quoted and escaped, bools and integers bare.
impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Bool(value) => write!(f, "{value}"),
            Value::Int(value) => write!(f, "{value}"),
            Value::Text(value) => {
                f.write_str("\"")?;
                for c in value.chars() {
                    match c {
                        '"' => f.write_str("\\\"")?,
                        '\\' => f.write_str("\\\\")?,
                        c if c.is_control() => write!(f, "\\u{:04X}", c as u32)?,
                        c => write!(f, "{c}")?,
                    }
                }
                f.write_str("\"")
            }
        }
    }
}

/// One key of the schema.
#[derive(Debug)]
pub(crate) struct KeyDef {
    /// `<table>.<key>`, as written on the command line.
    pub name: &'static str,
    pub scope: Scope,
    pub kind: ValueKind,
    /// The environment variable that outranks every file, if any.
    pub env: Option<&'static str>,
    /// The built-in default, as it would be typed on the command line, or
    /// `None` for "(unset)". Parsed by [`KeyDef::default_value`].
    pub default: Option<&'static str>,
    /// The project file may only lower this key below the user value (or the
    /// default), so a cloned repository cannot raise a limit.
    pub lower_only: bool,
}

impl KeyDef {
    /// The built-in default as a value.
    pub(crate) fn default_value(&self) -> Option<Value> {
        self.default.map(|raw| {
            parse_raw(self.kind, raw).expect("every schema default passes its own validation")
        })
    }

    /// The TOML table and the key inside it.
    pub(crate) fn split(&self) -> (&'static str, &'static str) {
        self.name
            .split_once('.')
            .expect("every schema key is <table>.<key>")
    }
}

/// Every key, in the documented order (`docs/reference/user-config.md#keys`).
pub(crate) static KEYS: &[KeyDef] = &[
    KeyDef {
        name: "agent.binary",
        // A cloned repository must not switch the CLI every agent step runs
        // under (review of #1647).
        scope: Scope::UserOnly,
        kind: ValueKind::AgentBinary,
        env: Some("AMPLIHACK_AGENT_BINARY"),
        default: None,
        lower_only: false,
    },
    KeyDef {
        name: "paths.home",
        scope: Scope::UserOnly,
        kind: ValueKind::AbsPath,
        env: Some("AMPLIHACK_HOME"),
        default: None,
        lower_only: false,
    },
    KeyDef {
        name: "node.heap_consent",
        scope: Scope::UserOnly,
        kind: ValueKind::Bool,
        env: None,
        default: None,
        lower_only: false,
    },
    KeyDef {
        name: "node.max_old_space_mb",
        scope: Scope::UserOnly,
        kind: ValueKind::Int {
            min: crate::memory_config::MIN_MEMORY_MB,
            max: crate::memory_config::MAX_MEMORY_MB,
        },
        env: None,
        default: None,
        lower_only: false,
    },
    KeyDef {
        name: "recipe.verbose",
        scope: Scope::UserAndProject,
        kind: ValueKind::Bool,
        env: None,
        default: Some("false"),
        lower_only: false,
    },
    KeyDef {
        name: "recipe.step_timeout",
        scope: Scope::UserAndProject,
        kind: ValueKind::Int {
            min: 1,
            max: u64::MAX,
        },
        env: Some("AMPLIHACK_STEP_TIMEOUT"),
        default: None,
        lower_only: true,
    },
    KeyDef {
        name: "recipe.detach",
        scope: Scope::UserAndProject,
        kind: ValueKind::Choice(DETACH_MODES),
        env: None,
        default: Some("none"),
        lower_only: false,
    },
    KeyDef {
        name: "recipe.tmux_session_pattern",
        scope: Scope::UserAndProject,
        kind: ValueKind::SessionPattern,
        env: None,
        default: Some(DEFAULT_SESSION_PATTERN),
        lower_only: false,
    },
    KeyDef {
        name: "recipe.log_dir",
        scope: Scope::UserOnly,
        kind: ValueKind::AbsPath,
        env: None,
        default: None,
        lower_only: false,
    },
    KeyDef {
        name: "recipe.usage_limit_wait",
        scope: Scope::UserAndProject,
        kind: ValueKind::Bool,
        env: Some("AMPLIHACK_USAGE_LIMIT_WAIT"),
        default: Some("true"),
        lower_only: false,
    },
    KeyDef {
        name: "recipe.usage_limit_max_wait_secs",
        scope: Scope::UserAndProject,
        kind: ValueKind::Int {
            min: 0,
            max: u64::MAX,
        },
        env: Some("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS"),
        default: None,
        lower_only: true,
    },
    KeyDef {
        name: "recipe.usage_limit_wait_budget_secs",
        scope: Scope::UserAndProject,
        kind: ValueKind::Int {
            min: 0,
            max: u64::MAX,
        },
        env: Some("AMPLIHACK_USAGE_LIMIT_WAIT_BUDGET_SECS"),
        default: Some("21600"),
        lower_only: true,
    },
    KeyDef {
        // 1..=100 is a safety limit: every wait can last 5 h + 120 s.
        name: "recipe.usage_limit_max_waits_per_step",
        scope: Scope::UserAndProject,
        kind: ValueKind::Int { min: 1, max: 100 },
        env: Some("AMPLIHACK_USAGE_LIMIT_MAX_WAITS_PER_STEP"),
        default: Some("5"),
        lower_only: true,
    },
    KeyDef {
        name: "orchestrator.max_depth",
        scope: Scope::UserAndProject,
        kind: ValueKind::Int {
            min: 0,
            max: MAX_DEPTH_CEILING as u64,
        },
        env: Some("AMPLIHACK_MAX_DEPTH"),
        default: Some(DEFAULT_MAX_DEPTH),
        lower_only: true,
    },
];

/// The schema entry for `name`.
pub(crate) fn key(name: &str) -> Option<&'static KeyDef> {
    KEYS.iter().find(|key| key.name == name)
}

/// The schema entry for `name`, or a usage error naming it.
pub(crate) fn known_key(name: &str) -> Result<&'static KeyDef, ConfigError> {
    key(name).ok_or_else(|| ConfigError::UnknownKey(super::safe::display_safe(name)))
}

/// Parse a value typed on the command line for `name`.
pub(crate) fn parse_cli(name: &str, raw: &str) -> Result<Value, ConfigError> {
    let key = known_key(name)?;
    parse_raw(key.kind, raw).map_err(|reason| ConfigError::InvalidValue {
        key: key.name.to_string(),
        reason,
    })
}

/// Parse a raw string (a command-line value or an environment variable) as a
/// `kind`. The error is the reason, ready to follow `<key>: `.
pub(crate) fn parse_raw(kind: ValueKind, raw: &str) -> Result<Value, String> {
    match kind {
        ValueKind::AgentBinary => validate_binary_name(raw)
            .map(Value::Text)
            .ok_or_else(|| kind_requirement(kind)),
        ValueKind::AbsPath => validate_abs_path(raw).map(|()| Value::Text(raw.to_string())),
        ValueKind::Choice(words) => words
            .iter()
            .find(|word| **word == raw)
            .map(|word| Value::Text((*word).to_string()))
            .ok_or_else(|| kind_requirement(kind)),
        ValueKind::SessionPattern => {
            validate_session_pattern(raw).map(|()| Value::Text(raw.to_string()))
        }
        ValueKind::Bool => match raw {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(kind_requirement(kind)),
        },
        ValueKind::Int { min, max } => {
            if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
                return Err(kind_requirement(kind));
            }
            match raw.parse::<u64>() {
                Ok(value) if (min..=max).contains(&value) => Ok(Value::Int(value)),
                _ => Err(kind_requirement(kind)),
            }
        }
    }
}

/// Convert a value read from a TOML file. The error is the reason.
pub(crate) fn from_toml(kind: ValueKind, item: &toml_edit::Value) -> Result<Value, String> {
    match (kind, item) {
        (
            ValueKind::AgentBinary
            | ValueKind::AbsPath
            | ValueKind::Choice(_)
            | ValueKind::SessionPattern,
            toml_edit::Value::String(s),
        ) => parse_raw(kind, s.value()),
        (ValueKind::Bool, toml_edit::Value::Boolean(b)) => Ok(Value::Bool(*b.value())),
        (ValueKind::Int { .. }, toml_edit::Value::Integer(i)) => {
            let value = *i.value();
            if value < 0 {
                return Err(kind_requirement(kind));
            }
            parse_raw(kind, &value.to_string())
        }
        _ => Err(kind_requirement(kind)),
    }
}

/// What a valid value of `kind` looks like, phrased to follow `<key>: `.
pub(crate) fn kind_requirement(kind: ValueKind) -> String {
    match kind {
        ValueKind::AgentBinary => "must be one of amplifier, claude, codex or copilot".to_string(),
        ValueKind::AbsPath => format!(
            "must be an absolute path without '..', control characters or NUL, at most \
             {MAX_PATH_BYTES} bytes"
        ),
        ValueKind::Bool => "must be true or false".to_string(),
        ValueKind::Choice(words) => format!("must be one of {}", words.join(", ")),
        ValueKind::SessionPattern => format!(
            "must be 1 to {MAX_SESSION_PATTERN_CHARS} characters of letters, digits, '-' and \
             '_', with the placeholders {}",
            SESSION_PLACEHOLDERS.join(", ")
        ),
        ValueKind::Int { min, max } if max == u64::MAX => {
            format!("must be an integer of at least {min}")
        }
        ValueKind::Int { min, max } => format!("must be an integer from {min} to {max}"),
    }
}

/// The syntactic checks every path value passes: absolute, no `..`
/// components, no control characters or NUL, at most [`MAX_PATH_BYTES`].
pub(crate) fn validate_abs_path(raw: &str) -> Result<(), String> {
    let requirement = || kind_requirement(ValueKind::AbsPath);
    if raw.is_empty() || raw.len() > MAX_PATH_BYTES || raw.chars().any(char::is_control) {
        return Err(requirement());
    }
    let path = Path::new(raw);
    if !path.is_absolute() || path.components().any(|c| c == Component::ParentDir) {
        return Err(requirement());
    }
    Ok(())
}

/// A session pattern is 1 to [`MAX_SESSION_PATTERN_CHARS`] characters of
/// ASCII letters, digits, `-` and `_`, plus the [`SESSION_PLACEHOLDERS`].
/// tmux forbids `.` and `:` in session names, and nothing here needs quoting.
pub(crate) fn validate_session_pattern(raw: &str) -> Result<(), String> {
    let requirement = || kind_requirement(ValueKind::SessionPattern);
    if raw.is_empty() || raw.chars().count() > MAX_SESSION_PATTERN_CHARS {
        return Err(requirement());
    }
    let literal = SESSION_PLACEHOLDERS
        .iter()
        .fold(raw.to_string(), |rest, placeholder| {
            rest.replace(placeholder, "")
        });
    if literal
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Ok(())
    } else {
        Err(requirement())
    }
}

/// The extra check a configured home must pass when it is used or set: an
/// existing directory owned by this user.
pub(crate) fn validate_home_dir(path: &Path) -> Result<(), String> {
    let meta =
        std::fs::metadata(path).map_err(|_| "must name an existing directory".to_string())?;
    if !meta.is_dir() {
        return Err("must name an existing directory".to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions and cannot fail.
        if meta.uid() != unsafe { libc::geteuid() } {
            return Err("must name a directory you own".to_string());
        }
    }
    Ok(())
}
