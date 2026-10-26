//! Issue #1661: `amplihack install` refreshes the hooks in each profile's
//! existing settings.json. Every test uses a temp profile root.

use super::settings_refresh::{Refresh, refresh_in};
use super::store::ProfileStore;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

fn store() -> (tempfile::TempDir, ProfileStore) {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("profiles");
    fs::create_dir_all(&root).unwrap();
    (d, ProfileStore::new(root))
}

fn profile(store: &ProfileStore, name: &str, settings: Option<&str>) {
    let dir = store.root().join(name);
    fs::create_dir_all(&dir).unwrap();
    if let Some(text) = settings {
        fs::write(dir.join("settings.json"), text).unwrap();
        fs::set_permissions(dir.join("settings.json"), fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn read(store: &ProfileStore, name: &str) -> Value {
    serde_json::from_str(
        &fs::read_to_string(store.root().join(name).join("settings.json")).unwrap(),
    )
    .unwrap()
}

/// Stands in for install's hook update: sets one hook event.
fn add_stop_failure(settings: &mut Value) {
    settings["hooks"]["StopFailure"] = json!([{"matcher": "rate_limit"}]);
}

fn result<'a>(results: &'a [(String, Refresh)], name: &str) -> &'a Refresh {
    &results.iter().find(|(n, _)| n == name).unwrap().1
}

#[test]
fn hooks_are_updated_and_other_settings_kept() {
    let (_d, store) = store();
    profile(
        &store,
        "work",
        Some(r#"{"hooks":{"Stop":[{"x":1}]},"language":"en"}"#),
    );
    let results = refresh_in(&store, &add_stop_failure);
    assert_eq!(result(&results, "work"), &Refresh::Updated);
    let s = read(&store, "work");
    assert_eq!(s["language"], json!("en"));
    assert_eq!(s["hooks"]["Stop"], json!([{"x": 1}]));
    assert_eq!(
        s["hooks"]["StopFailure"],
        json!([{"matcher": "rate_limit"}])
    );
    let mode = fs::metadata(store.root().join("work/settings.json"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn a_missing_settings_file_is_not_created() {
    let (_d, store) = store();
    profile(&store, "bare", None);
    let results = refresh_in(&store, &add_stop_failure);
    assert_eq!(result(&results, "bare"), &Refresh::Missing);
    assert!(!store.root().join("bare/settings.json").exists());
}

#[test]
fn an_up_to_date_file_is_not_rewritten() {
    let (_d, store) = store();
    profile(
        &store,
        "done",
        Some(r#"{"hooks":{"StopFailure":[{"matcher":"rate_limit"}]}}"#),
    );
    let path = store.root().join("done/settings.json");
    let before = fs::read_to_string(&path).unwrap();
    let results = refresh_in(&store, &add_stop_failure);
    assert_eq!(result(&results, "done"), &Refresh::Unchanged);
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        before,
        "byte-for-byte untouched"
    );
}

#[test]
fn a_symlinked_settings_file_is_skipped_and_its_target_untouched() {
    let (d, store) = store();
    profile(&store, "linked", None);
    let target = d.path().join("elsewhere.json");
    fs::write(&target, "{}").unwrap();
    symlink(&target, store.root().join("linked/settings.json")).unwrap();
    let results = refresh_in(&store, &add_stop_failure);
    assert!(matches!(result(&results, "linked"), Refresh::Skipped(_)));
    assert_eq!(fs::read_to_string(&target).unwrap(), "{}");
}

#[test]
fn invalid_or_non_object_json_is_skipped_unchanged() {
    let (_d, store) = store();
    profile(&store, "broken", Some("{not json"));
    profile(&store, "list", Some("[1,2]"));
    let results = refresh_in(&store, &add_stop_failure);
    assert!(matches!(result(&results, "broken"), Refresh::Skipped(_)));
    assert!(matches!(result(&results, "list"), Refresh::Skipped(_)));
    assert_eq!(
        fs::read_to_string(store.root().join("broken/settings.json")).unwrap(),
        "{not json"
    );
}

#[test]
fn only_real_profile_directories_are_visited() {
    let (d, store) = store();
    profile(&store, "real", Some("{}"));
    let outside = d.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("settings.json"), "{}").unwrap();
    symlink(&outside, store.root().join("alias")).unwrap();
    fs::create_dir_all(store.root().join(".auto-switch.d")).unwrap();
    fs::write(store.root().join(".auto-switch.d/settings.json"), "{}").unwrap();
    let results = refresh_in(&store, &add_stop_failure);
    let names: Vec<&str> = results.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["real"]);
    assert_eq!(
        fs::read_to_string(outside.join("settings.json")).unwrap(),
        "{}"
    );
    assert!(Path::new(&store.root().join(".auto-switch.d/settings.json")).exists());
    assert_eq!(
        fs::read_to_string(store.root().join(".auto-switch.d/settings.json")).unwrap(),
        "{}"
    );
}

#[test]
fn no_root_means_no_profiles() {
    let d = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(d.path().join("absent"));
    assert!(refresh_in(&store, &add_stop_failure).is_empty());
}
