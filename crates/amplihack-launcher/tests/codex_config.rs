//! Issue #1538: project-scoped compatibility entrypoint must use preserving TOML.
//! User CODEX_HOME/bootstrap transactions have separate CLI coverage.
use amplihack_launcher::codex::configure_codex;
use std::fs;

#[test]
fn ordinary_configuration_never_creates_legacy_yaml_or_approval_override() {
    let dir = tempfile::tempdir().unwrap();
    configure_codex(dir.path()).unwrap();
    assert!(!dir.path().join(".codex/config.yaml").exists());
    let path = dir.path().join(".codex/config.toml");
    if path.exists() {
        assert!(
            !fs::read_to_string(path)
                .unwrap()
                .contains("approval_policy")
        );
    }
}
#[test]
fn existing_preferences_comments_profiles_and_plugins_are_byte_preserved() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join(".codex")).unwrap();
    let path = dir.path().join(".codex/config.toml");
    let original = "# user preference\napproval_policy = { reject = { sandbox_approval = true } }\nsandbox_mode = \"read-only\"\n[profiles.custom]\nmodel = \"user-choice\"\n[plugins.foreign]\nenabled = true\n";
    fs::write(&path, original).unwrap();
    configure_codex(dir.path()).unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), original);
    assert!(!dir.path().join(".codex/config.yaml").exists());
}
#[test]
fn malformed_toml_is_refused_without_modification() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join(".codex")).unwrap();
    let path = dir.path().join(".codex/config.toml");
    fs::write(&path, "approval_policy = [broken").unwrap();
    let result = configure_codex(dir.path());
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        "approval_policy = [broken"
    );
    assert!(result.is_err(), "malformed config must report remediation");
}
#[cfg(unix)]
#[test]
fn symlink_configuration_is_refused_and_target_is_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("user.toml");
    fs::write(&target, "# outside config\n").unwrap();
    fs::create_dir(dir.path().join(".codex")).unwrap();
    std::os::unix::fs::symlink(&target, dir.path().join(".codex/config.toml")).unwrap();
    assert!(configure_codex(dir.path()).is_err());
    assert_eq!(fs::read_to_string(target).unwrap(), "# outside config\n");
}
#[test]
fn ambiguous_legacy_yaml_is_preserved() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join(".codex")).unwrap();
    let path = dir.path().join(".codex/config.yaml");
    fs::write(&path, "approval_mode: auto\ncustom: true\n").unwrap();
    configure_codex(dir.path()).unwrap();
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        "approval_mode: auto\ncustom: true\n"
    );
}
