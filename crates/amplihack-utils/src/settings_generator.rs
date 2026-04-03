//! Settings generation and merging for plugins.
//!
//! Ported from `amplihack/settings_generator/generator.py`.
//!
//! Provides [`SettingsGenerator`] which:
//! - Creates settings from plugin manifests
//! - Performs deep merging of settings dictionaries
//! - Writes formatted JSON settings files
//! - Validates plugin names, URLs, and detects circular references

use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};
use thiserror::Error;

/// Errors that can occur during settings generation.
#[derive(Debug, Error)]
pub enum SettingsError {
    /// Plugin name failed validation.
    #[error(
        "invalid plugin name: {name} \
         (must be lowercase letters, numbers, hyphens only)"
    )]
    InvalidPluginName {
        /// The invalid name.
        name: String,
    },

    /// A marketplace URL failed validation.
    #[error("invalid marketplace URL: {url}")]
    InvalidMarketplaceUrl {
        /// The rejected URL.
        url: String,
    },

    /// A marketplace name failed validation.
    #[error("invalid marketplace name: {name}")]
    InvalidMarketplaceName {
        /// The rejected name.
        name: String,
    },

    /// GitHub URL structure is invalid.
    #[error("invalid GitHub URL structure: {url}")]
    InvalidGithubUrl {
        /// The rejected URL.
        url: String,
    },

    /// Only GitHub marketplaces are currently supported.
    #[error("only GitHub marketplaces are currently supported")]
    UnsupportedMarketplace,

    /// Circular reference detected in the manifest.
    #[error("circular reference detected in manifest")]
    CircularReference,

    /// I/O error writing the settings file.
    #[error("failed to write settings to {path}: {source}")]
    WriteError {
        /// Target file path.
        path: String,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// JSON serialization error.
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Pattern for valid plugin/marketplace names (lowercase, digits, hyphens).
static NAME_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9-]+$").expect("NAME_PATTERN is valid"));

/// Generates and merges settings for plugins.
///
/// # Example
///
/// ```
/// use serde_json::json;
/// use amplihack_utils::settings_generator::SettingsGenerator;
///
/// let sg = SettingsGenerator::new();
/// let manifest = json!({
///     "name": "my-plugin",
///     "version": "1.0.0",
///     "description": "A cool plugin"
/// });
/// let settings = sg.generate(&manifest, None).unwrap();
/// assert!(settings.get("plugins").is_some());
/// ```
pub struct SettingsGenerator;

impl SettingsGenerator {
    /// Create a new `SettingsGenerator`.
    pub fn new() -> Self {
        Self
    }

    /// Generate settings from a plugin manifest.
    ///
    /// # Errors
    ///
    /// Returns [`SettingsError`] on invalid names, URLs, circular references,
    /// or unsupported marketplace types.
    pub fn generate(
        &self,
        plugin_manifest: &Value,
        user_settings: Option<&Value>,
    ) -> Result<Value, SettingsError> {
        let manifest = plugin_manifest
            .as_object()
            .cloned()
            .unwrap_or_default();

        // Check for circular references.
        check_circular_reference(plugin_manifest, &mut HashSet::new())?;

        // Validate plugin name if present.
        if let Some(name_val) = manifest.get("name")
            && let Some(name) = name_val.as_str()
            && !NAME_PATTERN.is_match(name)
        {
            return Err(SettingsError::InvalidPluginName {
                name: name.to_owned(),
            });
        }

        let mut settings = Map::new();

        // MCP servers.
        if let Some(servers) = manifest.get("mcpServers")
            && let Some(obj) = servers.as_object()
        {
            settings.insert(
                "mcpServers".to_owned(),
                Value::Object(resolve_paths_in_map(obj)),
            );
        }

        // Enabled plugins array.
        if let Some(name_val) = manifest.get("name")
            && let Some(name) = name_val.as_str()
            && !name.is_empty()
        {
            settings.insert(
                "enabledPlugins".to_owned(),
                Value::Array(vec![Value::String(name.to_owned())]),
            );
        }

        // Marketplace configuration.
        if let Some(mp_val) = manifest.get("marketplace")
            && let Some(mp) = mp_val.as_object()
        {
            self.process_marketplace(mp, &mut settings)?;
        }

        // Plugin metadata.
        if !manifest.is_empty() {
            let plugin_name = manifest
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("unknown");

            let mut plugin_meta = Map::new();
            if let Some(v) = manifest.get("version") {
                plugin_meta.insert("version".to_owned(), v.clone());
            }
            if let Some(d) = manifest.get("description") {
                plugin_meta.insert("description".to_owned(), d.clone());
            }

            let plugins = settings
                .entry("plugins")
                .or_insert_with(|| Value::Object(Map::new()));
            if let Some(obj) = plugins.as_object_mut() {
                obj.insert(
                    plugin_name.to_owned(),
                    Value::Object(plugin_meta),
                );
            }
        }

        let mut result = Value::Object(settings);

        // Merge with user settings if provided.
        if let Some(user) = user_settings {
            result = self.merge_settings(&result, user);
        }

        Ok(result)
    }

    /// Deep merge two settings values.
    ///
    /// - Dict + Dict → recursive merge
    /// - Array + Array → concatenate
    /// - Otherwise overlay takes precedence
    pub fn merge_settings(&self, base: &Value, overlay: &Value) -> Value {
        match (base, overlay) {
            (Value::Object(b), Value::Object(o)) => {
                let mut merged = b.clone();
                for (key, oval) in o {
                    let new_val = if let Some(bval) = merged.get(key) {
                        self.merge_settings(bval, oval)
                    } else {
                        oval.clone()
                    };
                    merged.insert(key.clone(), new_val);
                }
                Value::Object(merged)
            }
            (Value::Array(b), Value::Array(o)) => {
                let mut combined = b.clone();
                combined.extend(o.iter().cloned());
                Value::Array(combined)
            }
            (_, overlay) => overlay.clone(),
        }
    }

    /// Write settings to a JSON file.
    ///
    /// Creates parent directories as needed. Returns `true` on success,
    /// `false` on I/O or serialization errors.
    pub fn write_settings(
        &self,
        settings: &Value,
        target_path: &Path,
    ) -> bool {
        let result = (|| -> Result<(), SettingsError> {
            if let Some(parent) = target_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    SettingsError::WriteError {
                        path: target_path.display().to_string(),
                        source: e,
                    }
                })?;
            }
            let json_content =
                serde_json::to_string_pretty(settings)?;
            std::fs::write(target_path, json_content).map_err(|e| {
                SettingsError::WriteError {
                    path: target_path.display().to_string(),
                    source: e,
                }
            })?;
            Ok(())
        })();

        result.is_ok()
    }

    /// Process marketplace configuration and add to settings.
    fn process_marketplace(
        &self,
        mp: &Map<String, Value>,
        settings: &mut Map<String, Value>,
    ) -> Result<(), SettingsError> {
        let url = mp
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("");

        if url.is_empty() || !is_valid_url(url) {
            return Err(SettingsError::InvalidMarketplaceUrl {
                url: url.to_owned(),
            });
        }

        let name = mp
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("");

        if name.is_empty() || !NAME_PATTERN.is_match(name) {
            return Err(SettingsError::InvalidMarketplaceName {
                name: name.to_owned(),
            });
        }

        let mp_type = mp
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("github");

        if mp_type != "github" {
            return Err(SettingsError::UnsupportedMarketplace);
        }

        if !is_valid_github_url(url) {
            return Err(SettingsError::InvalidGithubUrl {
                url: url.to_owned(),
            });
        }

        let repo = url
            .split("github.com/")
            .nth(1)
            .ok_or_else(|| SettingsError::InvalidGithubUrl {
                url: url.to_owned(),
            })?
            .trim_end_matches(".git")
            .trim_end_matches('/');

        let extra = settings
            .entry("extraKnownMarketplaces")
            .or_insert_with(|| Value::Object(Map::new()));

        if let Some(obj) = extra.as_object_mut()
            && !obj.contains_key(name)
        {
            let mut source_inner = Map::new();
            source_inner.insert(
                "source".to_owned(),
                Value::String("github".to_owned()),
            );
            source_inner.insert(
                "repo".to_owned(),
                Value::String(repo.to_owned()),
            );
            let mut entry = Map::new();
            entry.insert(
                "source".to_owned(),
                Value::Object(source_inner),
            );
            obj.insert(name.to_owned(), Value::Object(entry));
        }

        Ok(())
    }
}

impl Default for SettingsGenerator {
    fn default() -> Self {
        Self::new()
    }
}

// ── helpers ──────────────────────────────────────────────────────────────

/// Check for circular references in a JSON value tree.
fn check_circular_reference(
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
fn resolve_paths_in_map(data: &Map<String, Value>) -> Map<String, Value> {
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
            (_, Value::Object(nested)) => {
                Value::Object(resolve_paths_in_map(nested))
            }
            _ => value.clone(),
        };
        resolved.insert(key.clone(), new_val);
    }
    resolved
}

/// Simple URL validation — must start with `http://` or `https://`.
fn is_valid_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Validate GitHub URL structure — must contain `github.com` with ≥3 slashes.
fn is_valid_github_url(url: &str) -> bool {
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

#[cfg(test)]
#[path = "tests/settings_generator_tests.rs"]
mod tests;
