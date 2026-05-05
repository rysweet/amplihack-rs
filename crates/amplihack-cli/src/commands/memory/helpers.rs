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

/// Parse a raw string value into a [`BackendChoice`].
///
/// # Recognised values
///
/// | Input        | Result                   |
/// |--------------|--------------------------|
/// | `"sqlite"`   | `BackendChoice::Sqlite`  |
/// | `"graph-db"` | `BackendChoice::GraphDb` |
/// | `"kuzu"`     | `BackendChoice::GraphDb` *(legacy alias)* |
///
/// Any other value produces an error whose message lists the valid options.
/// A `tracing::warn!` is also emitted so the invalid value is visible in logs
/// even when the caller swallows the error (e.g. by defaulting).
pub(crate) fn parse_backend_choice_env_value(value: &str) -> Result<BackendChoice> {
    match value {
        "sqlite" => Ok(BackendChoice::Sqlite),
        "graph-db" | "kuzu" => Ok(BackendChoice::GraphDb),
        other => {
            let msg = format!(
                "Unrecognized AMPLIHACK_MEMORY_BACKEND value {other:?}. \
                 Valid values: sqlite, graph-db. Legacy compatibility value: kuzu",
            );
            tracing::warn!("{msg}");
            Err(anyhow::anyhow!(msg))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_backend_choice_env_value_error_lists_valid_values() {
        let err = parse_backend_choice_env_value("not-a-backend")
            .expect_err("unrecognised value should error");
        let msg = err.to_string();
        assert!(
            msg.contains("Valid values: sqlite, graph-db"),
            "error should list valid values, got: {msg}",
        );
        assert!(
            msg.contains("not-a-backend"),
            "error should echo the rejected value, got: {msg}",
        );
    }

    #[test]
    fn parse_backend_choice_env_value_accepts_known_values() {
        assert_eq!(
            parse_backend_choice_env_value("sqlite").unwrap(),
            BackendChoice::Sqlite,
        );
        assert_eq!(
            parse_backend_choice_env_value("graph-db").unwrap(),
            BackendChoice::GraphDb,
        );
        assert_eq!(
            parse_backend_choice_env_value("kuzu").unwrap(),
            BackendChoice::GraphDb,
        );
    }
}
