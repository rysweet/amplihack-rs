//! Loading the layered config: env > project > user > legacy > default, with
//! every problem in a hand-edited file reported, never fatal.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use amplihack_utils::agent_binary::{BINARY_ENV, InheritedBinary, SOURCE_ENV, classify_inherited};

use super::edit::parse_document;
use super::files::{Project, project, same_file, user_file};
use super::migrate::{self, LegacyLayer};
use super::safe::{insecure_reason, read_bounded};
use super::schema::{KEYS, KeyDef, Scope, Value, ValueKind, from_toml, key, parse_raw};
use super::{Diagnostic, Origin};

/// Everything a load depends on, so tests can supply it.
pub(crate) struct LoadCtx<'a> {
    /// `$HOME`. `None`: no user file and no legacy file.
    pub home: Option<PathBuf>,
    /// Where the project-file search starts.
    pub cwd: PathBuf,
    /// Whether to read the project layer at all.
    pub read_project: bool,
    /// The environment.
    pub env: &'a dyn Fn(&str) -> Option<String>,
    /// Whether to run the legacy migration.
    pub migrate: bool,
}

/// One key's effective value.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    pub key: &'static KeyDef,
    /// `None` is "(unset)".
    pub value: Option<Value>,
    pub origin: Origin,
    /// What the files alone say (project, user, legacy), with its origin.
    pub file: Option<(Value, Origin)>,
}

/// The loaded config.
#[derive(Debug)]
pub(crate) struct Effective {
    /// One entry per schema key, in schema order.
    pub entries: Vec<Entry>,
    pub diagnostics: Vec<Diagnostic>,
    pub user_file: Option<PathBuf>,
    pub project: Project,
    /// The legacy file while it still holds keys amplihack reads.
    pub legacy: Option<LegacyLayer>,
}

impl Effective {
    pub(crate) fn entry(&self, name: &str) -> &Entry {
        self.entries
            .iter()
            .find(|entry| entry.key.name == name)
            .expect("every schema key has an entry")
    }

    /// The value the files give `name`, ignoring env and default.
    pub(crate) fn file_value(&self, name: &str) -> Option<&Value> {
        self.entry(name).file.as_ref().map(|(value, _)| value)
    }
}

/// Which file a layer came from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Layer {
    User,
    Project,
}

/// Read one config file into validated values, reporting every problem.
fn read_layer(
    path: &Path,
    layer: Layer,
    diagnostics: &mut Vec<Diagnostic>,
) -> HashMap<&'static str, Value> {
    let mut values = HashMap::new();
    let text = match read_bounded(path) {
        Ok(Some(text)) => text,
        Ok(None) => return values,
        Err(reason) => {
            diagnostics.push(Diagnostic::file(path, reason));
            return values;
        }
    };
    let doc = match parse_document(&text) {
        Ok(doc) => doc,
        Err(reason) => {
            diagnostics.push(Diagnostic::file(path, format!("{reason}; file ignored")));
            return values;
        }
    };
    let insecure = match layer {
        Layer::User => insecure_reason(path),
        Layer::Project => None,
    };
    for (table, item) in doc.as_table().iter() {
        let Some(entries) = item.as_table_like() else {
            diagnostics.push(Diagnostic::key(path, table, "unknown key".to_string()));
            continue;
        };
        for (name, item) in entries.iter() {
            let full = format!("{table}.{name}");
            let Some(def) = key(&full) else {
                diagnostics.push(Diagnostic::key(path, &full, "unknown key".to_string()));
                continue;
            };
            if layer == Layer::Project && def.scope == Scope::UserOnly {
                diagnostics.push(Diagnostic::key(
                    path,
                    &full,
                    "can only be set in the user file; ignored".to_string(),
                ));
                continue;
            }
            if def.scope == Scope::UserOnly
                && let Some(reason) = insecure
            {
                diagnostics.push(Diagnostic::key(path, &full, reason.to_string()));
                continue;
            }
            let parsed = item
                .as_value()
                .ok_or_else(|| super::schema::kind_requirement(def.kind))
                .and_then(|value| from_toml(def.kind, value));
            match parsed {
                Ok(value) => {
                    values.insert(def.name, value);
                }
                Err(reason) => diagnostics.push(Diagnostic::key(path, &full, reason)),
            }
        }
    }
    values
}

/// Load the config described by `ctx`. Every diagnostic is also printed to
/// stderr, once per process.
pub(crate) fn load(ctx: &LoadCtx<'_>) -> Effective {
    let mut diagnostics = Vec::new();
    let legacy = match &ctx.home {
        Some(home) => {
            let outcome = migrate::run(home, ctx.migrate);
            diagnostics.extend(outcome.diagnostics);
            outcome.layer
        }
        None => None,
    };
    let user_path = ctx.home.as_deref().map(user_file);
    let user = user_path
        .as_deref()
        .map(|path| read_layer(path, Layer::User, &mut diagnostics))
        .unwrap_or_default();
    let project_info = project(&ctx.cwd);
    let project_path = match (&project_info, ctx.read_project) {
        (Project::File { path, found: true }, true)
            if !user_path
                .as_deref()
                .is_some_and(|user| same_file(user, path)) =>
        {
            Some(path.clone())
        }
        _ => None,
    };
    let project_values = project_path
        .as_deref()
        .map(|path| read_layer(path, Layer::Project, &mut diagnostics))
        .unwrap_or_default();

    let entries = KEYS
        .iter()
        .map(|def| {
            let user_value = user
                .get(def.name)
                .cloned()
                .zip(user_path.clone().map(Origin::User))
                .or_else(|| legacy_value(def, legacy.as_ref()));
            let project_value = project_values
                .get(def.name)
                .cloned()
                .zip(project_path.clone().map(Origin::Project));
            let file = merge_files(def, user_value, project_value, &mut diagnostics);
            resolve_entry(def, file, ctx.env)
        })
        .collect();

    for diagnostic in &diagnostics {
        warn_once(diagnostic);
    }
    Effective {
        entries,
        diagnostics,
        user_file: user_path,
        project: project_info,
        legacy,
    }
}

/// The legacy layer's value for `def`: only `node.heap_consent`.
fn legacy_value(def: &KeyDef, legacy: Option<&LegacyLayer>) -> Option<(Value, Origin)> {
    let legacy = legacy?;
    if def.name != "node.heap_consent" {
        return None;
    }
    legacy
        .consent
        .map(|consent| (Value::Bool(consent), Origin::Legacy(legacy.path.clone())))
}

/// Combine the user (or legacy) and project values. The project file wins,
/// except that for a lower-only key it can only lower the user value, or the
/// default when the user file does not set one.
fn merge_files(
    def: &'static KeyDef,
    user: Option<(Value, Origin)>,
    project: Option<(Value, Origin)>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<(Value, Origin)> {
    let Some((project_value, project_origin)) = project else {
        return user;
    };
    if !def.lower_only {
        return Some((project_value, project_origin));
    }
    let base = user
        .as_ref()
        .map(|(value, _)| (value.clone(), "user value"))
        .or_else(|| def.default_value().map(|value| (value, "default")));
    match (base, project_value.as_int()) {
        (Some((base_value, label)), Some(lowered))
            if base_value.as_int().is_some_and(|base| lowered > base) =>
        {
            if let Origin::Project(path) = &project_origin {
                diagnostics.push(Diagnostic::key(
                    path,
                    def.name,
                    format!(
                        "{lowered} is above the {label} {base_value}; a project file can only \
                         lower it"
                    ),
                ));
            }
            user
        }
        _ => Some((project_value, project_origin)),
    }
}

/// Put the environment above the files and the default below them.
fn resolve_entry(
    def: &'static KeyDef,
    file: Option<(Value, Origin)>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Entry {
    let below = || match &file {
        Some((value, origin)) => (Some(value.clone()), origin.clone()),
        None => (def.default_value(), Origin::Default),
    };
    let (value, origin) = match env_layer(def, env) {
        EnvLayer::Unset => below(),
        EnvLayer::Valid(var, value) => (Some(value), Origin::Env(var)),
        EnvLayer::Invalid(var, reason) => {
            let (value, _) = below();
            (value, Origin::EnvInvalid { var, reason })
        }
    };
    Entry {
        key: def,
        value,
        origin,
        file,
    }
}

enum EnvLayer {
    Unset,
    Valid(&'static str, Value),
    Invalid(&'static str, String),
}

fn env_layer(def: &KeyDef, env: &dyn Fn(&str) -> Option<String>) -> EnvLayer {
    let Some(var) = def.env else {
        return EnvLayer::Unset;
    };
    let Some(raw) = env(var).filter(|raw| !raw.is_empty()) else {
        return EnvLayer::Unset;
    };
    if def.kind == ValueKind::AgentBinary {
        // Only an explicit value is the env layer; a value amplihack exported
        // with a tag describes another layer (issue #1606).
        return match classify_inherited(env(BINARY_ENV).as_deref(), env(SOURCE_ENV).as_deref()) {
            InheritedBinary::Explicit(binary) => EnvLayer::Valid(var, Value::Text(binary)),
            InheritedBinary::Absent => EnvLayer::Invalid(
                var,
                format!("{var} is not one of amplifier, claude, codex or copilot"),
            ),
            _ => EnvLayer::Unset,
        };
    }
    // A boolean variable also takes `1` and `0`, the usual shell spelling.
    let raw = match (def.kind, raw.trim()) {
        (ValueKind::Bool, "1") => "true",
        (ValueKind::Bool, "0") => "false",
        (_, raw) => raw,
    };
    match parse_raw(def.kind, raw) {
        Ok(value) => EnvLayer::Valid(var, value),
        Err(_) => EnvLayer::Invalid(var, env_reason(def.kind, var, raw)),
    }
}

/// Why an environment value is invalid, without quoting it.
fn env_reason(kind: ValueKind, var: &str, raw: &str) -> String {
    match kind {
        ValueKind::Int { .. } if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) => {
            format!("{var} is not an integer")
        }
        ValueKind::Int { min, max } if max == u64::MAX => format!("{var} is below {min}"),
        ValueKind::Int { min, max } => format!("{var} is outside {min} to {max}"),
        ValueKind::AbsPath => format!("{var} is not a valid absolute path"),
        ValueKind::Bool => format!("{var} is not true, false, 1 or 0"),
        ValueKind::Choice(_) | ValueKind::SessionPattern => format!("{var} is not a valid value"),
        ValueKind::AgentBinary => {
            format!("{var} is not one of amplifier, claude, codex or copilot")
        }
    }
}

/// Print `diagnostic` to stderr unless this process already has.
fn warn_once(diagnostic: &Diagnostic) {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let line = diagnostic.to_string();
    let first = SEEN
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(line.clone());
    if first {
        eprintln!("warning: {line}");
    }
}
