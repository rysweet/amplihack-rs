//! Hardening contract for seeding shared Claude assets into a new profile.
//! Every test uses a temp directory as the source; none reads the real HOME.

use super::ProfileError;
use super::name::ProfileName;
use super::store::ProfileStore;
use std::fs;
use std::path::{Path, PathBuf};

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn fixture() -> (tempfile::TempDir, PathBuf, ProfileStore) {
    let d = tempfile::tempdir().unwrap();
    let source = d.path().join("main-claude");
    fs::create_dir_all(&source).unwrap();
    if let Some(home) = std::env::var_os("HOME") {
        assert_ne!(
            source.canonicalize().unwrap(),
            PathBuf::from(home).join(".claude"),
            "tests must never seed from the live ~/.claude"
        );
    }
    let store = ProfileStore::new(d.path().join("profiles")).with_shared_assets(source.clone());
    (d, source, store)
}

#[test]
fn credentials_history_and_state_are_never_copied() {
    let (_d, source, store) = fixture();
    for name in [
        ".credentials.json",
        ".claude.json",
        "history.jsonl",
        "history",
        "statsig",
        "shell-snapshots",
        "ide",
        "projects",
        "sessions",
        "todos",
    ] {
        let path = source.join(name);
        if name.contains('.') {
            fs::write(&path, "secret").unwrap();
        } else {
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("data"), "secret").unwrap();
        }
    }
    fs::write(source.join("CLAUDE.md"), "ok").unwrap();

    let profile = store.add(&n("work")).unwrap();

    let names: Vec<String> = fs::read_dir(&profile)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, vec!["CLAUDE.md".to_string()]);
}

#[test]
fn non_directory_shared_source_is_rejected_and_profile_is_rolled_back() {
    let d = tempfile::tempdir().unwrap();
    let source = d.path().join("main-claude");
    fs::write(&source, "not a directory").unwrap();
    let store = ProfileStore::new(d.path().join("profiles")).with_shared_assets(source);

    assert!(matches!(
        store.add(&n("work")),
        Err(ProfileError::InvalidSharedSource(_))
    ));
    assert!(!store.root().join("work").exists());
    assert!(store.list().unwrap().is_empty());
}

#[test]
fn settings_with_only_non_allowlisted_keys_writes_no_file() {
    let (_d, source, store) = fixture();
    fs::write(source.join("settings.json"), r#"{"env":{"K":"v"}}"#).unwrap();
    let profile = store.add(&n("work")).unwrap();
    assert!(!profile.join("settings.json").exists());
}

#[test]
fn settings_that_are_not_an_object_are_rejected_and_rolled_back() {
    let (_d, source, store) = fixture();
    fs::write(source.join("settings.json"), "[1,2]").unwrap();
    assert!(matches!(
        store.add(&n("work")),
        Err(ProfileError::InvalidSharedSettings(_))
    ));
    assert!(!store.root().join("work").exists());
}

#[test]
fn rollback_after_late_failure_removes_partial_copies() {
    let (_d, source, store) = fixture();
    fs::create_dir_all(source.join("agents")).unwrap();
    fs::write(source.join("agents/a.md"), "a").unwrap();
    fs::write(source.join("settings.json"), "{bad").unwrap();
    assert!(store.add(&n("work")).is_err());
    assert!(!store.root().join("work").exists());
    assert!(store.list().unwrap().is_empty());
}

#[test]
fn failed_add_never_deletes_a_pre_existing_profile() {
    let (_d, source, store) = fixture();
    let existing = store.add(&n("work")).unwrap();
    fs::write(existing.join("keep.txt"), "mine").unwrap();
    fs::write(source.join("settings.json"), "{bad").unwrap();

    assert!(matches!(
        store.add(&n("work")),
        Err(ProfileError::AlreadyExists(_))
    ));
    assert_eq!(
        fs::read_to_string(existing.join("keep.txt")).unwrap(),
        "mine"
    );
}

#[test]
fn copies_are_by_value_not_links() {
    let (_d, source, store) = fixture();
    fs::write(source.join("CLAUDE.md"), "v1").unwrap();
    let profile = store.add(&n("work")).unwrap();
    fs::write(source.join("CLAUDE.md"), "v2").unwrap();
    assert_eq!(fs::read_to_string(profile.join("CLAUDE.md")).unwrap(), "v1");
    assert!(
        !fs::symlink_metadata(profile.join("CLAUDE.md"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[cfg(unix)]
#[test]
fn symlinked_allowlisted_file_and_dir_roots_are_skipped() {
    use std::os::unix::fs::symlink;
    let (d, source, store) = fixture();
    let outside = d.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("x.md"), "external").unwrap();
    fs::write(d.path().join("real.md"), "external").unwrap();
    symlink(d.path().join("real.md"), source.join("CLAUDE.md")).unwrap();
    symlink(&outside, source.join("skills")).unwrap();

    let profile = store.add(&n("work")).unwrap();
    assert!(!profile.join("CLAUDE.md").exists());
    assert!(!profile.join("skills").exists());
}

#[cfg(unix)]
#[test]
fn fifo_inside_allowlisted_tree_is_skipped_without_blocking() {
    let (_d, source, store) = fixture();
    fs::create_dir_all(source.join("agents")).unwrap();
    fs::write(source.join("agents/ok.md"), "ok").unwrap();
    let fifo = std::ffi::CString::new(source.join("agents/pipe").to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);

    let profile = store.add(&n("work")).unwrap();
    assert!(profile.join("agents/ok.md").is_file());
    assert!(!profile.join("agents/pipe").exists());
}

#[cfg(unix)]
#[test]
fn seeded_tree_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let (_d, source, store) = fixture();
    fs::create_dir_all(source.join("skills/a/b")).unwrap();
    fs::write(source.join("skills/a/b/SKILL.md"), "s").unwrap();
    fs::write(source.join("CLAUDE.md"), "c").unwrap();
    fs::set_permissions(source.join("CLAUDE.md"), fs::Permissions::from_mode(0o644)).unwrap();
    fs::write(source.join("settings.json"), r#"{"model":"m"}"#).unwrap();

    let profile = store.add(&n("work")).unwrap();
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&profile), 0o700);
    assert_eq!(mode(&profile.join("skills/a/b")), 0o700);
    assert_eq!(mode(&profile.join("skills/a/b/SKILL.md")), 0o600);
    assert_eq!(mode(&profile.join("CLAUDE.md")), 0o600);
    assert_eq!(mode(&profile.join("settings.json")), 0o600);
}

#[test]
fn excessive_depth_fails_closed_and_rolls_back() {
    let (_d, source, store) = fixture();
    let mut deep = source.join("skills");
    for i in 0..64 {
        deep = deep.join(format!("d{i}"));
    }
    fs::create_dir_all(&deep).unwrap();
    fs::write(deep.join("f.md"), "x").unwrap();

    assert!(matches!(
        store.add(&n("work")),
        Err(ProfileError::SharedAssetLimit(_))
    ));
    assert!(!store.root().join("work").exists());
}

#[test]
fn excessive_entry_count_fails_closed_and_rolls_back() {
    let (_d, source, store) = fixture();
    fs::create_dir_all(source.join("commands")).unwrap();
    for i in 0..5_100 {
        fs::write(source.join(format!("commands/c{i}.md")), "x").unwrap();
    }
    assert!(matches!(
        store.add(&n("work")),
        Err(ProfileError::SharedAssetLimit(_))
    ));
    assert!(!store.root().join("work").exists());
}

#[test]
fn missing_source_root_seeds_nothing_and_succeeds() {
    let d = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(d.path().join("profiles"))
        .with_shared_assets(d.path().join("does-not-exist"));
    let profile = store.add(&n("work")).unwrap();
    assert_eq!(fs::read_dir(profile).unwrap().count(), 0);
}

#[test]
fn oversized_settings_are_rejected_and_rolled_back() {
    let (_d, source, store) = fixture();
    let filler = "x".repeat(1024 * 1024 + 16);
    fs::write(
        source.join("settings.json"),
        format!("{{\"model\":\"{filler}\"}}"),
    )
    .unwrap();
    let err = store.add(&n("big")).unwrap_err();
    assert!(matches!(err, ProfileError::SharedAssetLimit(_)));
    assert!(!store.root().join("big").exists());
}

#[cfg(unix)]
mod race_and_nested {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn nested_symlinked_dir_and_file_are_not_followed() {
        let (d, source, store) = fixture();
        let outside = d.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret"), "TOPSECRET").unwrap();
        let sub = source.join("skills/s");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("ok.md"), "ok").unwrap();
        symlink(&outside, sub.join("linkdir")).unwrap();
        symlink(outside.join("secret"), sub.join("linkfile")).unwrap();
        let dir = store.add(&n("w")).unwrap();
        assert!(dir.join("skills/s/ok.md").is_file());
        assert!(fs::symlink_metadata(dir.join("skills/s/linkdir")).is_err());
        assert!(fs::symlink_metadata(dir.join("skills/s/linkfile")).is_err());
    }

    #[test]
    fn directory_swapped_for_symlink_between_stat_and_open_is_not_followed() {
        let (d, source, store) = fixture();
        let outside = d.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret"), "TOPSECRET").unwrap();
        let sub = source.join("skills/victim");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("a.md"), "a").unwrap();
        // Hook fires after the entry was classified, before it is opened.
        super::super::store::test_hook::set_before_open(Some(Box::new(move |p| {
            if p.ends_with("victim") {
                let _ = fs::remove_dir_all(p);
                let _ = symlink(&outside, p);
            }
        })));
        let res = store.add(&n("w"));
        super::super::store::test_hook::set_before_open(None);
        let dir = store.root().join("w");
        let leaked = dir.exists()
            && walk(&dir).iter().any(|f| {
                fs::read_to_string(f)
                    .map(|s| s.contains("TOPSECRET"))
                    .unwrap_or(false)
            });
        assert!(!leaked, "swapped symlink was followed");
        if let Ok(p) = res {
            assert!(fs::symlink_metadata(p.join("skills/victim/secret")).is_err());
        }
    }

    fn walk(p: &Path) -> Vec<PathBuf> {
        let mut out = vec![];
        if let Ok(rd) = fs::read_dir(p) {
            for e in rd.flatten() {
                let ft = e.file_type().unwrap();
                if ft.is_dir() {
                    out.extend(walk(&e.path()));
                } else if ft.is_file() {
                    out.push(e.path());
                }
            }
        }
        out
    }

    #[test]
    fn copied_files_are_0600_and_dirs_0700() {
        let (_d, source, store) = fixture();
        fs::create_dir_all(source.join("rules/x")).unwrap();
        fs::write(source.join("rules/x/r.md"), "r").unwrap();
        fs::set_permissions(
            source.join("rules/x/r.md"),
            fs::Permissions::from_mode(0o666),
        )
        .unwrap();
        let dir = store.add(&n("w")).unwrap();
        let m = |p: PathBuf| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(m(dir.join("rules/x")), 0o700);
        assert_eq!(m(dir.join("rules/x/r.md")), 0o600);
    }
}
