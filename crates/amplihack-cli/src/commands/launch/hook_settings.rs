//! Issue #2279 (C9, D5): `amplihack claude` passes amplihack's hooks block
//! as a settings source of its own, `--settings <AMPLIHACK_HOME>/runtime/
//! launch-hooks-settings.json`, so a stripped `~/.claude/settings.json`
//! cannot leave a session without amplihack's hooks.
//!
//! The file holds only `{"hooks": block}` (no `env`, no other key), is
//! written at mode 0600 in a 0700 `runtime/` directory only when its bytes
//! change, and is passed by path, never as inline JSON (#2030). Claude Code
//! runs identical hook commands from several settings sources once, and the
//! home file and this file are both built by `hook_block::block`, so their
//! commands are byte-identical.

use crate::commands::install::hook_block::{HookTarget, block, hook_target, reason};
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The environment setting naming an optional Claude hooks settings file.
const EXTRA_HOOKS_FILE_ENV: &str = "AMPLIHACK_EXTRA_HOOKS_FILE";

/// The launch file, relative to the amplihack home.
pub(crate) const LAUNCH_FILE: &str = "runtime/launch-hooks-settings.json";

/// The launch file of this process's amplihack home, when that home is
/// known and absolute (SR-11). Doctor reads it; nothing here creates it.
pub(crate) fn launch_file_path() -> Option<PathBuf> {
    super::contract_file::contract_home()
        .ok()
        .filter(|home| home.is_absolute())
        .map(|home| home.join(LAUNCH_FILE))
}

/// Whether and what to pass as `--settings`.
#[derive(Debug)]
pub(super) enum Inject {
    Path(PathBuf),
    Scoped(tempfile::TempPath, Option<Vec<String>>),
    /// The amplihack home or `runtime/` is not private (SR-15). The one skip
    /// the user can fix, so the launch warns about it.
    NotPrivate,
    Skip(&'static str),
    Error(String),
}

/// Validated hooks from the optional caller settings file.
#[derive(Debug)]
pub(super) struct ExtraHooks {
    path: PathBuf,
    events: Map<String, Value>,
    ignored_keys: Vec<String>,
}

/// Everything [`prepare_with`] decides on.
pub(super) struct Inputs<'a> {
    pub binary_name: &'a str,
    pub extra_args: &'a [String],
    /// The LiteLLM proxy (safe mode) is in use; it passes
    /// `--setting-sources ""` and keeps its own behaviour.
    pub proxy: bool,
    /// This binary is older than the install stamp (#1526).
    pub newer_stamp: bool,
    pub target: Option<&'a HookTarget>,
    pub amplihack_home: &'a Path,
    pub execution_dir: &'a Path,
    pub extra_hooks: Option<&'a ExtraHooks>,
}

/// Whether the user passed `--settings` (either form) before a literal `--`.
pub(super) fn user_passed_settings(extra_args: &[String]) -> bool {
    extra_args
        .iter()
        .take_while(|arg| arg.as_str() != "--")
        .any(|arg| arg == "--settings" || arg.starts_with("--settings="))
}

/// The production [`prepare_with`], with its inputs read from this process.
pub(super) fn prepare(
    binary_name: &str,
    extra_args: &[String],
    execution_dir: &Path,
    extra_hooks: Option<&ExtraHooks>,
) -> Inject {
    if binary_name != "claude" {
        return Inject::Skip("not claude");
    }
    let skipped = |why| match extra_hooks {
        Some(extra) if extra.has_handlers() => Inject::Error(format!(
            "cannot apply extra hooks file {}: {why}",
            extra.path.display()
        )),
        _ => Inject::Skip(why),
    };
    let Ok(amplihack_home) = super::contract_file::contract_home() else {
        return skipped("no amplihack home");
    };
    if !amplihack_home.is_absolute() {
        return skipped("amplihack home is not absolute");
    }
    let proxy = amplihack_utils::litellm_proxy::proxy_requested();
    // An unreadable stamp counts as newer: never write over a newer install.
    let newer_stamp = !crate::commands::install::binary_is_current();
    let configured = extra_hooks.is_some_and(ExtraHooks::has_handlers);
    let target = if newer_stamp || proxy || (!configured && user_passed_settings(extra_args)) {
        None
    } else {
        hook_target()
    };
    let inject = prepare_with(&Inputs {
        binary_name,
        extra_args,
        proxy,
        newer_stamp,
        target: target.as_ref(),
        amplihack_home: &amplihack_home,
        execution_dir,
        extra_hooks,
    });
    match inject {
        Inject::Skip(why) => skipped(why),
        Inject::NotPrivate if extra_hooks.is_some_and(ExtraHooks::has_handlers) => {
            skipped("the launch settings directory is not private")
        }
        inject => inject,
    }
}

/// Writes the launch file when it applies and returns its path, or why no
/// `--settings` is passed. A failed write logs one warning; the launch goes
/// on without the flag.
pub(super) fn prepare_with(i: &Inputs<'_>) -> Inject {
    if i.binary_name != "claude" {
        return Inject::Skip("not claude");
    }
    if i.proxy {
        return Inject::Skip("LiteLLM proxy in use");
    }
    let extra = i.extra_hooks.filter(|extra| extra.has_handlers());
    if extra.is_none() && user_passed_settings(i.extra_args) {
        eprintln!(
            "amplihack: you passed --settings, so amplihack does not add its hooks settings \
             file; ~/.claude/settings.json still holds the hooks."
        );
        return Inject::Skip("user passed --settings");
    }
    if i.newer_stamp {
        return Inject::Skip("older binary than the install stamp");
    }
    let Some(target) = i.target else {
        return Inject::Skip("no hooks binary");
    };
    if !home_is_private(i.amplihack_home) {
        return Inject::NotPrivate;
    }
    let path = i.amplihack_home.join(LAUNCH_FILE);
    if let Some(extra) = extra {
        let (settings, args) = match scoped_settings(i, target, extra) {
            Ok(settings) => settings,
            Err(error) => return Inject::Error(error),
        };
        return match write_scoped(&path, &settings) {
            Ok(path) => Inject::Scoped(path, args),
            Err(WriteError::NotPrivate) => Inject::NotPrivate,
            Err(WriteError::Io(why)) => Inject::Error(format!(
                "could not write extra hook launch settings in {}: {why}",
                i.amplihack_home.display()
            )),
        };
    }
    match write_if_changed(&path, target) {
        Ok(()) => Inject::Path(path),
        Err(WriteError::NotPrivate) => Inject::NotPrivate,
        Err(WriteError::Io(reason)) => {
            tracing::warn!("launch hooks settings file not written: {reason}");
            eprintln!(
                "amplihack: warning: {} not written ({reason}); launching without it.",
                path.display()
            );
            Inject::Skip("write failed")
        }
    }
}

enum WriteError {
    NotPrivate,
    Io(String),
}

/// The amplihack home must be a real directory owned by this user that no
/// group or other user can write (SR-15).
#[cfg(unix)]
fn home_is_private(home: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(home).is_ok_and(|m| {
        m.is_dir() && m.uid() == amplihack_utils::secure_files::euid() && m.mode() & 0o022 == 0
    })
}

#[cfg(not(unix))]
fn home_is_private(_home: &Path) -> bool {
    false
}

/// Creates or checks `runtime/` (0700, ours, not a symlink), tightening a
/// directory of ours that an older install left at a wider mode.
#[cfg(unix)]
fn private_runtime_dir(dir: &Path) -> Result<(), WriteError> {
    use amplihack_utils::secure_files::{ensure_private_dir, euid};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    if ensure_private_dir(dir).is_ok() {
        return Ok(());
    }
    let meta = std::fs::symlink_metadata(dir).map_err(|_| WriteError::NotPrivate)?;
    if meta.file_type().is_symlink() || !meta.is_dir() || meta.uid() != euid() {
        return Err(WriteError::NotPrivate);
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| WriteError::Io(format!("runtime dir mode not set: {}", e.kind())))?;
    tracing::info!("tightened {} to mode 0700", dir.display());
    ensure_private_dir(dir).map_err(|_| WriteError::NotPrivate)
}

#[cfg(unix)]
fn write_if_changed(path: &Path, target: &HookTarget) -> Result<(), WriteError> {
    use amplihack_utils::secure_files::{read_bounded_nofollow, write_private_atomic};
    let dir = path.parent().ok_or(WriteError::NotPrivate)?;
    private_runtime_dir(dir)?;
    let bytes = settings_bytes(&serde_json::json!({ "hooks": block(target) }))?;
    let limit = bytes.len() as u64 + 1;
    if let Ok(Some((current, _))) = read_bounded_nofollow(path, limit)
        && current == bytes
    {
        return Ok(());
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or(WriteError::NotPrivate)?;
    write_private_atomic(dir, name, &bytes).map_err(|e| WriteError::Io(reason(&e)))
}

#[cfg(not(unix))]
fn write_if_changed(_path: &Path, _target: &HookTarget) -> Result<(), WriteError> {
    Err(WriteError::NotPrivate)
}

#[cfg(unix)]
fn settings_bytes(settings: &Value) -> Result<Vec<u8>, WriteError> {
    let mut bytes = serde_json::to_vec_pretty(settings)
        .map_err(|e| WriteError::Io(format!("serialise: {:?}", e.classify())))?;
    bytes.push(b'\n');
    Ok(bytes)
}

// The shared file is still used by unconfigured launches. Give each child
// that takes extra hooks its own settings, held until the child exits.
#[cfg(unix)]
fn write_scoped(path: &Path, value: &Value) -> Result<tempfile::TempPath, WriteError> {
    use std::io::Write;
    let dir = path.parent().ok_or(WriteError::NotPrivate)?;
    private_runtime_dir(dir)?;
    let mut settings = tempfile::Builder::new()
        .prefix("launch-hooks-")
        .suffix(".json")
        .tempfile_in(dir)
        .map_err(|e| WriteError::Io(reason(&e)))?;
    settings
        .write_all(&settings_bytes(value)?)
        .map_err(|e| WriteError::Io(reason(&e)))?;
    Ok(settings.into_temp_path())
}

#[cfg(not(unix))]
fn write_scoped(_path: &Path, _value: &Value) -> Result<tempfile::TempPath, WriteError> {
    Err(WriteError::NotPrivate)
}

/// Read, validate, and retain only the hooks member of a caller settings file.
pub(super) fn load_extra_hooks(path: &Path) -> Result<ExtraHooks, String> {
    let object = read_settings(path, "extra hooks file")?;
    let ignored_keys = object
        .keys()
        .filter(|key| key.as_str() != "hooks")
        .cloned()
        .collect();
    let events = match object.get("hooks") {
        None => Map::new(),
        Some(hooks) => validate_hooks(hooks, path)?,
    };
    Ok(ExtraHooks {
        path: path.to_path_buf(),
        events,
        ignored_keys,
    })
}

fn read_settings(path: &Path, label: &str) -> Result<Map<String, Value>, String> {
    let context = format!("{label} {}", path.display());
    let file = open_extra_hooks(path).map_err(|error| format!("cannot open {context}: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect {context}: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("{context} is not a regular file"));
    }
    #[cfg(unix)]
    {
        use crate::commands::install::hook_block::owner_refusal;
        use std::os::unix::fs::MetadataExt;
        if let Some(reason) = owner_refusal(metadata.uid(), amplihack_utils::secure_files::euid()) {
            return Err(format!("{context} is {reason}"));
        }
        if metadata.mode() & 0o022 != 0 {
            return Err(format!("{context} is writable by group or other users"));
        }
        if metadata.mode() & 0o400 == 0 {
            return Err(format!("{context} is not readable"));
        }
    }
    let value: Value = serde_json::from_reader(std::io::BufReader::new(file)).map_err(|error| {
        let reason = if error.is_io() {
            "could not be read"
        } else {
            "has invalid JSON"
        };
        format!(
            "{context} {reason} at line {}, column {}",
            error.line(),
            error.column()
        )
    })?;
    match value {
        Value::Object(object) => Ok(object),
        _ => Err(format!("{context} must contain a JSON object")),
    }
}

#[cfg(unix)]
fn open_extra_hooks(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| {
            if error.raw_os_error() == Some(libc::ELOOP) {
                amplihack_utils::secure_files::invalid("is a symlink")
            } else {
                error
            }
        })
}

#[cfg(not(unix))]
fn open_extra_hooks(_path: &Path) -> std::io::Result<std::fs::File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "cannot verify file ownership on this platform",
    ))
}

impl ExtraHooks {
    pub(super) fn from_env() -> anyhow::Result<Option<Self>> {
        let Some(path) = std::env::var_os(EXTRA_HOOKS_FILE_ENV) else {
            return Ok(None);
        };
        anyhow::ensure!(
            !path.is_empty(),
            "{EXTRA_HOOKS_FILE_ENV} names an empty file path"
        );
        let extra = load_extra_hooks(Path::new(&path)).map_err(anyhow::Error::msg)?;
        if !extra.ignored_keys.is_empty() {
            eprintln!(
                "amplihack: ignored settings keys in extra hooks file: {:?}",
                extra.ignored_keys
            );
        }
        Ok(Some(extra))
    }

    pub(super) fn events(&self) -> &Map<String, Value> {
        &self.events
    }

    fn has_handlers(&self) -> bool {
        self.events
            .values()
            .filter_map(Value::as_array)
            .flatten()
            .any(|group| {
                group["hooks"]
                    .as_array()
                    .is_some_and(|hooks| !hooks.is_empty())
            })
    }
}

fn validate_hooks(value: &Value, path: &Path) -> Result<Map<String, Value>, String> {
    let invalid = |detail: &str| {
        format!(
            "extra hooks file {} has an invalid hooks schema: {detail}",
            path.display()
        )
    };
    let events = value
        .as_object()
        .ok_or_else(|| invalid("hooks must be an object"))?;
    for (event, groups) in events {
        if !CLAUDE_EVENTS.contains(&event.as_str()) {
            return Err(invalid(&format!("unknown event {event:?}")));
        }
        let groups = groups
            .as_array()
            .ok_or_else(|| invalid(&format!("{event} must be an array")))?;
        for group in groups {
            let handlers = group.get("hooks").and_then(Value::as_array);
            let valid_group = group.as_object().is_some_and(|group| {
                group.iter().all(|(key, value)| match key.as_str() {
                    "hooks" => true,
                    "matcher" => value.is_string(),
                    _ => false,
                })
            });
            let handlers = handlers.filter(|_| valid_group).ok_or_else(|| {
                invalid(&format!(
                    "{event} must contain matcher groups with a hooks array"
                ))
            })?;
            for handler in handlers {
                if !valid_command(handler) {
                    return Err(invalid(&format!(
                        "{event} contains an invalid command handler"
                    )));
                }
            }
        }
    }
    Ok(events.clone())
}

// Claude Code hooks reference, including events that Codex does not implement.
const CLAUDE_EVENTS: &[&str] = &[
    "SessionStart",
    "Setup",
    "UserPromptSubmit",
    "UserPromptExpansion",
    "PreSkillShellCommand",
    "PreToolUse",
    "PermissionRequest",
    "PermissionDenied",
    "PostToolUse",
    "PostToolUseFailure",
    "PostToolBatch",
    "Notification",
    "MessageDisplay",
    "SubagentStart",
    "SubagentStop",
    "TaskCreated",
    "TaskCompleted",
    "Stop",
    "StopFailure",
    "TeammateIdle",
    "InstructionsLoaded",
    "ConfigChange",
    "CwdChanged",
    "DirectoryAdded",
    "FileChanged",
    "WorktreeCreate",
    "WorktreeRemove",
    "PreCompact",
    "PostCompact",
    "PreModelSwitch",
    "PostModelSwitch",
    "Elicitation",
    "ElicitationResult",
    "SessionEnd",
];

fn valid_command(handler: &Value) -> bool {
    handler["type"] == "command"
        && handler["command"]
            .as_str()
            .is_some_and(|command| !command.trim().is_empty() && !command.contains('\0'))
        && handler.as_object().is_some_and(|handler| {
            handler.iter().all(|(key, value)| match key.as_str() {
                "type" | "command" => true,
                "if" | "statusMessage" => value.is_string(),
                "once" | "async" | "asyncRewake" => value.is_boolean(),
                "timeout" => value.as_f64().is_some_and(|seconds| seconds > 0.0),
                "shell" => matches!(value.as_str(), Some("bash" | "powershell")),
                "onFailure" => matches!(value.as_str(), Some("continue" | "block")),
                "args" => value.as_array().is_some_and(|args| {
                    args.iter()
                        .all(|arg| arg.as_str().is_some_and(|arg| !arg.contains('\0')))
                }),
                _ => false,
            })
        })
}

fn scoped_settings(
    inputs: &Inputs<'_>,
    target: &HookTarget,
    extra: &ExtraHooks,
) -> Result<(Value, Option<Vec<String>>), String> {
    let (mut settings, args) = if user_passed_settings(inputs.extra_args) {
        let (source, args) = separate_settings_arg(inputs.extra_args)?;
        let settings = if source.trim_start().starts_with('{') {
            serde_json::from_str(source)
                .map_err(|_| "invalid JSON object passed to --settings".to_owned())?
        } else {
            read_settings(&inputs.execution_dir.join(source), "--settings file")?
        };
        (settings, Some(args))
    } else {
        (Map::new(), None)
    };
    if settings.get("disableAllHooks") == Some(&Value::Bool(true)) {
        return Err("--settings disables all hooks".to_owned());
    }
    let mut hooks = block(target);
    let own_events = hooks
        .as_object_mut()
        .expect("amplihack hooks are an object");
    if let Some(existing) = settings.remove("hooks") {
        let Value::Object(existing) = existing else {
            return Err("--settings hooks must be an object".to_owned());
        };
        for (event, groups) in existing {
            let Value::Array(groups) = groups else {
                return Err(format!("--settings hooks for {event:?} must be an array"));
            };
            event_groups(own_events, &event).extend(groups);
        }
    }
    for (event, groups) in extra.events() {
        let destination = event_groups(own_events, event);
        let mut seen = commands_in(destination);
        destination.extend(unique_groups(groups, &mut seen));
    }
    settings.insert("hooks".into(), hooks);
    Ok((Value::Object(settings), args))
}

fn separate_settings_arg(args: &[String]) -> Result<(&str, Vec<String>), String> {
    let mut source = None;
    let mut remaining = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            remaining.push(arg.clone());
            remaining.extend(args.cloned());
            break;
        }
        if arg == "--settings" {
            source = args
                .next()
                .map(String::as_str)
                .filter(|value| !value.starts_with("--"));
            if source.is_none() {
                break;
            }
        } else if let Some(value) = arg.strip_prefix("--settings=") {
            source = Some(value);
        } else {
            remaining.push(arg.clone());
        }
    }
    let source = source
        .filter(|source| !source.is_empty())
        .ok_or_else(|| "--settings needs a JSON object or a file path".to_owned())?;
    Ok((source, remaining))
}

fn event_groups<'a>(events: &'a mut Map<String, Value>, event: &str) -> &'a mut Vec<Value> {
    events
        .entry(event)
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .expect("hook event is an array")
}

pub(super) fn unique_groups(groups: &Value, seen: &mut HashSet<String>) -> Vec<Value> {
    let groups = groups.as_array().expect("validated hook event is an array");
    let mut unique = Vec::new();
    for group in groups {
        let mut group = group.clone();
        let handlers = group["hooks"]
            .as_array_mut()
            .expect("validated command array");
        handlers.retain(|handler| {
            let command = handler["command"].as_str().expect("validated hook command");
            seen.insert(command.to_owned())
        });
        if !handlers.is_empty() {
            unique.push(group);
        }
    }
    unique
}

pub(super) fn commands_in(groups: &[Value]) -> HashSet<String> {
    groups
        .iter()
        .filter_map(|group| group.get("hooks"))
        .filter_map(Value::as_array)
        .flat_map(|handlers| handlers.iter())
        .filter(|handler| handler.get("type").and_then(Value::as_str) == Some("command"))
        .filter_map(|handler| handler.get("command"))
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}
