//! Issue #2279 (C6, C7): profile refresh and profile seeding go through
//! `hook_block`.
//!
//! * `refresh_one` maps `FileMerge` onto `Refresh` (Written→Updated,
//!   AlreadyCurrent→Unchanged, Absent→Missing, Unparseable/Refused→Skipped),
//!   and a Skipped reason never holds serde's text or file content.
//! * A profile seeded from a hook-less home file still gets the full block.
//!   A refused hooks path writes nothing extra and seeding still succeeds.
//!
//! Tests set the seeding target with `ProfileStore::with_seed_hooks`, which
//! stands in for production's `hook_target()` plus `hook_path_refusal`.
//! Every file lives in a temp directory.

#![cfg(unix)]

use super::name::ProfileName;
use super::settings_refresh::{Refresh, refresh_in};
use super::store::ProfileStore;
use crate::commands::install::hook_block::{self, HookTarget};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const SECRET: &str = "sk-test-do-not-print";

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn target() -> HookTarget {
    HookTarget {
        bin: PathBuf::from("/opt/amplihack-test/bin/amplihack-hooks"),
        limit_resume: true,
    }
}

fn private(path: &Path, body: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn read(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn merge_update(s: &mut Value) {
    hook_block::merge(s, &target());
}

fn result<'a>(results: &'a [(String, Refresh)], name: &str) -> &'a Refresh {
    &results.iter().find(|(n, _)| n == name).unwrap().1
}

// ── refresh ───────────────────────────────────────────────────────────────

fn refresh_store() -> (tempfile::TempDir, ProfileStore) {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("profiles");
    fs::create_dir_all(&root).unwrap();
    (d, ProfileStore::new(root))
}

#[test]
fn refresh_maps_every_file_merge_result() {
    let (_d, store) = refresh_store();
    let root = store.root().to_path_buf();

    private(&root.join("stripped/settings.json"), b"{\"hooks\":{}}");
    let mut current = json!({});
    hook_block::merge(&mut current, &target());
    private(
        &root.join("current/settings.json"),
        &serde_json::to_vec_pretty(&current).unwrap(),
    );
    fs::create_dir_all(root.join("bare")).unwrap();
    private(
        &root.join("broken/settings.json"),
        format!("{{\"env\":{{\"K\":\"{SECRET}\"}}, oops").as_bytes(),
    );

    let results = refresh_in(&store, &merge_update);
    assert_eq!(result(&results, "stripped"), &Refresh::Updated);
    assert_eq!(result(&results, "current"), &Refresh::Unchanged);
    assert_eq!(result(&results, "bare"), &Refresh::Missing);
    let Refresh::Skipped(reason) = result(&results, "broken") else {
        panic!("broken must be Skipped");
    };
    assert!(!reason.contains(SECRET), "{reason}");
    assert!(!reason.contains('{'), "{reason}");
    assert!(
        !reason.contains("expected"),
        "no serde error text passes through: {reason}"
    );
    assert!(hook_block::drift(&read(&root.join("stripped/settings.json")), &target()).is_clean());
}

#[test]
fn refresh_never_creates_a_missing_settings_file() {
    let (_d, store) = refresh_store();
    fs::create_dir_all(store.root().join("bare")).unwrap();
    refresh_in(&store, &merge_update);
    assert!(!store.root().join("bare/settings.json").exists());
}

// ── seeding ───────────────────────────────────────────────────────────────

fn seed_fixture(home_settings: &Value) -> (tempfile::TempDir, ProfileStore) {
    let d = tempfile::tempdir().unwrap();
    let source = d.path().join("main-claude");
    fs::create_dir_all(&source).unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o700)).unwrap();
    private(
        &source.join("settings.json"),
        &serde_json::to_vec_pretty(home_settings).unwrap(),
    );
    let store = ProfileStore::new(d.path().join("profiles")).with_shared_assets(source);
    (d, store)
}

#[test]
fn a_profile_seeded_from_a_hook_less_home_file_holds_the_full_block() {
    let (_d, store) = seed_fixture(&json!({
        "hooks": {},
        "language": "en",
        "env": {"XPIA_ENABLED": "false"}
    }));
    let store = store.with_seed_hooks(Ok(target()));
    let profile = store.add(&n("work")).unwrap();
    let s = read(&profile.join("settings.json"));
    assert!(hook_block::drift(&s, &target()).is_clean(), "{s}");
    assert_eq!(s["language"], json!("en"));
    assert!(
        s.get("env").is_none(),
        "env is still filtered out on seeding"
    );
    assert_eq!(
        fs::metadata(profile.join("settings.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn a_profile_seeded_with_no_allowlisted_key_stays_without_settings() {
    let (_d, store) = seed_fixture(&json!({"env": {"A": "1"}}));
    let store = store.with_seed_hooks(Ok(target()));
    let profile = store.add(&n("work")).unwrap();
    assert!(!profile.join("settings.json").exists());
}

#[test]
fn a_refused_hooks_path_writes_no_hooks_and_seeding_still_succeeds() {
    let (_d, store) = seed_fixture(&json!({"hooks": {}, "language": "en"}));
    let store = store.with_seed_hooks(Err("hooks binary is under a temporary directory".into()));
    let profile = store
        .add(&n("work"))
        .expect("seeding never fails on the hook step");
    let s = read(&profile.join("settings.json"));
    assert_eq!(s, json!({"hooks": {}, "language": "en"}));
}
