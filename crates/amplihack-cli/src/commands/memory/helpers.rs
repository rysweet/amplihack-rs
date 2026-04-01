use anyhow::{Context, Result};
use serde_json::Value as JsonValue;
use std::fs;
use std::path::Path;

use super::types::BackendChoice;

fn parent_dir(path: &Path) -> Option<&Path> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
}

pub(crate) fn required_parent_dir(path: &Path) -> Result<&Path> {
    parent_dir(path).with_context(|| format!("path {} has no parent directory", path.display()))
}

pub(crate) fn ensure_parent_dir(path: &Path) -> Result<()> {
    let Some(parent) = parent_dir(path) else {
        return Ok(());
    };
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "failed to create parent directory {} for {}",
            parent.display(),
            path.display()
        )
    })?;
    Ok(())
}

pub(crate) fn parse_json_value(value: &str) -> Result<JsonValue> {
    if value.is_empty() {
        return Ok(JsonValue::Object(Default::default()));
    }
    Ok(serde_json::from_str(value)?)
}

pub(crate) fn parse_backend_choice_env_value(value: &str) -> Result<BackendChoice> {
    match value {
        "sqlite" => Ok(BackendChoice::Sqlite),
        "graph-db" | "kuzu" => Ok(BackendChoice::GraphDb),
        other => Err(anyhow::anyhow!(
            "Unrecognized AMPLIHACK_MEMORY_BACKEND value {:?}. \
             Valid values: sqlite, graph-db. Legacy compatibility value: kuzu",
            other
        )),
    }
}
