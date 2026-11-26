//! The typed schema of the user config: every key, its type, where it may be
//! set, and the environment variable that outranks it.
//!
//! Every key here has a consumer. A key that would need a new subsystem is not
//! defined until that subsystem exists (issue #1606: no dead config).

use std::fmt;
use std::path::{Component, Path};

use amplihack_utils::agent_binary::validate_binary_name;
use amplihack_utils::user_config::text_switch;

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
    /// A model id amplihack is willing to put on a command line, validated
    /// by the one checker every model string crosses
    /// ([`crate::commands::launch::model_tiers::table::validate_model_id`]).
    ModelId,
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
    /// A TOML array of command prefixes, each validated as a
    /// [`crate::policy::Prefix`], at most [`MAX_DENY_COMMANDS`] of them
    /// (issue #2589). Set only by editing the user file.
    CommandList,
    /// Canonical skill and agent identities, supplied as a TOML array.
    EntryList,
}

/// Most dangerous-command entries one file may add (issue #2589). The
/// shipped defaults are in code and are not counted against it.
pub(crate) const MAX_DENY_COMMANDS: usize = 64;

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
    /// A list of strings validated according to the key's kind.
    List(Vec<String>),
}

impl Value {
    /// The value as `config get` prints it: strings bare, without quotes.
    pub(crate) fn bare(&self) -> String {
        match self {
            Value::Bool(value) => value.to_string(),
            Value::Int(value) => value.to_string(),
            Value::Text(value) => value.clone(),
            // `config get` prints one value; the entries read best on one
            // line, which is also how `config list` shows them.
            Value::List(values) => values.join(", "),
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

    pub(crate) fn as_list(&self) -> Option<&[String]> {
        match self {
            Value::List(values) => Some(values),
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
            Value::List(values) => {
                f.write_str("[")?;
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{}", Value::Text(value.clone()))?;
                }
                f.write_str("]")
            }
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
            // A list default is written as TOML, which only a file supplies.
            let value = match self.kind {
                ValueKind::EntryList => raw
                    .parse::<toml_edit::Value>()
                    .map_err(|error| error.to_string())
                    .and_then(|item| from_toml(self.kind, &item)),
                _ => parse_raw(self.kind, raw),
            };
            value.expect("every schema default passes its own validation")
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
        name: "lists.set",
        scope: Scope::UserOnly,
        kind: ValueKind::Choice(&["default", "all"]),
        env: None,
        default: Some("default"),
        lower_only: false,
    },
    KeyDef {
        name: "lists.enabled",
        scope: Scope::UserOnly,
        kind: ValueKind::EntryList,
        env: None,
        default: Some("[]"),
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
        // Issue #2589: the dangerous-command entries this installation adds
        // to the shipped list. User-only, because a cloned repository must
        // not change what any harness denies, and with no variable, because
        // a deny list one exported variable can empty is not one.
        name: "policy.deny_commands",
        scope: Scope::UserOnly,
        kind: ValueKind::CommandList,
        env: None,
        default: None,
        lower_only: false,
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
    KeyDef {
        name: "model.force",
        // Issue #1619: names the model every launch asks for, and lands as
        // the argv token right after `--model`. A cloned repository must not
        // choose it (the reason the review of #1647 gave for `agent.binary`).
        scope: Scope::UserOnly,
        kind: ValueKind::ModelId,
        env: Some("AMPLIHACK_FORCE_MODEL"),
        default: None,
        lower_only: false,
    },
    KeyDef {
        name: "model.tier_map",
        // Issue #1619: the instance's tier mapping, including the
        // `[step_tiers]` table that drives amplihack's rewriting of recipe
        // YAML, whose bash step bodies execute. A path, and it decides which
        // model every agent step asks for, so user-only. No variable: it is
        // set deliberately in the user's own config, not picked up from an
        // inherited environment.
        scope: Scope::UserOnly,
        kind: ValueKind::AbsPath,
        env: None,
        default: None,
        lower_only: false,
    },
    KeyDef {
        name: "review.combined_small_diff",
        // Issue #1619: a small change may have its code, security and
        // philosophy passes as one review. Off by default, so the three
        // separate passes run exactly as they did before.
        //
        // User-only for the reason `model.force` above is: the review step
        // reads this from inside the worktree under review, so a project
        // value would let a cloned repository replace its own dedicated
        // security and philosophy passes with one generalist pass.
        scope: Scope::UserOnly,
        kind: ValueKind::Bool,
        env: None,
        default: Some("false"),
        lower_only: false,
    },
    KeyDef {
        name: "review.combined_diff_threshold",
        // Changed lines at or below which a diff counts as small. A
        // threshold of zero would mean no diff is ever small, which is what
        // turning the switch off already does.
        //
        // User-only with the switch it bounds: a project-settable threshold
        // of 100000 makes every diff small and is the same control by
        // another name.
        scope: Scope::UserOnly,
        kind: ValueKind::Int {
            min: 1,
            max: 100_000,
        },
        env: None,
        default: Some("200"),
        lower_only: false,
    },
    super::schema_governor::GOVERNOR_ENABLED,
    super::schema_governor::GOVERNOR_MAX_CONCURRENCY,
    super::schema_governor::GOVERNOR_RESERVE_POINTS,
    super::schema_governor::GOVERNOR_STEP,
    super::schema_governor::GOVERNOR_BAND_PCT,
    super::schema_governor::GOVERNOR_EVAL_INTERVAL_SECS,
    super::schema_governor::GOVERNOR_WAIT_SECS,
    super::schema_governor::GOVERNOR_MAX_READING_AGE_SECS,
    KeyDef {
        name: "agent.standing",
        // Issue #2912: whether a session started with `amplihack claude
        // --supervise NAME` is relaunched in its pane with `claude
        // --continue` when its process exits other than by its owner.
        //
        // User-only: it makes amplihack start an agent with nobody at the
        // keyboard, so a cloned repository must not be able to set it. No
        // variable: the wrapper is started by a person in a pane, and a
        // stray variable inherited from a parent run would mark a session
        // standing that nobody asked to be standing.
        scope: Scope::UserOnly,
        kind: ValueKind::Bool,
        env: None,
        default: Some("false"),
        lower_only: false,
    },
    KeyDef {
        name: "sessions.daily_archive",
        // Turns on a job that removes files from ~/.claude/projects; a
        // cloned repository must not decide that, and the rotator that
        // reads it has no project. No variable: the background rotator is
        // started without the launching shell's AMPLIHACK_ROTATE_*
        // variables, so only the file reaches it (issue #1746).
        scope: Scope::UserOnly,
        kind: ValueKind::Bool,
        env: None,
        default: Some("true"),
        lower_only: false,
    },
    KeyDef {
        name: "sessions.archive_days",
        // Read by the rotator's daily run as well as `sessions archive`, so
        // it has no variable, for the same reason as the switch: a rotator
        // keeps the environment it was started with, and the two would
        // disagree (issue #1746). A test pins the default to
        // `sessions::config::DEFAULT_DAYS`.
        scope: Scope::UserOnly,
        kind: ValueKind::Int {
            min: 1,
            max: u32::MAX as u64,
        },
        env: None,
        default: Some("3"),
        lower_only: false,
    },
    KeyDef {
        name: "sessions.archive_dest",
        // A path, so user-only; no variable, as above, so `sessions
        // restore` in a shell looks where the daily run wrote. Unset means
        // `~/.amplihack/session-archive`.
        scope: Scope::UserOnly,
        kind: ValueKind::AbsPath,
        env: None,
        default: None,
        lower_only: false,
    },
    KeyDef {
        // Issue #3247: the routing reminder, the session-start text that
        // sends work to the orchestrator, and the router lines of the launch
        // contract. The hook program reads it through the same shared code
        // (`amplihack_utils::user_config::text_switch`), where an unknown
        // value leaves the default in force.
        name: text_switch::ROUTING.key,
        scope: Scope::UserAndProject,
        kind: ValueKind::Choice(text_switch::VALUES),
        env: Some(text_switch::ROUTING.env),
        default: Some(text_switch::DEFAULT),
        lower_only: false,
    },
    KeyDef {
        // Issue #3247: the preferences table the prompt-event hook adds.
        name: text_switch::PROMPT_PREFERENCES.key,
        scope: Scope::UserAndProject,
        kind: ValueKind::Choice(text_switch::VALUES),
        env: Some(text_switch::PROMPT_PREFERENCES.env),
        default: Some(text_switch::DEFAULT),
        lower_only: false,
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
        // Checked on whatever the caller passes, and the value stored is
        // the one checked. The file path hands over the raw input; the
        // environment path trims first (`load.rs`), which rejects everything
        // raw validation would and stores the trimmed value, so neither can
        // hide a leading dash from a later reader.
        ValueKind::ModelId => crate::commands::launch::model_tiers::table::validate_model_id(raw)
            .map(|()| Value::Text(raw.to_string()))
            .map_err(|_| kind_requirement(kind)),
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
        // A list is never typed on the command line or read from a
        // variable, so the reason says where it is set instead of
        // describing a value the caller cannot supply here.
        ValueKind::CommandList => Err("is a list and is set by editing the user file \
             ~/.amplihack/config.toml, not from the command line"
            .to_string()),
        ValueKind::EntryList => Err("is a list; use `amplihack directory enable` and \
             `amplihack directory disable` to change individual entries"
            .to_string()),
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
            | ValueKind::ModelId
            | ValueKind::Choice(_)
            | ValueKind::SessionPattern,
            toml_edit::Value::String(s),
        ) => parse_raw(kind, s.value()),
        (ValueKind::CommandList, toml_edit::Value::Array(array)) => {
            let mut entries = Vec::with_capacity(array.len());
            for item in array.iter() {
                let Some(text) = item.as_str() else {
                    return Err(kind_requirement(kind));
                };
                entries.push(text.to_string());
            }
            parse_command_list(&entries)
        }
        (ValueKind::EntryList, toml_edit::Value::Array(array)) => {
            let mut entries = std::collections::BTreeSet::new();
            for item in array.iter() {
                let id = item
                    .as_str()
                    .ok_or_else(|| kind_requirement(kind))?
                    .parse::<amplihack_utils::list_selection::EntryId>()
                    .map_err(|_| kind_requirement(kind))?;
                if !id.is_default() {
                    entries.insert(id.to_string());
                }
            }
            Ok(Value::List(entries.into_iter().collect()))
        }
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
        // The length comes from the checker itself, so the requirement this
        // prints cannot outlive a change to it.
        ValueKind::ModelId => format!(
            "must be a model id of at most {} letters, digits and '.', '_', '-', '[', ']', \
             '@' or ':', not starting with '-'",
            crate::commands::launch::model_tiers::table::MAX_MODEL_ID_BYTES
        ),
        ValueKind::AbsPath => format!(
            "must be an absolute path without '..', control characters or NUL, at most \
             {MAX_PATH_BYTES} bytes"
        ),
        ValueKind::Bool => "must be true or false".to_string(),
        ValueKind::EntryList => {
            "must be a TOML array of skill:<name> or agent:<bundle-relative-path> strings"
                .to_string()
        }
        ValueKind::CommandList => format!(
            "must be a TOML array of at most {MAX_DENY_COMMANDS} command prefixes, each 1 to \
             {} bytes of ASCII letters, digits and the characters . _ - / @ = +, holding at \
             most {} words separated by exactly one space",
            crate::policy::prefix::MAX_BYTES,
            crate::policy::prefix::MAX_TOKENS
        ),
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

/// Validate the entries of a `policy.deny_commands` value (issue #2589).
///
/// One bad entry rejects the whole value: an entry silently dropped from a
/// deny list is worse than a value refused with a reason, because the list
/// that remains would still be believed. The reason never quotes the entry,
/// which is attacker-chosen; the caller escapes it when it shows it.
pub(crate) fn parse_command_list(entries: &[String]) -> Result<Value, String> {
    let requirement = || kind_requirement(ValueKind::CommandList);
    if entries.len() > MAX_DENY_COMMANDS {
        return Err(requirement());
    }
    for entry in entries {
        crate::policy::Prefix::try_from(entry.as_str()).map_err(|_| requirement())?;
    }
    Ok(Value::List(entries.to_vec()))
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
