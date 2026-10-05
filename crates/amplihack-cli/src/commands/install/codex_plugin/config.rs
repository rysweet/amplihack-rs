//! Exact anticipated native configuration writes; unrelated bytes stay guarded.
use super::*;
use toml_edit::{DocumentMut, Item, Table, value};

/// Native marketplace/add/remove use TOML-preserving edits. Predict their bytes
/// before mutation rather than adopting arbitrary configuration observed later.
pub(super) fn config_states(
    original: Option<&[u8]>,
    market: &Path,
    owned: bool,
) -> Result<Vec<Vec<u8>>> {
    let text = std::str::from_utf8(original.unwrap_or_default())?;
    let mut doc = text.parse::<DocumentMut>()?;
    let path = market
        .to_str()
        .context("Codex marketplace path must be UTF-8")?;
    if let Some(entry) = doc
        .get("marketplaces")
        .and_then(|t| t.get("amplihack-local"))
    {
        ensure!(
            entry.get("source_type").and_then(Item::as_str) == Some("local")
                && entry.get("source").and_then(Item::as_str) == Some(path),
            "foreign Codex marketplace configuration; refusing replacement"
        );
    }
    if doc.get("plugins").and_then(|t| t.get(ID)).is_some() {
        ensure!(
            owned,
            "unowned Codex plugin configuration; refusing replacement"
        );
    }
    for key in ["plugins", "marketplaces"] {
        ensure!(
            doc.get(key).is_none_or(Item::is_table),
            "Codex {key} must be a table"
        );
    }
    if let Some(entry) = doc.get("plugins").and_then(|t| t.get(ID)) {
        ensure!(
            entry
                .as_table()
                .is_some_and(|t| t.iter().all(|(key, _)| key == "enabled")),
            "foreign Codex plugin settings; preserve and reconcile manually"
        );
    }
    let mut states = Vec::new();
    // Removal during rollback may leave the marketplace but remove the plugin.
    if let Some(plugins) = doc.get_mut("plugins").and_then(Item::as_table_mut) {
        plugins.remove(ID);
        states.push(doc.to_string().into_bytes());
    }
    let mut doc = text.parse::<DocumentMut>()?;
    if doc.get("marketplaces").is_none() {
        let mut table = Table::new();
        table.set_implicit(true);
        doc["marketplaces"] = Item::Table(table);
    }
    if doc["marketplaces"].get("amplihack-local").is_none() {
        doc["marketplaces"]["amplihack-local"] = Item::Table(Table::new());
    }
    doc["marketplaces"]["amplihack-local"]["source_type"] = value("local");
    doc["marketplaces"]["amplihack-local"]["source"] = value(path);
    states.push(doc.to_string().into_bytes());
    if doc.get("plugins").is_none() {
        let mut table = Table::new();
        table.set_implicit(true);
        doc["plugins"] = Item::Table(table);
    }
    if doc["plugins"].get(ID).is_none() {
        doc["plugins"][ID] = Item::Table(Table::new());
    }
    doc["plugins"][ID]["enabled"] = value(true);
    states.push(doc.to_string().into_bytes());
    doc["plugins"]
        .as_table_mut()
        .context("plugins must be a table")?
        .remove(ID);
    states.push(doc.to_string().into_bytes());
    Ok(states)
}

pub(super) fn config_matches(pending: &Value, current: &Value) -> bool {
    current == &pending["config"]
        || current == &pending["expected"]["config"]
        || pending["config_states"]
            .as_array()
            .is_some_and(|states| states.contains(current))
}
