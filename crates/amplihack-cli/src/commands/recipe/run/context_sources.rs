//! Issue #1570: recipe context read from files (`--context-file`,
//! `--context-from-file KEY=PATH`), so a large value never occupies a single
//! argv element of `amplihack recipe run`.
//!
//! Precedence, lowest to highest: `--context-file`, `--context-from-file`,
//! `-c`. Error messages name the option, path, key or argument, never the
//! file contents.

use anyhow::{Result, anyhow, bail};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

/// Largest file either option, or `--prompt-file`, reads.
pub(crate) const CONTEXT_SOURCE_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// Characters of an invalid key shown in an error message.
const SHOWN_KEY_MAX_CHARS: usize = 64;

const CONTEXT_FILE: &str = "--context-file";
const CONTEXT_FROM_FILE: &str = "--context-from-file";

/// Merge the file sources with the parsed `-c` pairs.
pub(super) fn merge_context_sources(
    context_file: Option<&Path>,
    context_from_file: &[String],
    cli_context: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    let mut merged = BTreeMap::new();
    if let Some(path) = context_file {
        merged.extend(read_context_file(path)?);
    }
    for arg in context_from_file {
        let (key, value) = read_context_from_file(arg)?;
        merged.insert(key, value);
    }
    for (key, value) in cli_context {
        merged.insert(key.clone(), value.clone());
    }
    Ok(merged)
}

/// Resolve `amplihack recipe run`'s context options into the `KEY=VALUE`
/// list `run_recipe` takes. With no file source the `-c` arguments are
/// returned unchanged, so their existing validation still applies. Errors are
/// printed and turned into exit status 1.
pub fn resolve_context_args(
    context_file: Option<&str>,
    context_from_file: &[String],
    context_args: Vec<String>,
) -> Result<Vec<String>> {
    if context_file.is_none() && context_from_file.is_empty() {
        return Ok(context_args);
    }
    let (cli_context, errors) = super::parse_context_args(&context_args);
    let merged = if errors.is_empty() {
        merge_context_sources(context_file.map(Path::new), context_from_file, &cli_context)
            .map_err(|error| vec![format!("{error:#}")])
    } else {
        Err(errors)
    };
    match merged {
        // File and `-c` keys never contain `=`, so `split_once('=')` in
        // `parse_context_args` restores each pair exactly.
        Ok(map) => Ok(map.into_iter().map(|(k, v)| format!("{k}={v}")).collect()),
        Err(errors) => {
            for error in errors {
                eprintln!("Error: {error}");
            }
            Err(crate::command_error::exit_error(1))
        }
    }
}

fn read_context_file(path: &Path) -> Result<Vec<(String, String)>> {
    let text = read_source(CONTEXT_FILE, path)?;
    let value: Value = serde_json::from_str(&text).map_err(|error| {
        // Only the position is reported: serde_json's message can quote input.
        anyhow!(
            "{CONTEXT_FILE} {}: invalid JSON at line {} column {}",
            path.display(),
            error.line(),
            error.column()
        )
    })?;
    let Value::Object(object) = value else {
        bail!(
            "{CONTEXT_FILE} {}: top-level JSON value must be an object",
            path.display()
        );
    };
    let mut pairs = Vec::with_capacity(object.len());
    for (key, value) in object {
        validate_key(CONTEXT_FILE, &key)?;
        let value = json_to_context_value(value);
        check_no_nul(CONTEXT_FILE, path, &key, &value)?;
        pairs.push((key, value));
    }
    Ok(pairs)
}

fn read_context_from_file(arg: &str) -> Result<(String, String)> {
    let Some((key, path)) = arg.split_once('=') else {
        bail!(
            "{CONTEXT_FROM_FILE}: expected KEY=PATH, got '{}'",
            arg.escape_debug()
        );
    };
    validate_key(CONTEXT_FROM_FILE, key)?;
    if path.is_empty() {
        bail!("{CONTEXT_FROM_FILE}: expected KEY=PATH, got an empty path for key '{key}'");
    }
    let path = Path::new(path);
    let value = read_source(CONTEXT_FROM_FILE, path)?;
    check_no_nul(CONTEXT_FROM_FILE, path, key, &value)?;
    Ok((key.to_string(), value))
}

/// Read at most [`CONTEXT_SOURCE_MAX_BYTES`] of UTF-8 text from `path`.
/// Errors name `option` and the path, never the contents.
pub(crate) fn read_source(option: &str, path: &Path) -> Result<String> {
    let shown = path.display();
    let file = std::fs::File::open(path)
        .map_err(|error| anyhow!("{option} {shown}: cannot open file: {error}"))?;
    let size = file
        .metadata()
        .map_err(|error| anyhow!("{option} {shown}: cannot read file: {error}"))?
        .len();
    if size > CONTEXT_SOURCE_MAX_BYTES {
        bail!(
            "{option} {shown}: file is {size} bytes, larger than the \
             {CONTEXT_SOURCE_MAX_BYTES}-byte (16 MiB) limit"
        );
    }
    let mut bytes = Vec::with_capacity(size as usize);
    // `take` guards against a file that grows after the size check.
    file.take(CONTEXT_SOURCE_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| anyhow!("{option} {shown}: cannot read file: {error}"))?;
    if bytes.len() as u64 > CONTEXT_SOURCE_MAX_BYTES {
        bail!(
            "{option} {shown}: file is larger than the \
             {CONTEXT_SOURCE_MAX_BYTES}-byte (16 MiB) limit"
        );
    }
    String::from_utf8(bytes).map_err(|error| {
        anyhow!(
            "{option} {shown}: file is not valid UTF-8 (invalid byte at offset {})",
            error.utf8_error().valid_up_to()
        )
    })
}

fn validate_key(option: &str, key: &str) -> Result<()> {
    if key.is_empty() {
        bail!("{option}: context key must not be empty");
    }
    if !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        bail!(
            "{option}: key '{}' is invalid; keys must match [A-Za-z0-9_]+",
            shown_key(key)
        );
    }
    if key.to_ascii_uppercase().starts_with("AMPLIHACK_") {
        bail!("{option}: key '{}' is reserved", shown_key(key));
    }
    Ok(())
}

/// `key` for an error message: at most [`SHOWN_KEY_MAX_CHARS`] characters,
/// escaped, with `…` when cut.
fn shown_key(key: &str) -> String {
    let mut chars = key.chars();
    let head: String = chars.by_ref().take(SHOWN_KEY_MAX_CHARS).collect();
    let mut shown = head.escape_debug().to_string();
    if chars.next().is_some() {
        shown.push('…');
    }
    shown
}

fn check_no_nul(option: &str, path: &Path, key: &str, value: &str) -> Result<()> {
    if value.contains('\0') {
        bail!(
            "{option} {}: value for key '{key}' contains a NUL byte",
            path.display()
        );
    }
    Ok(())
}

/// JSON value to context string: strings as they are, `null` as `""`, other
/// scalars as their JSON text, arrays and objects as compact JSON.
fn json_to_context_value(value: Value) -> String {
    match value {
        Value::String(s) => s,
        Value::Null => String::new(),
        other => other.to_string(),
    }
}
