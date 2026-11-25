//! Persistent extensions resolved from the complete installed index.

use std::{collections::BTreeSet, fs, path::Path};

use amplihack_utils::list_selection::EntryId;
use anyhow::{Context, Result, ensure};

use crate::user_config::{self, edit, schema};

use super::{index::Entry, resolve};

pub(super) fn change(staged: &Path, entries: &[Entry], name: &str, enable: bool) -> Result<()> {
    let id = resolve::resolve(staged, entries, name)?;
    ensure!(
        enable || !id.is_default(),
        "{id} is in the default set; disable removes only explicit extensions"
    );
    let config = user_config::load_user_only();
    ensure!(
        config.diagnostics.is_empty(),
        "correct configuration problems with `amplihack config edit` before changing lists"
    );
    crate::commands::install::lists::ensure_current_installation()?;
    let path = config
        .user_file
        .context("cannot find the user configuration file")?;
    let selection = user_config::list_selection();
    let mut enabled = selection.enabled().clone();
    let changed = if enable && !id.is_default() {
        enabled.insert(id)
    } else if enable {
        false
    } else {
        enabled.remove(&id)
    };
    if changed {
        write_enabled(&path, &enabled)?;
    }
    // Always reapply, including when the desired configuration is unchanged:
    // a previous invocation may have published only some of the tools.
    crate::commands::install::lists::apply()
}

fn write_enabled(path: &Path, enabled: &BTreeSet<EntryId>) -> Result<()> {
    if !enabled.is_empty() {
        return Ok(edit::set_value(
            path,
            schema::known_key("lists.enabled")?,
            &schema::Value::List(enabled.iter().map(ToString::to_string).collect()),
        )?);
    }
    let mut doc = fs::read_to_string(path)?.parse::<toml_edit::DocumentMut>()?;
    if let Some(table) = doc
        .get_mut("lists")
        .and_then(toml_edit::Item::as_table_like_mut)
    {
        table.remove("enabled");
        if table.is_empty() {
            doc.remove("lists");
        }
    }
    Ok(edit::write_atomic(path, doc.to_string().as_bytes())?)
}
