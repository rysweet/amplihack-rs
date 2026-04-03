use super::*;
use crate::plugin_manager_paths::{extract_plugin_name_from_url, validate_path_safety};
use tempfile::TempDir;

/// Create a minimal valid plugin directory.
fn create_plugin_dir(root: &Path, name: &str) -> PathBuf {
    let dir = root.join(name);
    let manifest_dir = dir.join(".claude-plugin");
    std::fs::create_dir_all(&manifest_dir).unwrap();
    std::fs::write(
        manifest_dir.join("plugin.json"),
        r#"{"name":"test-plugin","version":"1.0.0","entry_point":"main.py"}"#,
    )
    .unwrap();
    std::fs::write(dir.join("main.py"), "# entry point").unwrap();
    dir
}

#[test]
fn install_from_local_path() {
    let tmp = TempDir::new().unwrap();
    let source = create_plugin_dir(tmp.path(), "test-plugin");
    let install_root = tmp.path().join("installed");
    let settings = tmp.path().join("settings.json");

    let mgr = PluginManager::with_paths(install_root.clone(), settings);
    let result = mgr.install(source.to_str().unwrap(), false);

    assert!(result.success, "install failed: {}", result.message);
    assert_eq!(result.plugin_name, "test-plugin");
    assert!(install_root.join("test-plugin").exists());
    assert!(install_root.join("test-plugin").join("main.py").exists());
}

#[test]
fn install_rejects_nonexistent_source() {
    let tmp = TempDir::new().unwrap();
    let mgr =
        PluginManager::with_paths(tmp.path().join("plugins"), tmp.path().join("settings.json"));
    let result = mgr.install("/nonexistent/path", false);
    assert!(!result.success);
    assert!(result.message.contains("must be a directory"));
}

#[test]
fn install_rejects_empty_source() {
    let tmp = TempDir::new().unwrap();
    let mgr =
        PluginManager::with_paths(tmp.path().join("plugins"), tmp.path().join("settings.json"));
    let result = mgr.install("", false);
    assert!(!result.success);
    assert!(result.message.contains("Empty source"));
}

#[test]
fn install_rejects_duplicate_without_force() {
    let tmp = TempDir::new().unwrap();
    let source = create_plugin_dir(tmp.path(), "test-plugin");
    let install_root = tmp.path().join("installed");
    let settings = tmp.path().join("settings.json");

    let mgr = PluginManager::with_paths(install_root, settings);
    let r1 = mgr.install(source.to_str().unwrap(), false);
    assert!(r1.success);

    let r2 = mgr.install(source.to_str().unwrap(), false);
    assert!(!r2.success);
    assert!(r2.message.contains("already installed"));
}

#[test]
fn install_force_overwrites_existing() {
    let tmp = TempDir::new().unwrap();
    let source = create_plugin_dir(tmp.path(), "test-plugin");
    let install_root = tmp.path().join("installed");
    let settings = tmp.path().join("settings.json");

    let mgr = PluginManager::with_paths(install_root, settings);
    let r1 = mgr.install(source.to_str().unwrap(), false);
    assert!(r1.success);

    let r2 = mgr.install(source.to_str().unwrap(), true);
    assert!(r2.success, "force install failed: {}", r2.message);
}

#[test]
fn uninstall_removes_plugin() {
    let tmp = TempDir::new().unwrap();
    let source = create_plugin_dir(tmp.path(), "test-plugin");
    let install_root = tmp.path().join("installed");
    let settings = tmp.path().join("settings.json");

    let mgr = PluginManager::with_paths(install_root.clone(), settings);
    mgr.install(source.to_str().unwrap(), false);

    assert!(mgr.uninstall("test-plugin"));
    assert!(!install_root.join("test-plugin").exists());
}

#[test]
fn uninstall_returns_false_for_missing_plugin() {
    let tmp = TempDir::new().unwrap();
    let mgr =
        PluginManager::with_paths(tmp.path().join("plugins"), tmp.path().join("settings.json"));
    assert!(!mgr.uninstall("no-such-plugin"));
}

#[test]
fn list_installed_finds_valid_plugins() {
    let tmp = TempDir::new().unwrap();
    let install_root = tmp.path().join("plugins");

    create_plugin_dir(&install_root, "alpha-plugin");
    create_plugin_dir(&install_root, "beta-plugin");

    std::fs::create_dir_all(install_root.join("not-a-plugin")).unwrap();

    let mgr = PluginManager::with_paths(install_root, tmp.path().join("settings.json"));
    let list = mgr.list_installed();
    assert_eq!(list, vec!["alpha-plugin", "beta-plugin"]);
}

#[test]
fn list_installed_empty_when_no_plugins() {
    let tmp = TempDir::new().unwrap();
    let mgr =
        PluginManager::with_paths(tmp.path().join("plugins"), tmp.path().join("settings.json"));
    assert!(mgr.list_installed().is_empty());
}

#[test]
fn resolve_paths_makes_relative_absolute() {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path().join("plugin");
    std::fs::create_dir_all(&base).unwrap();

    let mgr =
        PluginManager::with_paths(tmp.path().join("plugins"), tmp.path().join("settings.json"));

    let mut manifest = serde_json::Map::new();
    manifest.insert(
        "entry_point".to_string(),
        serde_json::Value::String("src/main.py".to_string()),
    );
    manifest.insert(
        "name".to_string(),
        serde_json::Value::String("my-plugin".to_string()),
    );

    let resolved = mgr.resolve_paths(&manifest, Some(&base)).unwrap();
    let ep = resolved["entry_point"].as_str().unwrap();
    assert!(ep.starts_with(base.to_str().unwrap()));
    assert!(ep.ends_with("src/main.py"));
    assert_eq!(resolved["name"].as_str().unwrap(), "my-plugin");
}

#[test]
fn resolve_paths_preserves_absolute() {
    let tmp = TempDir::new().unwrap();
    let mgr =
        PluginManager::with_paths(tmp.path().join("plugins"), tmp.path().join("settings.json"));

    let mut manifest = serde_json::Map::new();
    manifest.insert(
        "entry_point".to_string(),
        serde_json::Value::String("/absolute/path/main.py".to_string()),
    );

    let resolved = mgr.resolve_paths(&manifest, None).unwrap();
    assert_eq!(
        resolved["entry_point"].as_str().unwrap(),
        "/absolute/path/main.py"
    );
}

#[test]
fn resolve_paths_handles_arrays() {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path().join("plugin");
    std::fs::create_dir_all(&base).unwrap();

    let mgr =
        PluginManager::with_paths(tmp.path().join("plugins"), tmp.path().join("settings.json"));

    let mut manifest = serde_json::Map::new();
    manifest.insert(
        "files".to_string(),
        serde_json::json!(["src/a.py", "/abs/b.py"]),
    );

    let resolved = mgr.resolve_paths(&manifest, Some(&base)).unwrap();
    let files = resolved["files"].as_array().unwrap();
    assert!(files[0].as_str().unwrap().ends_with("src/a.py"));
    assert_eq!(files[1].as_str().unwrap(), "/abs/b.py");
}

#[test]
fn resolve_paths_recurses_into_nested_objects() {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path().join("plugin");
    std::fs::create_dir_all(&base).unwrap();

    let mgr =
        PluginManager::with_paths(tmp.path().join("plugins"), tmp.path().join("settings.json"));

    let mut manifest = serde_json::Map::new();
    let mut nested = serde_json::Map::new();
    nested.insert(
        "script".to_string(),
        serde_json::Value::String("run.sh".to_string()),
    );
    manifest.insert("hooks".to_string(), serde_json::Value::Object(nested));

    let resolved = mgr.resolve_paths(&manifest, Some(&base)).unwrap();
    let script = resolved["hooks"]["script"].as_str().unwrap();
    assert!(script.starts_with(base.to_str().unwrap()));
}

#[test]
fn extract_name_from_git_url() {
    assert_eq!(
        extract_plugin_name_from_url("https://github.com/org/my-plugin.git"),
        "my-plugin"
    );
    assert_eq!(
        extract_plugin_name_from_url("https://github.com/org/my-plugin"),
        "my-plugin"
    );
    assert_eq!(
        extract_plugin_name_from_url("git@github.com:org/cool-plugin.git"),
        "cool-plugin"
    );
    assert_eq!(
        extract_plugin_name_from_url("https://github.com/org/plugin/"),
        "plugin"
    );
}

#[test]
fn validate_path_safety_accepts_children() {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path();
    let child = base.join("subdir").join("file.txt");
    assert!(validate_path_safety(&child, base));
}

#[test]
fn validate_path_safety_rejects_traversal() {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path().join("plugin");
    std::fs::create_dir_all(&base).unwrap();
    let escaped = base.join("..").join("..").join("etc").join("passwd");
    assert!(!validate_path_safety(&escaped, &base));
}

#[test]
fn register_plugin_creates_settings_file() {
    let tmp = TempDir::new().unwrap();
    let settings = tmp.path().join("config").join("plugins.json");
    let mgr = PluginManager::with_paths(tmp.path().join("plugins"), settings.clone());

    mgr.register_plugin("my-plugin").unwrap();

    let text = std::fs::read_to_string(&settings).unwrap();
    let val: serde_json::Value = serde_json::from_str(&text).unwrap();
    let arr = val["enabledPlugins"].as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0].as_str().unwrap(), "my-plugin");
}

#[test]
fn register_plugin_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let settings = tmp.path().join("plugins.json");
    let mgr = PluginManager::with_paths(tmp.path().join("plugins"), settings.clone());

    mgr.register_plugin("x").unwrap();
    mgr.register_plugin("x").unwrap();

    let text = std::fs::read_to_string(&settings).unwrap();
    let val: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(val["enabledPlugins"].as_array().unwrap().len(), 1);
}
