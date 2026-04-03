//! Plugin manifest types and validation.
//!
//! Ported from `amplihack/plugin_manager/manager.py` (validation subset).
//!
//! Provides the [`PluginManifest`] type (deserialized from `plugin.json`)
//! and [`validate_manifest`] for structural + semantic checks.

use std::path::Path;

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors produced by manifest operations.
#[derive(Debug, Error)]
pub enum ManifestError {
    /// An I/O error occurred reading the manifest file.
    #[error("manifest I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The manifest file contains invalid JSON.
    #[error("invalid JSON in manifest: {0}")]
    Json(#[from] serde_json::Error),
}

// ---------------------------------------------------------------------------
// Regex patterns
// ---------------------------------------------------------------------------

/// Semantic version pattern: `major.minor.patch`.
static VERSION_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d+\.\d+\.\d+$").expect("VERSION_PATTERN regex is valid"));

/// Plugin name pattern: lowercase letters, digits, hyphens.
static NAME_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9-]+$").expect("NAME_PATTERN regex is valid"));

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Required fields in a plugin manifest.
const REQUIRED_FIELDS: &[&str] = &["name", "version", "entry_point"];

/// Recommended (but optional) fields — triggers warnings when absent.
const RECOMMENDED_FIELDS: &[&str] = &["description", "author"];

/// Fields whose values should be resolved to absolute paths.
pub const PATH_FIELDS: &[&str] = &["entry_point", "files", "cwd", "script", "path"];

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A deserialized `plugin.json` manifest.
///
/// Uses a loose schema so validation can produce detailed error messages
/// rather than opaque deserialization failures.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Plugin name (lowercase, digits, hyphens).
    #[serde(default)]
    pub name: Option<String>,
    /// Plugin version (semver `major.minor.patch`).
    #[serde(default)]
    pub version: Option<String>,
    /// Entry point file relative to the plugin root.
    #[serde(default)]
    pub entry_point: Option<String>,
    /// Human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// Plugin author.
    #[serde(default)]
    pub author: Option<String>,
    /// Additional files referenced by the plugin.
    #[serde(default)]
    pub files: Option<Vec<String>>,
    /// Working directory override.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Script path.
    #[serde(default)]
    pub script: Option<String>,
    /// Generic path field.
    #[serde(default)]
    pub path: Option<String>,
    /// All other fields captured as a map.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Result of manifest validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationResult {
    /// Whether the manifest passed all required checks.
    pub valid: bool,
    /// Hard errors that prevent installation.
    pub errors: Vec<String>,
    /// Non-fatal warnings (e.g. missing recommended fields).
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Load and deserialize a manifest file.
pub fn load_manifest(manifest_path: &Path) -> Result<PluginManifest, ManifestError> {
    let text = std::fs::read_to_string(manifest_path)?;
    let manifest: PluginManifest = serde_json::from_str(&text)?;
    Ok(manifest)
}

/// Validate a plugin manifest file on disk.
///
/// # Example
///
/// ```no_run
/// use amplihack_utils::plugin_manifest::validate_manifest;
/// use std::path::Path;
///
/// let result = validate_manifest(Path::new("/path/to/plugin.json"));
/// if !result.valid {
///     for err in &result.errors {
///         eprintln!("  error: {err}");
///     }
/// }
/// ```
pub fn validate_manifest(manifest_path: &Path) -> ValidationResult {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    if !manifest_path.exists() {
        errors.push(format!("Manifest file not found: {}", manifest_path.display()));
        return ValidationResult { valid: false, errors, warnings };
    }

    let text = match std::fs::read_to_string(manifest_path) {
        Ok(t) => t,
        Err(e) => {
            errors.push(format!("Failed to read manifest: {e}"));
            return ValidationResult { valid: false, errors, warnings };
        }
    };

    let raw: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            errors.push(format!("Invalid JSON in manifest: {e}"));
            return ValidationResult { valid: false, errors, warnings };
        }
    };

    let obj = match raw.as_object() {
        Some(o) => o,
        None => {
            errors.push("Manifest must be a JSON object".to_string());
            return ValidationResult { valid: false, errors, warnings };
        }
    };

    for &field in REQUIRED_FIELDS {
        if !obj.contains_key(field) {
            errors.push(format!("Missing required field: {field}"));
        }
    }

    if let Some(ver) = obj.get("version").and_then(|v| v.as_str())
        && !VERSION_PATTERN.is_match(ver)
    {
        errors.push(format!(
            "Invalid version format: {ver} (expected semver like 1.0.0)"
        ));
    }

    if let Some(name) = obj.get("name").and_then(|v| v.as_str())
        && !NAME_PATTERN.is_match(name)
    {
        errors.push(format!(
            "Invalid name format: {name} (must be lowercase letters, numbers, hyphens only)"
        ));
    }

    for &field in RECOMMENDED_FIELDS {
        if !obj.contains_key(field) {
            warnings.push(format!("Missing recommended field: {field}"));
        }
    }

    ValidationResult { valid: errors.is_empty(), errors, warnings }
}

/// Check whether a plugin name matches the required pattern.
pub fn is_valid_plugin_name(name: &str) -> bool {
    NAME_PATTERN.is_match(name)
}

/// Check whether a version string matches semver `major.minor.patch`.
pub fn is_valid_version(version: &str) -> bool {
    VERSION_PATTERN.is_match(version)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_manifest(dir: &Path, content: &str) -> std::path::PathBuf {
        let path = dir.join("plugin.json");
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn valid_manifest() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(
            tmp.path(),
            r#"{"name":"my-plugin","version":"1.0.0","entry_point":"main.py"}"#,
        );
        let result = validate_manifest(&path);
        assert!(result.valid);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn missing_required_fields() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(tmp.path(), r#"{"description":"test"}"#);
        let result = validate_manifest(&path);
        assert!(!result.valid);
        assert_eq!(result.errors.len(), 3);
    }

    #[test]
    fn invalid_version_format() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(
            tmp.path(),
            r#"{"name":"ok","version":"v1.0","entry_point":"main.py"}"#,
        );
        let result = validate_manifest(&path);
        assert!(!result.valid);
        assert!(result.errors.iter().any(|e| e.contains("Invalid version")));
    }

    #[test]
    fn invalid_name_format() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(
            tmp.path(),
            r#"{"name":"My Plugin!","version":"1.0.0","entry_point":"main.py"}"#,
        );
        let result = validate_manifest(&path);
        assert!(!result.valid);
        assert!(result.errors.iter().any(|e| e.contains("Invalid name")));
    }

    #[test]
    fn recommended_fields_produce_warnings() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(
            tmp.path(),
            r#"{"name":"ok","version":"1.0.0","entry_point":"main.py"}"#,
        );
        let result = validate_manifest(&path);
        assert!(result.valid);
        assert_eq!(result.warnings.len(), 2);
    }

    #[test]
    fn nonexistent_file() {
        let result = validate_manifest(Path::new("/nonexistent/plugin.json"));
        assert!(!result.valid);
        assert!(result.errors[0].contains("not found"));
    }

    #[test]
    fn invalid_json() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(tmp.path(), "not json {{{");
        let result = validate_manifest(&path);
        assert!(!result.valid);
        assert!(result.errors[0].contains("Invalid JSON"));
    }

    #[test]
    fn manifest_not_an_object() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(tmp.path(), r#"["array","not","object"]"#);
        let result = validate_manifest(&path);
        assert!(!result.valid);
        assert!(result.errors[0].contains("JSON object"));
    }

    #[test]
    fn name_validation_patterns() {
        assert!(is_valid_plugin_name("my-plugin"));
        assert!(is_valid_plugin_name("plugin123"));
        assert!(is_valid_plugin_name("a"));
        assert!(!is_valid_plugin_name("My Plugin"));
        assert!(!is_valid_plugin_name("plugin_underscore"));
        assert!(!is_valid_plugin_name(""));
    }

    #[test]
    fn version_validation_patterns() {
        assert!(is_valid_version("1.0.0"));
        assert!(is_valid_version("0.0.1"));
        assert!(is_valid_version("99.99.99"));
        assert!(!is_valid_version("v1.0.0"));
        assert!(!is_valid_version("1.0"));
        assert!(!is_valid_version("1"));
        assert!(!is_valid_version(""));
    }

    #[test]
    fn load_manifest_round_trip() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(
            tmp.path(),
            r#"{"name":"test","version":"2.0.0","entry_point":"run.sh","description":"A test","author":"dev"}"#,
        );
        let m = load_manifest(&path).unwrap();
        assert_eq!(m.name.as_deref(), Some("test"));
        assert_eq!(m.version.as_deref(), Some("2.0.0"));
        assert_eq!(m.entry_point.as_deref(), Some("run.sh"));
        assert_eq!(m.description.as_deref(), Some("A test"));
        assert_eq!(m.author.as_deref(), Some("dev"));
    }

    #[test]
    fn full_manifest_with_extra_fields() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(
            tmp.path(),
            r#"{"name":"x","version":"1.0.0","entry_point":"e.py","extra_field":true}"#,
        );
        let result = validate_manifest(&path);
        assert!(result.valid);
        let m = load_manifest(&path).unwrap();
        assert!(m.extra.contains_key("extra_field"));
    }
}
