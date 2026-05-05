use super::*;
use serde_json::json;
use tempfile::TempDir;

fn make_gen() -> SettingsGenerator {
    SettingsGenerator::new()
}

// ── generate tests ──────────────────────────────────────────────────────

#[test]
fn generate_basic_manifest() {
    let manifest = json!({
        "name": "my-plugin",
        "version": "1.0.0",
        "description": "A test plugin"
    });
    let settings = make_gen().generate(&manifest, None).unwrap();
    let plugins = settings.get("plugins").unwrap();
    assert!(plugins.get("my-plugin").is_some());
    assert_eq!(plugins["my-plugin"]["version"], json!("1.0.0"));
}

#[test]
fn generate_rejects_invalid_plugin_name() {
    let manifest = json!({ "name": "INVALID_NAME!" });
    let err = make_gen().generate(&manifest, None).unwrap_err();
    assert!(
        matches!(err, SettingsError::InvalidPluginName { .. }),
        "expected InvalidPluginName, got: {err}"
    );
}

#[test]
fn generate_accepts_valid_hyphenated_name() {
    let manifest = json!({ "name": "my-cool-plugin-2" });
    assert!(make_gen().generate(&manifest, None).is_ok());
}

#[test]
fn generate_includes_enabled_plugins() {
    let manifest = json!({ "name": "test-plugin" });
    let settings = make_gen().generate(&manifest, None).unwrap();
    let enabled = settings.get("enabledPlugins").unwrap();
    assert_eq!(enabled, &json!(["test-plugin"]));
}

#[test]
fn generate_processes_marketplace() {
    let manifest = json!({
        "name": "mp-plugin",
        "marketplace": {
            "url": "https://github.com/owner/repo",
            "name": "my-marketplace",
            "type": "github"
        }
    });
    let settings = make_gen().generate(&manifest, None).unwrap();
    let extra = settings.get("extraKnownMarketplaces").unwrap();
    let entry = extra.get("my-marketplace").unwrap();
    assert_eq!(entry["source"]["repo"], json!("owner/repo"));
}

#[test]
fn generate_rejects_invalid_marketplace_url() {
    let manifest = json!({
        "name": "test",
        "marketplace": {
            "url": "ftp://bad",
            "name": "test-mp"
        }
    });
    let err = make_gen().generate(&manifest, None).unwrap_err();
    assert!(matches!(err, SettingsError::InvalidMarketplaceUrl { .. }));
}

#[test]
fn generate_rejects_unsupported_marketplace_type() {
    let manifest = json!({
        "name": "test",
        "marketplace": {
            "url": "https://github.com/o/r",
            "name": "test-mp",
            "type": "gitlab"
        }
    });
    let err = make_gen().generate(&manifest, None).unwrap_err();
    assert!(matches!(err, SettingsError::UnsupportedMarketplace));
}

#[test]
fn generate_rejects_invalid_marketplace_name() {
    let manifest = json!({
        "name": "test",
        "marketplace": {
            "url": "https://github.com/o/r",
            "name": "INVALID!"
        }
    });
    let err = make_gen().generate(&manifest, None).unwrap_err();
    assert!(matches!(err, SettingsError::InvalidMarketplaceName { .. }));
}

// ── merge_settings tests ────────────────────────────────────────────────

#[test]
fn merge_deep_dicts() {
    let base = json!({ "a": { "x": 1 } });
    let overlay = json!({ "a": { "y": 2 } });
    let merged = SettingsGenerator::merge_settings(&base, &overlay);
    assert_eq!(merged, json!({ "a": { "x": 1, "y": 2 } }));
}

#[test]
fn merge_overlay_takes_precedence() {
    let base = json!({ "key": "old" });
    let overlay = json!({ "key": "new" });
    let merged = SettingsGenerator::merge_settings(&base, &overlay);
    assert_eq!(merged["key"], json!("new"));
}

#[test]
fn merge_concatenates_arrays() {
    let base = json!({ "items": [1, 2] });
    let overlay = json!({ "items": [3, 4] });
    let merged = SettingsGenerator::merge_settings(&base, &overlay);
    assert_eq!(merged["items"], json!([1, 2, 3, 4]));
}

#[test]
fn merge_with_user_settings() {
    let manifest = json!({ "name": "test" });
    let user = json!({ "custom": true });
    let settings = make_gen().generate(&manifest, Some(&user)).unwrap();
    assert_eq!(settings.get("custom"), Some(&json!(true)));
}

// ── write_settings tests ────────────────────────────────────────────────

#[test]
fn write_settings_creates_file() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("subdir").join("settings.json");
    let settings = json!({ "key": "value" });
    assert!(make_gen().write_settings(&settings, &target));
    let content = std::fs::read_to_string(&target).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(parsed["key"], json!("value"));
}

#[test]
fn write_settings_returns_false_on_invalid_path() {
    let settings = json!({ "key": "value" });
    // /dev/null/impossible is not writable as a regular file.
    let result = make_gen().write_settings(&settings, std::path::Path::new("/dev/null/impossible"));
    assert!(!result);
}

// ── semver validation tests ─────────────────────────────────────────────

#[test]
fn semver_valid() {
    assert!(is_valid_semver("1.0.0"));
    assert!(is_valid_semver("0.12.3"));
    assert!(is_valid_semver("1.0.0-beta.1"));
    assert!(is_valid_semver("1.0.0+build.42"));
    assert!(is_valid_semver("1.0.0-rc.1+build"));
}

#[test]
fn semver_invalid() {
    assert!(!is_valid_semver("1.0"));
    assert!(!is_valid_semver("abc"));
    assert!(!is_valid_semver("1.0.0.0"));
    assert!(!is_valid_semver(""));
}

// ── circular reference test ─────────────────────────────────────────────

#[test]
fn no_circular_ref_in_normal_data() {
    let data = json!({ "a": { "b": [1, 2, { "c": 3 }] } });
    assert!(check_circular_reference(&data, &mut HashSet::new()).is_ok());
}

// ── merge_settings as associated function (TDD: these fail until impl) ──
//
// The design requires `merge_settings` to be an associated function (no
// `&self`), callable without constructing a `SettingsGenerator` instance.
// The tests below use `SettingsGenerator::merge_settings(base, overlay)`
// syntax.  They will fail to compile until `&self` is removed from the
// method signature in settings_generator.rs.

#[test]
fn merge_assoc_deep_dicts() {
    let base = json!({ "a": { "x": 1 } });
    let overlay = json!({ "a": { "y": 2 } });
    let merged = SettingsGenerator::merge_settings(&base, &overlay);
    assert_eq!(merged, json!({ "a": { "x": 1, "y": 2 } }));
}

#[test]
fn merge_assoc_overlay_takes_precedence() {
    let base = json!({ "key": "old" });
    let overlay = json!({ "key": "new" });
    let merged = SettingsGenerator::merge_settings(&base, &overlay);
    assert_eq!(merged["key"], json!("new"));
}

#[test]
fn merge_assoc_concatenates_arrays() {
    let base = json!({ "items": [1, 2] });
    let overlay = json!({ "items": [3, 4] });
    let merged = SettingsGenerator::merge_settings(&base, &overlay);
    assert_eq!(merged["items"], json!([1, 2, 3, 4]));
}

#[test]
fn merge_assoc_empty_base() {
    let base = json!({});
    let overlay = json!({ "key": "value" });
    let merged = SettingsGenerator::merge_settings(&base, &overlay);
    assert_eq!(merged, json!({ "key": "value" }));
}

#[test]
fn merge_assoc_empty_overlay() {
    let base = json!({ "key": "value" });
    let overlay = json!({});
    let merged = SettingsGenerator::merge_settings(&base, &overlay);
    assert_eq!(merged, json!({ "key": "value" }));
}

#[test]
fn merge_assoc_non_object_overlay_replaces_base() {
    // When overlay is not an object, it replaces the base entirely.
    let base = json!({ "key": "old" });
    let overlay = json!("scalar");
    let merged = SettingsGenerator::merge_settings(&base, &overlay);
    assert_eq!(merged, json!("scalar"));
}

#[test]
fn merge_assoc_non_object_base_replaced_by_object_overlay() {
    let base = json!("scalar");
    let overlay = json!({ "key": "value" });
    let merged = SettingsGenerator::merge_settings(&base, &overlay);
    assert_eq!(merged, json!({ "key": "value" }));
}

#[test]
fn merge_assoc_three_level_deep_merge() {
    let base = json!({ "a": { "b": { "x": 1, "y": 2 } } });
    let overlay = json!({ "a": { "b": { "y": 99, "z": 3 } } });
    let merged = SettingsGenerator::merge_settings(&base, &overlay);
    assert_eq!(merged, json!({ "a": { "b": { "x": 1, "y": 99, "z": 3 } } }));
}

#[test]
fn merge_assoc_array_extended_with_more_types() {
    let base = json!({ "tags": ["alpha", "beta"] });
    let overlay = json!({ "tags": ["gamma"] });
    let merged = SettingsGenerator::merge_settings(&base, &overlay);
    let tags = merged["tags"].as_array().unwrap();
    assert_eq!(tags.len(), 3);
    assert!(tags.contains(&json!("alpha")));
    assert!(tags.contains(&json!("gamma")));
}
