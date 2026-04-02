use super::helpers::{
    copy_dir_recursive, is_valid_plugin_name, is_valid_semver, plugin_name_from_git_url,
    resolve_manifest_paths,
};
use super::manager::PluginManager;
use super::verifier::PluginVerifier;
use super::*;

fn create_plugin_source(root: &Path, name: &str) -> PathBuf {
    let source = root.join(name);
    fs::create_dir_all(source.join(".claude-plugin")).unwrap();
    fs::create_dir_all(source.join(".claude/tools/amplihack/hooks")).unwrap();
    fs::write(
        source.join(".claude-plugin/plugin.json"),
        serde_json::json!({
            "name": name,
            "version": "1.2.3",
            "entry_point": "main.py",
            "description": "desc",
            "author": "me"
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        source.join(".claude/tools/amplihack/hooks/hooks.json"),
        r#"{"PreToolUse": []}"#,
    )
    .unwrap();
    source
}

#[test]
fn validate_manifest_rejects_bad_name() {
    let temp = tempfile::tempdir().unwrap();
    let source = create_plugin_source(temp.path(), "demo");
    fs::write(
        source.join(".claude-plugin/plugin.json"),
        r#"{"name":"Bad_Name","version":"1.0.0","entry_point":"main.py"}"#,
    )
    .unwrap();
    let manager = PluginManager::new(Some(temp.path().join("plugins"))).unwrap();
    let result = manager
        .validate_manifest(&source.join(".claude-plugin/plugin.json"))
        .unwrap();
    assert!(!result.valid);
}

#[test]
fn install_local_plugin_registers_it() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());
    let source = create_plugin_source(temp.path(), "demo");
    let manager = PluginManager::new(None).unwrap();
    let result = manager.install(source.to_str().unwrap(), false).unwrap();
    assert!(result.success);
    assert!(
        temp.path()
            .join(".amplihack/.claude/plugins/demo/.claude-plugin/plugin.json")
            .exists()
    );
    let plugins: Value = serde_json::from_str(
        &fs::read_to_string(temp.path().join(".config/claude-code/plugins.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(plugins["enabledPlugins"][0], "demo");
    crate::test_support::restore_home(previous);
}

#[test]
fn uninstall_missing_plugin_returns_false() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());
    let manager = PluginManager::new(None).unwrap();
    let result = manager.uninstall("missing").unwrap();
    assert!(!result);
    crate::test_support::restore_home(previous);
}

#[test]
fn verifier_matches_python_checks() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());
    let source = create_plugin_source(temp.path(), "demo");
    fs::create_dir_all(temp.path().join(".amplihack/.claude/plugins")).unwrap();
    copy_dir_recursive(
        &source,
        &temp.path().join(".amplihack/.claude/plugins/demo"),
    )
    .unwrap();
    fs::create_dir_all(temp.path().join(".claude")).unwrap();
    fs::write(
        temp.path().join(".claude/settings.json"),
        r#"{"enabledPlugins":["demo"]}"#,
    )
    .unwrap();
    let result = PluginVerifier::new("demo").unwrap().verify().unwrap();
    assert!(result.success);
    crate::test_support::restore_home(previous);
}

#[test]
fn link_uses_cli_specific_path() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());
    fs::create_dir_all(temp.path().join(".amplihack/plugins/amplihack")).unwrap();
    run_link("amplihack").unwrap();
    let plugins: Value = serde_json::from_str(
        &fs::read_to_string(temp.path().join(".config/claude-code/plugins.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(plugins["enabledPlugins"][0], "amplihack");
    crate::test_support::restore_home(previous);
}

#[test]
fn plugin_name_from_git_url_strips_dot_git() {
    assert_eq!(
        plugin_name_from_git_url("https://example.com/demo.git").unwrap(),
        "demo"
    );
}

#[test]
fn semver_validation_is_strict() {
    assert!(is_valid_semver("1.2.3"));
    assert!(!is_valid_semver("1.2"));
    assert!(!is_valid_semver("1.2.beta"));
}

#[test]
fn plugin_name_validation_matches_python_pattern() {
    assert!(is_valid_plugin_name("demo-plugin1"));
    assert!(!is_valid_plugin_name("Demo"));
    assert!(!is_valid_plugin_name("../demo"));
}

#[test]
fn resolve_manifest_paths_converts_relative_string_fields() {
    let root = Path::new("/plugins/demo");
    let mut manifest = serde_json::json!({
        "name": "demo",
        "entry_point": "main.py",
        "cwd": "src",
        "script": "run.sh",
        "path": "lib/bin",
    });
    resolve_manifest_paths(&mut manifest, root);
    assert_eq!(manifest["entry_point"], "/plugins/demo/main.py");
    assert_eq!(manifest["cwd"], "/plugins/demo/src");
    assert_eq!(manifest["script"], "/plugins/demo/run.sh");
    assert_eq!(manifest["path"], "/plugins/demo/lib/bin");
    // Non-path fields are untouched
    assert_eq!(manifest["name"], "demo");
}

#[test]
fn resolve_manifest_paths_preserves_absolute_paths() {
    let root = Path::new("/plugins/demo");
    let mut manifest = serde_json::json!({
        "entry_point": "/usr/bin/python",
        "script": "/opt/run.sh",
    });
    resolve_manifest_paths(&mut manifest, root);
    assert_eq!(manifest["entry_point"], "/usr/bin/python");
    assert_eq!(manifest["script"], "/opt/run.sh");
}

#[test]
fn resolve_manifest_paths_handles_path_lists() {
    let root = Path::new("/plugins/demo");
    let mut manifest = serde_json::json!({
        "files": ["a.py", "/abs/b.py", "sub/c.py"],
    });
    resolve_manifest_paths(&mut manifest, root);
    let files = manifest["files"].as_array().unwrap();
    assert_eq!(files[0], "/plugins/demo/a.py");
    assert_eq!(files[1], "/abs/b.py");
    assert_eq!(files[2], "/plugins/demo/sub/c.py");
}

#[test]
fn resolve_manifest_paths_recurses_into_nested_objects() {
    let root = Path::new("/plugins/demo");
    let mut manifest = serde_json::json!({
        "name": "demo",
        "hooks": {
            "entry_point": "hooks/main.py",
            "nested": {
                "script": "deep/run.sh",
            }
        }
    });
    resolve_manifest_paths(&mut manifest, root);
    assert_eq!(
        manifest["hooks"]["entry_point"],
        "/plugins/demo/hooks/main.py"
    );
    assert_eq!(
        manifest["hooks"]["nested"]["script"],
        "/plugins/demo/deep/run.sh"
    );
}

#[test]
fn resolve_manifest_paths_ignores_non_path_fields() {
    let root = Path::new("/plugins/demo");
    let mut manifest = serde_json::json!({
        "name": "demo",
        "version": "1.0.0",
        "description": "relative/looking/but/not/a/path",
    });
    let original = manifest.clone();
    resolve_manifest_paths(&mut manifest, root);
    assert_eq!(manifest, original);
}

#[test]
fn resolve_manifest_paths_noop_on_non_object() {
    let root = Path::new("/plugins/demo");
    let mut manifest = serde_json::json!("just a string");
    let original = manifest.clone();
    resolve_manifest_paths(&mut manifest, root);
    assert_eq!(manifest, original);
}
