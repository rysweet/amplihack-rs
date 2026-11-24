//! The generated `hooks.json` of a local-model Codex home (D16).
//!
//! The file holds `PreToolUse` and nothing else: amplihack's own
//! pre-tool-use entry first, then the user's PreToolUse groups from their
//! `hooks.json`, in order and otherwise unchanged, minus the handlers that
//! run amplihack's pre-tool-use hook (the Codex plugin's wrapper, with or
//! without an `env NAME=value` prefix, or any program named
//! `amplihack-hooks`). A handler the filter misses runs the guard twice,
//! which is safe; the filter therefore removes only exact word lists.
//!
//! Pure apart from [`verify_generated`], which checks that the hooks binary
//! named in amplihack's entry is still an executable regular file. No error
//! quotes the file's content: a parse error gives the path, line and column.

use crate::commands::install::paths::sanitize_path;
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path};

const SUBCOMMAND: &str = "pre-tool-use";
const PROTOCOL: &str = "AMPLIHACK_HOOK_PROTOCOL=codex";
const HOOKS_BINARY: &str = "amplihack-hooks";

/// The user's PreToolUse groups from `text` (their `hooks.json` at `path`),
/// with amplihack's own pre-tool-use handlers removed and groups left empty
/// by that dropped. `plugin_root` is `<home>/.amplihack/codex`.
pub(super) fn user_pre_tool_use(text: &str, path: &Path, plugin_root: &Path) -> Result<Vec<Value>> {
    let shown = sanitize_path(path);
    let value: Value = serde_json::from_str(text).map_err(|error| {
        anyhow!(
            "{shown} is not valid JSON (line {}, column {})",
            error.line(),
            error.column()
        )
    })?;
    let Value::Object(root) = value else {
        bail!("{shown}: the top level must be a JSON object");
    };
    let Some(events) = root.get("hooks") else {
        return Ok(Vec::new());
    };
    let Value::Object(events) = events else {
        bail!("{shown}: `hooks` must be a JSON object");
    };
    let Some(groups) = events.get("PreToolUse") else {
        return Ok(Vec::new());
    };
    let Value::Array(groups) = groups else {
        bail!("{shown}: `hooks.PreToolUse` must be a JSON array");
    };
    Ok(groups
        .iter()
        .filter_map(|group| without_amplihack(group, plugin_root))
        .collect())
}

/// `group` without its amplihack pre-tool-use handlers; `None` when removing
/// them left it with none. A group or handler of an unexpected shape is kept
/// as it is.
fn without_amplihack(group: &Value, plugin_root: &Path) -> Option<Value> {
    let Some(handlers) = group.get("hooks").and_then(Value::as_array) else {
        return Some(group.clone());
    };
    let kept: Vec<Value> = handlers
        .iter()
        .filter(|handler| !is_amplihack_handler(handler, plugin_root))
        .cloned()
        .collect();
    if kept.len() == handlers.len() {
        return Some(group.clone());
    }
    if kept.is_empty() {
        return None;
    }
    let mut group = group.clone();
    group["hooks"] = Value::Array(kept);
    Some(group)
}

/// A `type: command` handler whose command runs amplihack's pre-tool-use.
fn is_amplihack_handler(handler: &Value, plugin_root: &Path) -> bool {
    handler.get("type").and_then(Value::as_str) == Some("command")
        && handler
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(|command| runs_amplihack_pre_tool_use(command, plugin_root))
}

/// Whether `command`, split into shell words with one optional leading
/// `env NAME=value …` prefix taken off, is exactly
/// `[program, pre-tool-use]`, where `program` is named `amplihack-hooks` or
/// is an absolute path under `plugin_root` ending in `bin/hook`. A command
/// that cannot be split, or is compound, is not.
pub(super) fn runs_amplihack_pre_tool_use(command: &str, plugin_root: &Path) -> bool {
    let Ok(words) = shell_words::split(command) else {
        return false;
    };
    let mut rest = words.as_slice();
    if let Some((first, tail)) = rest.split_first()
        && first == "env"
    {
        rest = tail;
        while let Some((word, tail)) = rest.split_first()
            && is_plain_assignment(word)
        {
            rest = tail;
        }
    }
    match rest {
        [program, subcommand] if subcommand == SUBCOMMAND => {
            is_amplihack_program(program, plugin_root)
        }
        _ => false,
    }
}

/// `NAME=value` with a shell variable name and a value the shell would not
/// expand (no `$` and no backquote), so removing the handler loses nothing
/// the command would have done.
fn is_plain_assignment(word: &str) -> bool {
    let Some((name, value)) = word.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !value.contains(['$', '`'])
}

fn is_amplihack_program(program: &str, plugin_root: &Path) -> bool {
    let path = Path::new(program);
    if path.file_name().and_then(|name| name.to_str()) == Some(HOOKS_BINARY) {
        return true;
    }
    path.is_absolute()
        && !path.components().any(|c| c == Component::ParentDir)
        && path != plugin_root
        && path.starts_with(plugin_root)
        && path.ends_with("bin/hook")
}

/// The number of handlers in `groups`.
pub(super) fn handler_count(groups: &[Value]) -> usize {
    groups
        .iter()
        .map(|group| {
            group
                .get("hooks")
                .and_then(Value::as_array)
                .map_or(0, Vec::len)
        })
        .sum()
}

/// amplihack's command for `hooks_bin`, quoted as the Codex plugin quotes its
/// wrapper (`codex_plugin/hooks.rs`): single quotes, with `'` written `'\''`.
fn amplihack_command(hooks_bin: &Path) -> String {
    let quoted = format!("'{}'", hooks_bin.to_string_lossy().replace('\'', "'\\''"));
    format!("env {PROTOCOL} {quoted} {SUBCOMMAND}")
}

/// The generated file: amplihack's group first, then `user_groups`.
/// Pretty-printed, with a trailing newline.
pub(super) fn render(user_groups: &[Value], hooks_bin: &Path) -> String {
    let mut groups = Vec::with_capacity(user_groups.len() + 1);
    groups.push(json!({"hooks": [{"type": "command", "command": amplihack_command(hooks_bin)}]}));
    groups.extend(user_groups.iter().cloned());
    let document = json!({"hooks": {"PreToolUse": groups}});
    let mut text = serde_json::to_string_pretty(&document)
        .expect("a serde_json::Value with string keys always serialises");
    text.push('\n');
    text
}

/// SF3: checks a generated file before a nested launch reuses it, and
/// returns the user groups after amplihack's. amplihack's group must be the
/// first PreToolUse group and exactly one command handler (no matcher, no
/// other key) that splits to `[env, AMPLIHACK_HOOK_PROTOCOL=codex, <absolute
/// executable regular file>, pre-tool-use]`.
pub(super) fn verify_generated(text: &str) -> Result<Vec<Value>> {
    let value: Value = serde_json::from_str(text).map_err(|error| {
        anyhow!(
            "hooks.json is not valid JSON (line {}, column {})",
            error.line(),
            error.column()
        )
    })?;
    let groups = value
        .get("hooks")
        .and_then(|hooks| hooks.get("PreToolUse"))
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("hooks.json has no PreToolUse list"))?;
    let (first, rest) = groups
        .split_first()
        .ok_or_else(|| anyhow!("hooks.json has no amplihack PreToolUse entry"))?;
    verify_amplihack_group(first)?;
    Ok(rest.to_vec())
}

fn verify_amplihack_group(group: &Value) -> Result<()> {
    let not_amplihack =
        || anyhow!("the first PreToolUse group in hooks.json is not amplihack's entry");
    let group = group.as_object().ok_or_else(not_amplihack)?;
    if group.len() != 1 {
        bail!("the first PreToolUse group in hooks.json has keys besides `hooks`");
    }
    let handlers = group
        .get("hooks")
        .and_then(Value::as_array)
        .ok_or_else(not_amplihack)?;
    let [handler] = handlers.as_slice() else {
        bail!("the first PreToolUse group in hooks.json does not hold exactly one handler");
    };
    let handler = handler.as_object().ok_or_else(not_amplihack)?;
    if handler.len() != 2 || handler.get("type").and_then(Value::as_str) != Some("command") {
        return Err(not_amplihack());
    }
    let command = handler
        .get("command")
        .and_then(Value::as_str)
        .ok_or_else(not_amplihack)?;
    let words = shell_words::split(command).map_err(|_| not_amplihack())?;
    let [env, protocol, binary, subcommand] = words.as_slice() else {
        return Err(not_amplihack());
    };
    if env != "env" || protocol != PROTOCOL || subcommand != SUBCOMMAND {
        return Err(not_amplihack());
    }
    let binary = Path::new(binary);
    if !binary.is_absolute() {
        bail!("the hooks binary in amplihack's PreToolUse entry is not an absolute path");
    }
    let usable = std::fs::metadata(binary)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0);
    if !usable {
        bail!(
            "the hooks binary in amplihack's PreToolUse entry, {}, is not an executable file",
            sanitize_path(binary)
        );
    }
    Ok(())
}
