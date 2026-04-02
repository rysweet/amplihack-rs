use super::helpers::{
    copy_dir_recursive, is_valid_plugin_name, is_valid_semver, plugin_name_from_git_url,
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
