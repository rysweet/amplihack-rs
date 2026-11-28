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
pub(crate) const EXTRA_HOOKS_FILE_ENV: &str = "AMPLIHACK_EXTRA_HOOKS_FILE";

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Inject {
    Path(PathBuf),
    /// The amplihack home or `runtime/` is not private (SR-15). The one skip
    /// the user can fix, so the launch warns about it.
    NotPrivate,
    Skip(&'static str),
    Error(String),
}

/// Validated hooks from the optional caller settings file.
#[derive(Debug, Clone)]
pub(crate) struct ExtraHooks {
    hooks: Value,
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
pub(super) fn prepare(binary_name: &str, extra_args: &[String]) -> Inject {
    let extra_hooks = match std::env::var_os(EXTRA_HOOKS_FILE_ENV) {
        None => None,
        Some(path) if path.is_empty() => {
            return Inject::Error(format!("{EXTRA_HOOKS_FILE_ENV} names an empty file path"));
        }
        Some(path) => match load_extra_hooks(Path::new(&path)) {
            Ok(extra) => Some(extra),
            Err(error) => return Inject::Error(error),
        },
    };
    if binary_name != "claude" {
        if binary_name == "codex"
            && extra_hooks.is_some()
            && super::codex_agent_step::exec_subcommand_index(extra_args).is_some()
        {
            eprintln!(
                "amplihack: codex step launched without extra hooks; this launch form has no per-launch hook configuration"
            );
        }
        return Inject::Skip("not claude");
    }
    let Ok(amplihack_home) = super::contract_file::contract_home() else {
        return Inject::Skip("no amplihack home");
    };
    if !amplihack_home.is_absolute() {
        return Inject::Skip("amplihack home is not absolute");
    }
    let proxy = amplihack_utils::litellm_proxy::proxy_requested();
    // An unreadable stamp counts as newer: never write over a newer install.
    let newer_stamp = !crate::commands::install::binary_is_current();
    let target = if newer_stamp || proxy || user_passed_settings(extra_args) {
        None
    } else {
        hook_target()
    };
    prepare_with(&Inputs {
        binary_name,
        extra_args,
        proxy,
        newer_stamp,
        target: target.as_ref(),
        amplihack_home: &amplihack_home,
        extra_hooks: extra_hooks.as_ref(),
    })
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
    if user_passed_settings(i.extra_args) {
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
    if let Some(extra) = i.extra_hooks {
        for key in &extra.ignored_keys {
            eprintln!("amplihack: ignored settings key {key} from extra hooks file");
        }
    }
    if !home_is_private(i.amplihack_home) {
        return Inject::NotPrivate;
    }
    let path = i.amplihack_home.join(LAUNCH_FILE);
    match write_if_changed(&path, target, i.extra_hooks) {
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
fn write_if_changed(
    path: &Path,
    target: &HookTarget,
    extra: Option<&ExtraHooks>,
) -> Result<(), WriteError> {
    use amplihack_utils::secure_files::{read_bounded_nofollow, write_private_atomic};
    let dir = path.parent().ok_or(WriteError::NotPrivate)?;
    private_runtime_dir(dir)?;
    let hooks = merged_hooks(target, extra);
    let mut bytes = serde_json::to_vec_pretty(&serde_json::json!({ "hooks": hooks }))
        .map_err(|e| WriteError::Io(format!("serialise: {:?}", e.classify())))?;
    bytes.push(b'\n');
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
fn write_if_changed(
    _path: &Path,
    _target: &HookTarget,
    _extra: Option<&ExtraHooks>,
) -> Result<(), WriteError> {
    Err(WriteError::NotPrivate)
}

/// Read, validate, and retain only the hooks member of a caller settings file.
pub(super) fn load_extra_hooks(path: &Path) -> Result<ExtraHooks, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("extra hooks file {}: {}", path.display(), error.kind()))?;
    if !metadata.is_file() {
        return Err(format!(
            "extra hooks file {} is not a regular file",
            path.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != amplihack_utils::secure_files::euid() {
            return Err(format!(
                "extra hooks file {} is owned by another user",
                path.display()
            ));
        }
        if metadata.mode() & 0o022 != 0 {
            return Err(format!(
                "extra hooks file {} is writable by group or other users",
                path.display()
            ));
        }
        if metadata.mode() & 0o400 == 0 {
            return Err(format!(
                "extra hooks file {} is not readable",
                path.display()
            ));
        }
    }
    let bytes = std::fs::read(path).map_err(|error| {
        format!(
            "extra hooks file {} is unreadable: {}",
            path.display(),
            error
        )
    })?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "extra hooks file {} has invalid JSON: {error}",
            path.display()
        )
    })?;
    let object = value.as_object().ok_or_else(|| {
        format!(
            "extra hooks file {} must contain a JSON object",
            path.display()
        )
    })?;
    let ignored_keys = object
        .keys()
        .filter(|key| key.as_str() != "hooks")
        .cloned()
        .collect();
    let hooks = match object.get("hooks") {
        None => Value::Object(Map::new()),
        Some(hooks) => validate_hooks(hooks, path)?,
    };
    Ok(ExtraHooks {
        hooks,
        ignored_keys,
    })
}

fn validate_hooks(value: &Value, path: &Path) -> Result<Value, String> {
    let Some(events) = value.as_object() else {
        return Err(format!(
            "extra hooks file {} has an invalid hooks schema",
            path.display()
        ));
    };
    for groups in events.values() {
        let Some(groups) = groups.as_array() else {
            return Err(format!(
                "extra hooks file {} has an invalid hooks schema",
                path.display()
            ));
        };
        for group in groups {
            let Some(group) = group.as_object() else {
                return Err(format!(
                    "extra hooks file {} has an invalid hooks schema",
                    path.display()
                ));
            };
            let Some(handlers) = group.get("hooks").and_then(Value::as_array) else {
                return Err(format!(
                    "extra hooks file {} has an invalid hooks schema",
                    path.display()
                ));
            };
            for handler in handlers {
                let Some(handler) = handler.as_object() else {
                    return Err(format!(
                        "extra hooks file {} has an invalid hooks schema",
                        path.display()
                    ));
                };
                if handler.get("type").and_then(Value::as_str) != Some("command")
                    || handler
                        .get("command")
                        .and_then(Value::as_str)
                        .is_none_or(str::is_empty)
                {
                    return Err(format!(
                        "extra hooks file {} has an invalid hooks schema",
                        path.display()
                    ));
                }
            }
        }
    }
    Ok(value.clone())
}

fn merged_hooks(target: &HookTarget, extra: Option<&ExtraHooks>) -> Value {
    let Some(extra) = extra else {
        return block(target);
    };
    let mut hooks = block(target);
    let Value::Object(extra_events) = &extra.hooks else {
        unreachable!("validated hooks are an object")
    };
    let Value::Object(own_events) = &mut hooks else {
        unreachable!("amplihack hooks are an object")
    };
    for (event, groups) in extra_events {
        let slot = own_events
            .entry(event.clone())
            .or_insert_with(|| Value::Array(Vec::new()));
        let existing_commands = commands_in(slot);
        let mut seen = existing_commands;
        let Some(groups) = groups.as_array() else {
            continue;
        };
        let destination = slot.as_array_mut().expect("hook event is an array");
        for group in groups {
            let mut group = group.clone();
            let Some(handlers) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                continue;
            };
            handlers.retain(|handler| {
                let Some(command) = handler.get("command").and_then(Value::as_str) else {
                    return false;
                };
                seen.insert(command.to_string())
            });
            if !handlers.is_empty() {
                destination.push(group);
            }
        }
    }
    hooks
}

fn commands_in(value: &Value) -> HashSet<String> {
    value
        .as_array()
        .into_iter()
        .flat_map(|groups| groups.iter())
        .filter_map(|group| group.get("hooks"))
        .filter_map(Value::as_array)
        .flat_map(|handlers| handlers.iter())
        .filter_map(|handler| handler.get("command"))
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}
