//! The generated `hooks.json` of a local-model Codex home.
//!
//! The file holds `PreToolUse` and nothing else: amplihack's own
//! pre-tool-use entry first, then the user's PreToolUse groups from their
//! `hooks.json`, in order and otherwise unchanged, minus the handlers that
//! run amplihack's pre-tool-use hook exactly as amplihack's own entry does
//! (the Codex plugin's wrapper, or a program named `amplihack-hooks` that
//! resolves to the same file as the hooks binary amplihack's entry runs,
//! bare or after `env AMPLIHACK_HOOK_PROTOCOL=codex`). A program of that name
//! at another file, or one that cannot be resolved, is the user's and is kept. A handler that sets any
//! other variable, such as `XPIA_ENABLED=true`, may check more than
//! amplihack's entry does, so it is kept. A handler the filter keeps runs
//! the guard twice, which is safe; the filter therefore removes only exact
//! word lists, and only when every word is one the shell would take as it is
//! (when in doubt, keep it).
//!
//! Pure apart from resolving a program named `amplihack-hooks` in the user's
//! file, and [`verify_generated`], which checks that the hooks binary named in
//! amplihack's entry is still an executable regular file, and the one the
//! launch itself resolves. No error
//! quotes the file's content: a parse error gives the path, line and column.

use crate::commands::install::paths::sanitize_path;
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path};

const SUBCOMMAND: &str = "pre-tool-use";
const PROTOCOL: &str = "AMPLIHACK_HOOK_PROTOCOL=codex";
pub(super) const HOOKS_BINARY: &str = "amplihack-hooks";
/// Characters a shell may treat as an operator, a quote, an expansion, a
/// glob or a comment. shell-words removes quoting but does not evaluate
/// operators or expansions, so these characters require a conservative check.
const SHELL_SPECIAL: [char; 21] = [
    ';', '&', '|', '<', '>', '(', ')', '$', '`', '\\', '"', '\'', '*', '?', '[', ']', '{', '}',
    '~', '!', '#',
];

/// The user's PreToolUse groups from `text` (their `hooks.json` at `path`),
/// with amplihack's own pre-tool-use handlers removed and groups left empty
/// by that dropped. `plugin_root` is `<home>/.amplihack/codex`; `hooks_bin`
/// is the hooks binary amplihack's own entry runs.
pub(super) fn user_pre_tool_use(
    text: &str,
    path: &Path,
    plugin_root: &Path,
    hooks_bin: &Path,
) -> Result<Vec<Value>> {
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
        .filter_map(|group| without_amplihack(group, plugin_root, hooks_bin))
        .collect())
}

/// `group` without its amplihack pre-tool-use handlers; `None` when removing
/// them left it with none. A group or handler of an unexpected shape is kept
/// as it is.
fn without_amplihack(group: &Value, plugin_root: &Path, hooks_bin: &Path) -> Option<Value> {
    let Some(handlers) = group.get("hooks").and_then(Value::as_array) else {
        return Some(group.clone());
    };
    let kept: Vec<Value> = handlers
        .iter()
        .filter(|handler| !is_amplihack_handler(handler, plugin_root, hooks_bin))
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

/// A `type: command` handler whose command runs amplihack's pre-tool-use:
/// the plugin wrapper, or the same hooks binary file as `hooks_bin`.
fn is_amplihack_handler(handler: &Value, plugin_root: &Path, hooks_bin: &Path) -> bool {
    handler.get("type").and_then(Value::as_str) == Some("command")
        && handler
            .get("command")
            .and_then(Value::as_str)
            .and_then(|command| amplihack_program(command, plugin_root))
            .is_some_and(|program| match program {
                Program::PluginWrapper => true,
                Program::HooksBinary(name) => same_file(&name, hooks_bin),
            })
}

/// What [`amplihack_program`] recognised.
#[derive(Debug, PartialEq, Eq)]
enum Program {
    /// The Codex plugin's wrapper under `plugin_root`.
    PluginWrapper,
    /// A program named `amplihack-hooks`, as the command gives it.
    HooksBinary(String),
}

/// Whether `program` (bare, resolved through `PATH`, or a path) is the same
/// file as `hooks_bin`. A program that cannot be resolved is not.
fn same_file(program: &str, hooks_bin: &Path) -> bool {
    let path = Path::new(program);
    let found = if path.components().count() == 1 {
        crate::commands::install::paths::find_binary(program)
    } else if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        None
    };
    match (
        found.and_then(|found| std::fs::canonicalize(found).ok()),
        std::fs::canonicalize(hooks_bin),
    ) {
        (Some(found), Ok(ours)) => found == ours,
        _ => false,
    }
}

/// Whether `command`, split into shell words, is exactly
/// `[program, pre-tool-use]`, optionally after `env` or
/// `env AMPLIHACK_HOOK_PROTOCOL=codex`, where `program` is named
/// `amplihack-hooks` or is an absolute path under `plugin_root` ending in
/// `bin/hook`. Any other assignment keeps the handler: it changes what the
/// hook checks (`XPIA_ENABLED=true` turns on the XPIA check), and removing
/// it would weaken the user's guard without a word. A command that cannot
/// be split, is compound, or has a word that is not [inert](is_inert_word),
/// is not amplihack's either. This is the shape only: the filter also
/// requires a program named `amplihack-hooks` to be the hooks binary file.
#[cfg(test)]
pub(super) fn runs_amplihack_pre_tool_use(command: &str, plugin_root: &Path) -> bool {
    amplihack_program(command, plugin_root).is_some()
}

/// The program of a command of the shape [`runs_amplihack_pre_tool_use`]
/// accepts; `None` for any other command.
fn amplihack_program(command: &str, plugin_root: &Path) -> Option<Program> {
    // shell-words treats raw newlines as word separators and loses shell
    // command boundaries. Keep these handlers before parsing can erase them.
    if command.contains('\n') {
        return None;
    }
    let words = shell_words::split(command).ok()?;
    if !words.iter().all(|word| is_inert_word(word)) {
        return None;
    }
    let rest = match words.as_slice() {
        [env, protocol, rest @ ..] if env == "env" && protocol == PROTOCOL => rest,
        [env, rest @ ..] if env == "env" => rest,
        rest => rest,
    };
    match rest {
        [program, subcommand] if subcommand == SUBCOMMAND => classify_program(program, plugin_root),
        _ => None,
    }
}

/// Whether the shell would take `word` as it is: no whitespace, no control
/// character and nothing from [`SHELL_SPECIAL`]. shell-words removes the
/// quoting it understands, so a word that was quoted in the command is
/// refused too when it holds one of these; such a handler is kept.
fn is_inert_word(word: &str) -> bool {
    !word
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || SHELL_SPECIAL.contains(&c))
}

fn classify_program(program: &str, plugin_root: &Path) -> Option<Program> {
    let path = Path::new(program);
    if path.file_name().and_then(|name| name.to_str()) == Some(HOOKS_BINARY) {
        return Some(Program::HooksBinary(program.to_owned()));
    }
    (path.is_absolute()
        && !path.components().any(|c| c == Component::ParentDir)
        && path != plugin_root
        && path.starts_with(plugin_root)
        && path.ends_with("bin/hook"))
    .then_some(Program::PluginWrapper)
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

/// Codex 0.166.0's trust hash for our single synchronous PreToolUse handler.
/// Codex hashes the sorted JSON form of its normalised TOML identity:
/// absent optional fields are omitted and the default command duration is
/// 600 seconds. Only our generated definition is trusted, never user groups.
pub(super) fn command_check_hash(hooks_bin: &Path) -> String {
    command_hash(&amplihack_command(hooks_bin))
}

/// Hash the actual command bytes: shell-equivalent whitespace or quoting
/// changes still invalidate Codex's saved trust.
fn command_hash(command: &str) -> String {
    let mut identity = json!({
        "event_name": "pre_tool_use",
        "hooks": [{
            "type": "command",
            "command": command,
            "async": false,
            "timeout": 600,
        }],
    });
    identity.sort_all_objects();
    let bytes = serde_json::to_vec(&identity)
        .expect("a serde_json::Value with string keys always serialises");
    format!("sha256:{:x}", Sha256::digest(bytes))
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

/// Checks a generated file before a nested launch reuses it, and returns
/// the user groups after amplihack's and the actual handler's trust hash.
/// amplihack's group must be the first
/// PreToolUse group and exactly one command handler (no matcher, no other
/// key) that splits to `[env, AMPLIHACK_HOOK_PROTOCOL=codex, <hooks binary>,
/// pre-tool-use]`, where the hooks binary is an absolute path of an
/// executable regular file named `amplihack-hooks` that resolves to the same
/// file as `expected`, the binary this launch would put there. A session
/// that pointed the entry at another file is refused.
pub(super) fn verify_generated(text: &str, expected: &Path) -> Result<(Vec<Value>, String)> {
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
    let hash = verify_amplihack_group(first, expected)?;
    Ok((rest.to_vec(), hash))
}

fn verify_amplihack_group(group: &Value, expected: &Path) -> Result<String> {
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
    if binary.file_name().and_then(|name| name.to_str()) != Some(HOOKS_BINARY) {
        bail!(
            "the hooks binary in amplihack's PreToolUse entry, {}, is not named {HOOKS_BINARY}",
            sanitize_path(binary)
        );
    }
    let usable = std::fs::metadata(binary)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0);
    if !usable {
        bail!(
            "the hooks binary in amplihack's PreToolUse entry, {}, is not an executable file",
            sanitize_path(binary)
        );
    }
    let same = match (
        std::fs::canonicalize(binary),
        std::fs::canonicalize(expected),
    ) {
        (Ok(found), Ok(expected)) => found == expected,
        _ => false,
    };
    if !same {
        bail!(
            "the hooks binary in amplihack's PreToolUse entry, {}, is not the one this launch \
             uses, {}",
            sanitize_path(binary),
            sanitize_path(expected)
        );
    }
    Ok(command_hash(command))
}
