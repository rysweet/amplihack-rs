//! Helper functions for settings generation.
//!
//! Extracted from `settings_generator` to keep modules under 400 lines.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};

use crate::settings_generator::SettingsError;

/// Check for circular references in a JSON value tree.
pub(crate) fn check_circular_reference(
    data: &Value,
    seen: &mut HashSet<usize>,
) -> Result<(), SettingsError> {
    match data {
        Value::Object(_) | Value::Array(_) => {
            let ptr = data as *const Value as usize;
            if !seen.insert(ptr) {
                return Err(SettingsError::CircularReference);
            }
            match data {
                Value::Object(map) => {
                    for v in map.values() {
                        check_circular_reference(v, seen)?;
                    }
                }
                Value::Array(arr) => {
                    for v in arr {
                        check_circular_reference(v, seen)?;
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
    Ok(())
}

/// Resolve relative paths (`cwd`, `path`, `script` keys) to absolute.
pub(crate) fn resolve_paths_in_map(data: &Map<String, Value>) -> Map<String, Value> {
    let mut resolved = Map::new();
    for (key, value) in data {
        let new_val = match (key.as_str(), value) {
            ("cwd" | "path" | "script", Value::String(s)) => {
                let p = std::path::Path::new(s);
                if p.is_absolute() {
                    value.clone()
                } else {
                    let abs = std::env::current_dir()
                        .unwrap_or_default()
                        .join(p);
                    Value::String(abs.to_string_lossy().into_owned())
                }
            }
            (_, Value::Object(nested)) => Value::Object(resolve_paths_in_map(nested)),
            _ => value.clone(),
        };
        resolved.insert(key.clone(), new_val);
    }
    resolved
}

/// Simple URL validation — must start with `http://` or `https://`.
pub(crate) fn is_valid_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Validate GitHub URL structure — must contain `github.com` with ≥3 slashes.
pub(crate) fn is_valid_github_url(url: &str) -> bool {
    url.contains("github.com") && url.chars().filter(|&c| c == '/').count() >= 3
}

/// Validate a semantic version string (major.minor.patch).
///
/// Accepts versions like `1.0.0`, `0.12.3`, and optional pre-release/build
/// suffixes like `1.0.0-beta.1+build.42`.
pub fn is_valid_semver(version: &str) -> bool {
    static SEMVER_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([\w][\w.]*)?)?(?:\+([\w][\w.]*))?$",
        )
        .expect("SEMVER_RE is valid")
    });
    SEMVER_RE.is_match(version)
}
