use super::name::ProfileName;
use super::store::ProfileStore;
use super::{ProfileError, pointer};
use std::fs;

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn store() -> (tempfile::TempDir, ProfileStore) {
    let d = tempfile::tempdir().unwrap();
    let s = ProfileStore::new(d.path().join("root"));
    (d, s)
}

#[test]
fn add_creates_directory_and_fails_on_duplicate() {
    let (_d, s) = store();
    let dir = s.add(&n("work")).unwrap();
    assert!(dir.is_dir());
    assert_eq!(dir.file_name().unwrap(), "work");
    assert!(matches!(
        s.add(&n("work")),
        Err(ProfileError::AlreadyExists(_))
    ));
}

#[test]
fn add_rejects_case_only_duplicates() {
    let (_d, s) = store();
    s.add(&n("Work")).unwrap();
    assert!(matches!(
        s.add(&n("work")),
        Err(ProfileError::AlreadyExists(_))
    ));
}

#[test]
fn add_does_not_create_credentials_or_files_inside() {
    let (_d, s) = store();
    let dir = s.add(&n("work")).unwrap();
    assert_eq!(fs::read_dir(dir).unwrap().count(), 0);
}

#[test]
fn add_copies_allowlisted_shared_assets_and_filters_settings() {
    let d = tempfile::tempdir().unwrap();
    let source = d.path().join("main-claude");
    fs::create_dir_all(source.join("agents")).unwrap();
    fs::create_dir_all(source.join("skills/review")).unwrap();
    fs::create_dir_all(source.join("plugins")).unwrap();
    fs::write(source.join("CLAUDE.md"), "shared instructions").unwrap();
    fs::write(source.join("agents/reviewer.md"), "shared agent").unwrap();
    fs::write(source.join("skills/review/SKILL.md"), "shared skill").unwrap();
    fs::write(source.join("plugins/installed.json"), "plugin state").unwrap();
    fs::write(source.join(".credentials.json"), "auth secret").unwrap();
    fs::write(source.join("history.jsonl"), "session history").unwrap();
    fs::write(
        source.join("settings.json"),
        r#"{
            "model": "sonnet",
            "hooks": {"Stop": [{"command": "echo shared-hook"}]},
            "env": {"API_KEY": "secret"},
            "apiKeyHelper": "secret-helper",
            "mcpServers": {"private": {"command": "private"}},
            "enabledPlugins": {"private": true},
            "unknownSecretSetting": "must not be copied"
        }"#,
    )
    .unwrap();

    let store = ProfileStore::new(d.path().join("profiles")).with_shared_assets(source);
    let profile = store.add(&n("work")).unwrap();

    assert_eq!(
        fs::read_to_string(profile.join("CLAUDE.md")).unwrap(),
        "shared instructions"
    );
    assert_eq!(
        fs::read_to_string(profile.join("agents/reviewer.md")).unwrap(),
        "shared agent"
    );
    assert_eq!(
        fs::read_to_string(profile.join("skills/review/SKILL.md")).unwrap(),
        "shared skill"
    );
    for excluded in [
        ".credentials.json",
        "history.jsonl",
        "plugins",
        "projects",
        "credentials.json",
    ] {
        assert!(
            !profile.join(excluded).exists(),
            "copied excluded {excluded}"
        );
    }
    let copied_settings: serde_json::Value =
        serde_json::from_slice(&fs::read(profile.join("settings.json")).unwrap()).unwrap();
    assert_eq!(copied_settings["model"], "sonnet");
    assert_eq!(
        copied_settings["hooks"]["Stop"][0]["command"],
        "echo shared-hook"
    );
    for excluded in [
        "env",
        "apiKeyHelper",
        "mcpServers",
        "enabledPlugins",
        "unknownSecretSetting",
    ] {
        assert!(
            copied_settings.get(excluded).is_none(),
            "copied setting {excluded}"
        );
    }
}

#[cfg(unix)]
#[test]
fn shared_asset_copy_skips_symlinks_and_keeps_private_modes() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let d = tempfile::tempdir().unwrap();
    let source = d.path().join("main-claude");
    let outside = d.path().join("outside");
    fs::create_dir_all(source.join("agents")).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("secret.md"), "external secret").unwrap();
    symlink(&outside, source.join("agents/external")).unwrap();
    fs::write(source.join("agents/local.md"), "local").unwrap();

    let store = ProfileStore::new(d.path().join("profiles")).with_shared_assets(source);
    let profile = store.add(&n("work")).unwrap();

    assert_eq!(
        fs::read_to_string(profile.join("agents/local.md")).unwrap(),
        "local"
    );
    assert!(!profile.join("agents/external").exists());
    let mode = |path: &std::path::Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&profile.join("agents")), 0o700);
    assert_eq!(mode(&profile.join("agents/local.md")), 0o600);
}

#[cfg(unix)]
#[test]
fn shared_asset_copy_refuses_symlinked_source_root() {
    use std::os::unix::fs::symlink;
    let d = tempfile::tempdir().unwrap();
    let source = d.path().join("main-claude");
    let actual = d.path().join("actual-claude");
    fs::create_dir(&actual).unwrap();
    symlink(&actual, &source).unwrap();
    let store = ProfileStore::new(d.path().join("profiles")).with_shared_assets(source);

    assert!(matches!(
        store.add(&n("work")),
        Err(ProfileError::SymlinkRefused(_))
    ));
    assert!(store.list().unwrap().is_empty());
}

#[test]
fn failed_shared_settings_copy_rolls_back_new_profile() {
    let d = tempfile::tempdir().unwrap();
    let source = d.path().join("main-claude");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("settings.json"), "not json").unwrap();
    let store = ProfileStore::new(d.path().join("profiles")).with_shared_assets(source);

    assert!(matches!(
        store.add(&n("work")),
        Err(ProfileError::InvalidSharedSettings(_))
    ));
    assert!(store.list().unwrap().is_empty());
    assert!(!store.root().join("work").exists());
}

#[cfg(unix)]
#[test]
fn root_and_profile_dirs_are_0700() {
    use std::os::unix::fs::PermissionsExt;
    let (_d, s) = store();
    let dir = s.add(&n("work")).unwrap();
    let mode = |p: &std::path::Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(s.root()), 0o700);
}

#[test]
fn list_is_sorted_and_skips_dot_entries_and_files() {
    let (_d, s) = store();
    s.add(&n("zeta")).unwrap();
    s.add(&n("alpha")).unwrap();
    fs::write(s.root().join(".default"), "alpha").unwrap();
    fs::write(s.root().join("afile"), "x").unwrap();
    fs::create_dir(s.root().join(".hidden")).unwrap();
    assert_eq!(s.list().unwrap(), vec!["alpha", "zeta"]);
}

#[test]
fn list_on_missing_root_is_empty() {
    let (_d, s) = store();
    assert!(s.list().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn list_and_resolve_skip_and_refuse_symlinks() {
    let (d, s) = store();
    s.add(&n("real")).unwrap();
    let outside = d.path().join("outside");
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, s.root().join("evil")).unwrap();
    assert_eq!(s.list().unwrap(), vec!["real"]);
    assert!(matches!(
        s.resolve(&n("evil")),
        Err(ProfileError::SymlinkRefused(_))
    ));
}

#[test]
fn resolve_missing_is_not_found() {
    let (_d, s) = store();
    s.add(&n("a")).unwrap();
    assert!(matches!(
        s.resolve(&n("nope")),
        Err(ProfileError::NotFound(_))
    ));
}

#[test]
fn resolve_returns_path_inside_root() {
    let (_d, s) = store();
    s.add(&n("work")).unwrap();
    let p = s.resolve(&n("work")).unwrap();
    assert!(p.starts_with(s.root().canonicalize().unwrap()));
}

#[test]
fn remove_requires_yes_and_keeps_dir() {
    let (_d, s) = store();
    let dir = s.add(&n("work")).unwrap();
    assert!(matches!(
        s.remove(&n("work"), false),
        Err(ProfileError::ConfirmationRequired)
    ));
    assert!(dir.exists());
}

#[test]
fn remove_deletes_profile_only_and_clears_matching_pointer() {
    let (_d, s) = store();
    s.add(&n("work")).unwrap();
    s.add(&n("home")).unwrap();
    pointer::write(s.root(), &n("work")).unwrap();
    s.remove(&n("work"), true).unwrap();
    assert!(!s.root().join("work").exists());
    assert!(s.root().join("home").exists());
    assert!(s.root().exists());
    assert!(!s.root().join(".default").exists());
}

#[test]
fn remove_keeps_pointer_for_other_profile() {
    let (_d, s) = store();
    s.add(&n("work")).unwrap();
    s.add(&n("home")).unwrap();
    pointer::write(s.root(), &n("home")).unwrap();
    s.remove(&n("work"), true).unwrap();
    assert_eq!(pointer::read(s.root()).unwrap(), Some(n("home")));
}

#[cfg(unix)]
#[test]
fn remove_never_follows_symlink_outside_root() {
    let (d, s) = store();
    s.add(&n("real")).unwrap();
    let outside = d.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep.txt"), "x").unwrap();
    std::os::unix::fs::symlink(&outside, s.root().join("evil")).unwrap();
    assert!(s.remove(&n("evil"), true).is_err());
    assert!(outside.join("keep.txt").exists());
}

#[test]
fn from_env_rejects_relative_and_empty_root() {
    for bad in ["", "relative/path"] {
        assert!(matches!(
            ProfileStore::from_root_str(bad),
            Err(ProfileError::InvalidRoot(_))
        ));
    }
    assert!(ProfileStore::from_root_str("/tmp/abs-root").is_ok());
}

#[cfg(unix)]
#[test]
fn symlinked_root_is_refused_everywhere() {
    let d = tempfile::tempdir().unwrap();
    let real = d.path().join("real");
    fs::create_dir(&real).unwrap();
    let link = d.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let s = ProfileStore::new(link.clone());
    assert!(matches!(s.list(), Err(ProfileError::SymlinkRefused(_))));
    assert!(matches!(
        s.resolve(&n("a")),
        Err(ProfileError::SymlinkRefused(_))
    ));
    assert!(matches!(
        s.remove(&n("a"), true),
        Err(ProfileError::SymlinkRefused(_))
    ));
    assert!(matches!(
        pointer::read(&link),
        Err(ProfileError::SymlinkRefused(_))
    ));
    assert!(matches!(
        pointer::write(&link, &n("a")),
        Err(ProfileError::SymlinkRefused(_))
    ));
    assert!(matches!(
        pointer::clear(&link),
        Err(ProfileError::SymlinkRefused(_))
    ));
}

#[test]
fn missing_root_is_empty_for_readers() {
    let (_d, s) = store();
    assert!(s.list().unwrap().is_empty());
    assert!(pointer::read(s.root()).unwrap().is_none());
}
