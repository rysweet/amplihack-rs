//! The generated `config.toml` of a local-model Codex home. Pure: no I/O.
//!
//! [`render`] builds the file in four steps:
//!
//! 1. copy the profile, minus `marketplaces`, `plugins`, `projects`, `hooks`,
//!    `profile` and `profiles` (the last two so that a profile cannot switch
//!    a forced setting back);
//! 2. force `project_doc_max_bytes = 0`, `model_instructions_file`,
//!    `web_search = "disabled"`, `[features]` (`hooks` on; `multi_agent`,
//!    `goals`, `view_image`, `plugins` and `apps` off) and
//!    `[skills.bundled] enabled = false`, merging into the profile's tables;
//! 3. set `approval_policy = "never"` only when the profile has none;
//! 4. carry `[projects.*]`, `[hooks.state.*]` and `[notice]` over from the
//!    previous generated file, which Codex writes itself. Hook definitions
//!    are never carried.
//!
//! The output is serialised from a value, not from the profile's layout, so
//! it depends only on the inputs: rendering again over its own output gives
//! the same bytes, and an unchanged home is never rewritten.
//!
//! Parse errors are fresh messages carrying the path and the line only. They
//! never quote or chain the parser's error, which may hold a line of the
//! source, so a key in the profile cannot reach a message (SF9).

use crate::commands::install::paths::sanitize_path;
use anyhow::{Result, anyhow, bail};
use std::path::Path;
use toml::{Table, Value};
use toml_edit::DocumentMut;

/// Top-level keys of the profile that never reach the generated file.
const REMOVED: [&str; 6] = [
    "marketplaces",
    "plugins",
    "projects",
    "hooks",
    "profile",
    "profiles",
];

/// `[features]` keys forced off: the tools and listings they add are left
/// out of the prompt.
const FEATURES_OFF: [&str; 5] = ["multi_agent", "goals", "view_image", "plugins", "apps"];

/// Parses a profile or a generated `config.toml`.
pub(super) fn parse(text: &str, path: &Path) -> Result<DocumentMut> {
    text.parse::<DocumentMut>().map_err(|error| {
        let shown = sanitize_path(path);
        match error.span() {
            Some(span) => anyhow!(
                "{shown} is not valid TOML (line {})",
                line_of(text, span.start)
            ),
            None => anyhow!("{shown} is not valid TOML"),
        }
    })
}

/// The 1-based line holding byte `offset` of `text`.
fn line_of(text: &str, offset: usize) -> usize {
    let end = offset.min(text.len());
    text.as_bytes()[..end]
        .iter()
        .filter(|b| **b == b'\n')
        .count()
        + 1
}

/// E8: `features`, `skills` and `skills.bundled` must be tables (standard,
/// inline or dotted) where the profile has them, so that amplihack's
/// settings can be merged into them.
pub(super) fn check_shape(profile: &DocumentMut, path: &Path) -> Result<()> {
    let root = profile.as_table();
    if let Some(features) = root.get("features")
        && !features.is_table_like()
    {
        bail!(shape_error(path, "features"));
    }
    if let Some(skills) = root.get("skills") {
        let Some(skills) = skills.as_table_like() else {
            bail!(shape_error(path, "skills"));
        };
        if let Some(bundled) = skills.get("bundled")
            && !bundled.is_table_like()
        {
            bail!(shape_error(path, "skills.bundled"));
        }
    }
    Ok(())
}

fn shape_error(path: &Path, key: &str) -> String {
    format!(
        "{}: `{key}` must be a table, so that amplihack can merge its settings into it",
        sanitize_path(path)
    )
}

/// The generated `config.toml` for `profile`, carrying Codex's own tables
/// over from `previous`. `instructions` is the absolute path of the home's
/// `instructions.md`.
pub(super) fn render(
    profile: &DocumentMut,
    previous: Option<&DocumentMut>,
    instructions: &Path,
) -> Result<String> {
    let instructions = instructions
        .to_str()
        .ok_or_else(|| anyhow!("{} is not valid UTF-8", sanitize_path(instructions)))?;
    let mut out = Table::new();

    // Step 1: the profile, minus what may not reach the generated home.
    for (key, value) in to_table(profile, "the local-model profile")? {
        if !REMOVED.contains(&key.as_str()) {
            out.insert(key, value);
        }
    }

    // Step 2: amplihack's settings override the profile's.
    out.insert("project_doc_max_bytes".into(), Value::Integer(0));
    out.insert(
        "model_instructions_file".into(),
        Value::String(instructions.to_string()),
    );
    out.insert("web_search".into(), Value::String("disabled".into()));
    let features = table_entry(&mut out, "features")?;
    features.insert("hooks".into(), Value::Boolean(true));
    for key in FEATURES_OFF {
        features.insert(key.into(), Value::Boolean(false));
    }
    let skills = table_entry(&mut out, "skills")?;
    let bundled = table_entry(skills, "bundled")?;
    bundled.insert("enabled".into(), Value::Boolean(false));

    // Step 3: a default, not an override (D7).
    out.entry("approval_policy")
        .or_insert_with(|| Value::String("never".into()));

    // Step 4: tables Codex wrote into the previous generated file.
    if let Some(previous) = previous {
        let previous = to_table(previous, "the previous generated config.toml")?;
        if let Some(projects) = previous.get("projects") {
            out.insert("projects".into(), projects.clone());
        }
        if let Some(state) = previous
            .get("hooks")
            .and_then(Value::as_table)
            .and_then(|hooks| hooks.get("state"))
        {
            let mut hooks = Table::new();
            hooks.insert("state".into(), state.clone());
            out.insert("hooks".into(), Value::Table(hooks));
        }
        if let Some(notice) = previous.get("notice") {
            out.insert("notice".into(), notice.clone());
        }
    }

    toml::to_string(&out).map_err(|_| anyhow!("the generated config.toml could not be serialised"))
}

/// `document` as a plain table. The document was parsed from valid TOML and
/// prints as valid TOML, so this re-reads its own output; a failure is
/// reported without the parser's message, which may quote the source.
fn to_table(document: &DocumentMut, what: &str) -> Result<Table> {
    toml::from_str::<Table>(&document.to_string())
        .map_err(|_| anyhow!("{what} could not be read back as TOML"))
}

/// The table at `key` in `table`, created when absent. An existing value
/// that is not a table is refused: `render` is a guard as well as
/// [`check_shape`].
fn table_entry<'a>(table: &'a mut Table, key: &str) -> Result<&'a mut Table> {
    table
        .entry(key)
        .or_insert_with(|| Value::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| anyhow!("`{key}` in the local-model profile is not a table"))
}
