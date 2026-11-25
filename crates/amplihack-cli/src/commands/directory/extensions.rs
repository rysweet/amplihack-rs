//! Preserve the original extension key when a directory change is reversed.

use std::{collections::BTreeSet, fs, path::Path};

use amplihack_utils::list_selection::EntryId;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use toml_edit::{DocumentMut, Item};

use crate::user_config::{edit, schema};

#[derive(Serialize, Deserialize)]
struct Original {
    config: Option<String>,
}

pub(super) fn write(path: &Path, enabled: &BTreeSet<EntryId>) -> Result<()> {
    let record = path.with_file_name("lists-enabled-origin.json");
    let original = match fs::read(&record) {
        Ok(bytes) => serde_json::from_slice::<Original>(&bytes)
            .context("read original list configuration")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let config = match fs::read_to_string(path) {
                Ok(text) => Some(text),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            let original = Original { config };
            edit::write_atomic(&record, &serde_json::to_vec(&original)?)?;
            original
        }
        Err(error) => return Err(error.into()),
    };
    let before = original
        .config
        .as_deref()
        .unwrap_or_default()
        .parse::<DocumentMut>()?;
    let before_value = before.get("lists").and_then(|table| table.get("enabled"));
    let before_enabled = before_value
        .and_then(Item::as_array)
        .into_iter()
        .flatten()
        .map(|value| {
            value
                .as_str()
                .context("invalid original extension")
                .and_then(str::parse)
        })
        .collect::<Result<BTreeSet<EntryId>>>()?
        .into_iter()
        .filter(|id| !id.is_default())
        .collect::<BTreeSet<_>>();
    let key = schema::known_key("lists.enabled")?;
    if &before_enabled != enabled {
        edit::set_value(
            path,
            key,
            &schema::Value::List(enabled.iter().map(ToString::to_string).collect()),
        )?;
        if enabled.is_empty() {
            fs::remove_file(record)?;
        }
        return Ok(());
    }
    let mut current = fs::read_to_string(path)?.parse::<DocumentMut>()?;
    if let Some(table) = current.get_mut("lists").and_then(Item::as_table_like_mut) {
        match before_value {
            Some(value) => {
                let key = before
                    .get("lists")
                    .and_then(Item::as_table_like)
                    .and_then(|table| table.key("enabled"))
                    .context("original extension key is missing")?;
                table.entry_format(key).or_insert(Item::None);
                *table
                    .get_mut("enabled")
                    .context("extension key is missing")? = value.clone();
                let mut current_key = table
                    .key_mut("enabled")
                    .context("extension key is missing")?;
                *current_key.leaf_decor_mut() = key.leaf_decor().clone();
                *current_key.dotted_decor_mut() = key.dotted_decor().clone();
            }
            None => {
                table.remove("enabled");
            }
        }
        if table.is_empty() && before.get("lists").is_none() {
            current.remove("lists");
        }
    }
    let restored = current.to_string();
    if original.config.is_none() && restored.is_empty() {
        fs::remove_file(path)?;
    } else {
        edit::write_atomic(path, restored.as_bytes())?;
    }
    fs::remove_file(record)?;
    Ok(())
}

/// Explicit configuration writes establish a new baseline for directory edits.
pub(crate) fn forget(path: &Path) -> Result<()> {
    match fs::remove_file(path.with_file_name("lists-enabled-origin.json")) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reversing_extensions_restores_absent_empty_and_existing_keys_byte_for_byte() {
        for original in [
            None,
            Some(""),
            Some("# keep\n[recipe]\nverbose = true\n"),
            Some("# keep\n[lists]\nset = 'all'\n\n[recipe]\nverbose = true\n"),
            Some("[lists]\n# entries\nenabled = [ ] # keep\n"),
            Some("[lists]\nenabled = ['skill:existing']\n"),
            Some("[lists]\nenabled = ['skill:pdf']\n"),
            Some("lists = { enabled = [ ] }\n"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            if let Some(text) = original {
                fs::write(&path, text).unwrap();
            }
            let baseline: BTreeSet<EntryId> =
                if original.is_some_and(|text| text.contains("skill:existing")) {
                    ["skill:existing".parse().unwrap()].into()
                } else {
                    BTreeSet::new()
                };
            let mut enabled = baseline.clone();
            enabled.insert("skill:extra".parse().unwrap());
            write(&path, &enabled).unwrap();
            write(&path, &enabled).unwrap();
            write(&path, &baseline).unwrap();
            assert_eq!(fs::read_to_string(&path).ok().as_deref(), original);
        }
    }

    #[test]
    fn unrelated_edits_between_enable_and_disable_are_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "[recipe]\nverbose = false\n").unwrap();
        write(&path, &["skill:extra".parse().unwrap()].into()).unwrap();
        edit::set_value(
            &path,
            schema::known_key("recipe.verbose").unwrap(),
            &schema::Value::Bool(true),
        )
        .unwrap();
        write(&path, &BTreeSet::new()).unwrap();
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "[recipe]\nverbose = true\n"
        );
    }
}
