//! Hook wrapper construction, matching, and JSON helpers.

use super::types::{HookCommandKind, HookSpec};
use super::binary::{validate_binary_path, validate_hook_command_string};
use serde_json::{Map, Value};
use std::path::Path;

pub(super) fn update_hook_paths(
    settings: &mut Value,
    hook_system: &str,
    specs: &[HookSpec],
    hooks_bin: &Path,
) {
    let root = ensure_object(settings);
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let hooks = ensure_object(hooks);

    for spec in specs {
        let wrappers = hooks
            .entry(spec.event)
            .or_insert_with(|| Value::Array(Vec::new()));
        let wrappers = ensure_array(wrappers);
        let desired = build_hook_wrapper(spec, hooks_bin);

        if let Some(existing) = wrappers
            .iter_mut()
            .find(|wrapper| wrapper_matches(wrapper, spec, hook_system))
        {
            *existing = desired;
        } else {
            wrappers.push(desired);
        }
    }
}

/// Build the Claude Code hook wrapper JSON for a given spec.
///
/// For `BinarySubcmd`: command = `"{hooks_bin} {subcmd}"`
/// Wrap a path string in double quotes, escaping any embedded double quotes.
pub(super) fn shell_quote_path(path_str: &str) -> String {
    let escaped = path_str.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

pub(super) fn build_hook_wrapper(spec: &HookSpec, hooks_bin: &Path) -> Value {
    let command_str = match &spec.cmd {
        HookCommandKind::BinarySubcmd { subcmd } => {
            let bin_str = hooks_bin.display().to_string();
            validate_binary_path(&bin_str)
                .expect("hooks binary path must not contain shell-unsafe characters");
            let quoted_bin = shell_quote_path(&bin_str);
            format!("{quoted_bin} {subcmd}")
        }
    };
    validate_hook_command_string(&command_str)
        .expect("hook command strings are built from controlled paths and literals");

    let mut hook = Map::new();
    hook.insert("type".to_string(), Value::String("command".to_string()));
    hook.insert("command".to_string(), Value::String(command_str));
    if let Some(timeout) = spec.timeout {
        hook.insert("timeout".to_string(), Value::Number(timeout.into()));
    }

    let mut wrapper = Map::new();
    if let Some(matcher) = spec.matcher {
        wrapper.insert("matcher".to_string(), Value::String(matcher.to_string()));
    }
    wrapper.insert("hooks".to_string(), Value::Array(vec![Value::Object(hook)]));
    Value::Object(wrapper)
}

pub(super) fn legacy_hook_script_name(subcmd: &str) -> Option<&'static str> {
    match subcmd {
        "session-start" => Some("session_start.py"),
        "stop" => Some("stop.py"),
        "pre-tool-use" => Some("pre_tool_use.py"),
        "post-tool-use" => Some("post_tool_use.py"),
        "workflow-classification-reminder" => Some("workflow_classification_reminder.py"),
        "user-prompt-submit" => Some("user_prompt_submit.py"),
        "pre-compact" => Some("pre_compact.py"),
        _ => None,
    }
}

/// Type-directed idempotency check.
///
/// `BinarySubcmd` matches either the native `amplihack-hooks <subcmd>` wrapper or
/// a legacy staged Python hook path for the corresponding hook. This lets a fresh
/// Rust reinstall replace older Python registrations in place instead of appending
/// duplicate wrappers.
pub(super) fn wrapper_matches(wrapper: &Value, spec: &HookSpec, hook_system: &str) -> bool {
    let Some(wrapper_obj) = wrapper.as_object() else {
        return false;
    };

    let matcher_matches = match spec.matcher {
        Some(expected) => wrapper_obj.get("matcher").and_then(Value::as_str) == Some(expected),
        None => !wrapper_obj.contains_key("matcher"),
    };
    if !matcher_matches {
        return false;
    }

    let command = wrapper_obj
        .get("hooks")
        .and_then(Value::as_array)
        .and_then(|entries| entries.first())
        .and_then(Value::as_object)
        .and_then(|hook| hook.get("command"))
        .and_then(Value::as_str);

    let Some(command) = command else {
        return false;
    };

    match &spec.cmd {
        HookCommandKind::BinarySubcmd { subcmd } => {
            (command.contains("amplihack-hooks") && command.ends_with(subcmd))
                || legacy_hook_script_name(subcmd).is_some_and(|legacy_name| {
                    ((hook_system == "amplihack" && command.contains("tools/amplihack/hooks/"))
                        || (hook_system == "xpia" && command.contains("tools/xpia/hooks/")))
                        && command.contains(legacy_name)
                })
        }
    }
}

pub(super) fn ensure_object(value: &mut Value) -> &mut Map<String, Value> {
    if !value.is_object() {
        *value = Value::Object(Map::new());
    }
    value.as_object_mut().expect("value converted to object")
}

pub(super) fn ensure_array(value: &mut Value) -> &mut Vec<Value> {
    if !value.is_array() {
        *value = Value::Array(Vec::new());
    }
    value.as_array_mut().expect("value converted to array")
}
