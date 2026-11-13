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
    // A umask-independent mode, so the shared-session root check (#1634) passes.
    #[cfg(unix)]
    fs::set_permissions(&source, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
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

fn numbered_files(dir: &Path, count: usize) {
    fs::create_dir_all(dir).unwrap();
    for i in 0..count {
        fs::write(dir.join(format!("{i}.md")), format!("skill {i}\n")).unwrap();
    }
}

/// Includes hidden entries and directories, but never follows session links.
fn inventory(root: &Path) -> Vec<PathBuf> {
    fn visit(root: &Path, dir: &Path, paths: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            paths.push(entry.path().strip_prefix(root).unwrap().to_path_buf());
            if entry.file_type().unwrap().is_dir() {
                visit(root, &entry.path(), paths);
            }
        }
    }
    let mut paths = Vec::new();
    visit(root, root, &mut paths);
    paths.sort();
    paths
}

#[test]
fn issue_2522_oversized_account_skill_caches_are_omitted_and_sources_preserved() {
    let (_d, source, store) = fixture();
    numbered_files(&source.join("skills/synced"), 5_001);
    numbered_files(&source.join("skills/.trash"), 3);
    let skills = [
        ("skills/review/SKILL.md", b"review\n\0original".as_slice()),
        ("skills/build/SKILL.md", b"build\n".as_slice()),
        ("skills/test/SKILL.md", b"test\n".as_slice()),
    ];
    for (relative, bytes) in skills {
        fs::create_dir_all(source.join(relative).parent().unwrap()).unwrap();
        fs::write(source.join(relative), bytes).unwrap();
    }

    let profile = store
        .add(&n("work"))
        .expect("excluded caches use no copy budget");

    for (relative, bytes) in skills {
        assert_eq!(fs::read(profile.join(relative)).unwrap(), bytes);
        assert_eq!(fs::read(source.join(relative)).unwrap(), bytes);
    }
    for (relative, count) in [("skills/synced", 5_001), ("skills/.trash", 3)] {
        assert_eq!(
            fs::symlink_metadata(profile.join(relative))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::NotFound,
            "excluded path {relative} must not be created"
        );
        let dir = source.join(relative);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), count);
        for i in 0..count {
            assert_eq!(
                fs::read(dir.join(format!("{i}.md"))).unwrap(),
                format!("skill {i}\n").as_bytes()
            );
        }
    }
}

#[test]
fn issue_2522_exclusions_match_only_the_two_exact_skill_paths() {
    let (_d, source, store) = fixture();
    let included = [
        "agents/synced/agent.md",
        "agents/.trash/agent.md",
        "commands/synced/command.md",
        "output-styles/.trash/style.md",
        "rules/synced/rule.md",
        "skills/custom/.trash/SKILL.md",
        "skills/custom/synced/SKILL.md",
        "skills/synced-custom/SKILL.md",
        "skills/.trash-custom/SKILL.md",
    ];
    for relative in included {
        fs::create_dir_all(source.join(relative).parent().unwrap()).unwrap();
        fs::write(source.join(relative), relative).unwrap();
    }
    for relative in ["skills/synced", "skills/.trash"] {
        fs::create_dir_all(source.join(relative)).unwrap();
    }

    let profile = store.add(&n("work")).unwrap();

    for relative in included {
        assert_eq!(
            fs::read(profile.join(relative)).unwrap(),
            relative.as_bytes()
        );
    }
    for relative in ["skills/synced", "skills/.trash"] {
        assert!(fs::symlink_metadata(profile.join(relative)).is_err());
        assert!(source.join(relative).is_dir());
    }
}

#[test]
fn issue_2522_case_distinct_skill_names_are_still_copied() {
    let (_d, source, store) = fixture();
    // Keep lower-case counterparts absent so this also works on macOS
    // volumes whose filesystem is case-insensitive.
    for relative in ["skills/Synced/SKILL.md", "skills/.Trash/SKILL.md"] {
        fs::create_dir_all(source.join(relative).parent().unwrap()).unwrap();
        fs::write(source.join(relative), relative).unwrap();
    }

    let profile = store.add(&n("work")).unwrap();

    for relative in ["skills/Synced/SKILL.md", "skills/.Trash/SKILL.md"] {
        assert_eq!(
            fs::read(profile.join(relative)).unwrap(),
            relative.as_bytes()
        );
    }
}

#[test]
fn issue_2522_excluded_paths_are_omitted_even_when_regular_files() {
    let (_d, source, store) = fixture();
    fs::create_dir(source.join("skills")).unwrap();
    for relative in ["skills/synced", "skills/.trash"] {
        fs::write(source.join(relative), b"account state").unwrap();
    }

    let profile = store.add(&n("work")).unwrap();

    for relative in ["skills/synced", "skills/.trash"] {
        assert!(fs::symlink_metadata(profile.join(relative)).is_err());
        assert_eq!(fs::read(source.join(relative)).unwrap(), b"account state");
    }
}

#[test]
fn issue_2522_excluded_descendants_do_not_trigger_depth_or_file_size_limits() {
    let (_d, source, store) = fixture();
    for relative in ["skills/synced", "skills/.trash"] {
        let dir = source.join(relative);
        fs::create_dir_all(&dir).unwrap();
        fs::File::create(dir.join("oversized"))
            .unwrap()
            .set_len(64 * 1024 * 1024 + 1)
            .unwrap();
        let deep = (0..18).fold(dir, |path, i| path.join(format!("d{i}")));
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("keep"), "untouched").unwrap();
    }

    let profile = store
        .add(&n("work"))
        .expect("excluded trees are never traversed");

    for relative in ["skills/synced", "skills/.trash"] {
        assert!(fs::symlink_metadata(profile.join(relative)).is_err());
        assert_eq!(
            fs::metadata(source.join(relative).join("oversized"))
                .unwrap()
                .len(),
            64 * 1024 * 1024 + 1
        );
        let deep = (0..18).fold(source.join(relative), |path, i| path.join(format!("d{i}")));
        assert_eq!(fs::read(deep.join("keep")).unwrap(), b"untouched");
    }
}

#[test]
fn issue_2522_excluded_directory_entries_do_not_consume_the_exact_5000_budget() {
    let (_d, source, store) = fixture();
    // 2,499 agent files + the custom directory entry + 2,500 skill files.
    numbered_files(&source.join("agents"), 2_499);
    numbered_files(&source.join("skills/custom"), 2_500);
    for relative in ["skills/synced", "skills/.trash"] {
        fs::create_dir_all(source.join(relative)).unwrap();
    }

    let profile = store
        .add(&n("work"))
        .expect("exactly 5,000 included entries fit");

    assert_eq!(fs::read_dir(profile.join("agents")).unwrap().count(), 2_499);
    assert_eq!(
        fs::read_dir(profile.join("skills/custom")).unwrap().count(),
        2_500
    );
    assert_eq!(fs::read_dir(profile.join("skills")).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn issue_2522_unreadable_excluded_directories_are_never_opened() {
    use std::os::unix::fs::PermissionsExt;
    let (_d, source, store) = fixture();
    let excluded = [source.join("skills/synced"), source.join("skills/.trash")];
    for dir in &excluded {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("keep"), "account state").unwrap();
        fs::set_permissions(dir, fs::Permissions::from_mode(0o000)).unwrap();
    }

    let result = store.add(&n("work"));

    // Restore access before asserting, including on the expected red run,
    // so TempDir can remove every fixture entry.
    for dir in &excluded {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let profile = result.expect("excluded directories must not be opened");
    for dir in &excluded {
        assert_eq!(fs::read(dir.join("keep")).unwrap(), b"account state");
        assert!(fs::symlink_metadata(profile.join(dir.strip_prefix(&source).unwrap())).is_err());
    }
}

#[cfg(unix)]
#[test]
fn issue_2522_excluded_symlink_and_fifo_entries_do_not_consume_the_budget() {
    use std::os::unix::fs::{FileTypeExt, symlink};
    let (d, source, store) = fixture();
    numbered_files(&source.join("agents"), 5_000);
    fs::create_dir(source.join("skills")).unwrap();
    let outside = d.path().join("outside");
    fs::write(&outside, "private").unwrap();
    symlink(&outside, source.join("skills/synced")).unwrap();
    let fifo = std::ffi::CString::new(source.join("skills/.trash").as_os_str().as_encoded_bytes())
        .unwrap();
    // SAFETY: a valid NUL-terminated path in this test's temporary source.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);

    let profile = store
        .add(&n("work"))
        .expect("excluded special entries use no budget");

    assert_eq!(fs::read_dir(profile.join("agents")).unwrap().count(), 5_000);
    for relative in ["skills/synced", "skills/.trash"] {
        assert!(fs::symlink_metadata(profile.join(relative)).is_err());
    }
    assert_eq!(
        fs::read_link(source.join("skills/synced")).unwrap(),
        outside
    );
    assert_eq!(fs::read(&outside).unwrap(), b"private");
    assert!(
        fs::symlink_metadata(source.join("skills/.trash"))
            .unwrap()
            .file_type()
            .is_fifo()
    );
}

#[test]
fn issue_2522_entry_limit_is_aggregate_across_shared_folders() {
    let (_d, source, store) = fixture();
    numbered_files(&source.join("agents"), 2_501);
    numbered_files(&source.join("commands"), 2_500);

    let error = store.add(&n("work")).unwrap_err();

    let ProfileError::SharedAssetLimit(detail) = error else {
        panic!("expected an entry-limit error, got {error:?}");
    };
    assert!(detail.contains("more than 5000 entries"), "{detail}");
    let folder = detail
        .split_once("while copying shared folder ")
        .expect("limit error must identify its source-relative folder")
        .1;
    assert!(["agents", "commands"].contains(&folder), "{detail}");
    assert!(!store.root().join("work").exists());
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

    // Issue #1634: the session dirs are links to the source, never copies.
    let mut names: Vec<String> = fs::read_dir(&profile)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    if cfg!(not(unix)) {
        assert_eq!(names, vec!["CLAUDE.md".to_string()]);
        return;
    }
    assert_eq!(
        names,
        ["CLAUDE.md", "file-history", "projects", "session-env"].map(String::from)
    );
    assert!(
        fs::symlink_metadata(profile.join("CLAUDE.md"))
            .unwrap()
            .is_file()
    );
    let canonical_source = source.canonicalize().unwrap();
    for name in ["file-history", "projects", "session-env"] {
        let link = profile.join(name);
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "{name} copied by value"
        );
        assert_eq!(fs::read_link(&link).unwrap(), canonical_source.join(name));
    }
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
    let existing = store.add(&n("existing")).unwrap();
    fs::write(existing.join(".sentinel"), "existing profile bytes").unwrap();
    let before = inventory(store.root());
    numbered_files(&source.join("skills/custom"), 5_001);

    let error = store.add(&n("work")).unwrap_err();

    assert_eq!(
        inventory(store.root()),
        before,
        "failed add left staging or partial profile entries"
    );
    assert_eq!(
        fs::read(existing.join(".sentinel")).unwrap(),
        b"existing profile bytes"
    );
    assert!(!store.root().join("work").exists());
    assert_eq!(
        fs::read_dir(source.join("skills/custom")).unwrap().count(),
        5_001
    );
    let ProfileError::SharedAssetLimit(detail) = error else {
        panic!("expected an entry-limit error, got {error:?}");
    };
    assert!(detail.contains("more than 5000 entries"), "{detail}");
    assert!(
        detail.contains(&format!(
            "while copying shared folder {}",
            Path::new("skills").join("custom").display()
        )),
        "{detail}"
    );
    assert!(
        !detail.contains(&source.display().to_string()),
        "diagnostic must be source-relative: {detail}"
    );
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
        // The hook receives the display path relative to the source root, so
        // resolve it against `source`; a bare relative path would act on the
        // process working directory and could plant a symlink in the checkout.
        // The hook must succeed and must fire; otherwise the test checks
        // nothing, which is how the relative-path bug went unnoticed.
        let source_root = source.clone();
        let swapped = std::rc::Rc::new(std::cell::Cell::new(false));
        let hook_swapped = std::rc::Rc::clone(&swapped);
        super::super::store::test_hook::set_before_open(Some(Box::new(move |p| {
            if p.ends_with("victim") {
                let target = source_root.join(p);
                fs::remove_dir_all(&target).unwrap();
                symlink(&outside, &target).unwrap();
                hook_swapped.set(true);
            }
        })));
        let res = store.add(&n("w"));
        super::super::store::test_hook::set_before_open(None);
        assert!(
            swapped.get(),
            "before_open hook never swapped the directory"
        );
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

    #[test]
    fn source_root_swapped_to_symlink_after_anchor_copies_no_outside_data() {
        let (d, source, store) = fixture();
        fs::create_dir_all(source.join("skills/real")).unwrap();
        fs::write(source.join("skills/real/a.md"), "REAL").unwrap();
        fs::write(source.join("CLAUDE.md"), "REAL-CLAUDE").unwrap();
        fs::write(source.join("settings.json"), r#"{"model":"real"}"#).unwrap();

        let outside = d.path().join("outside");
        fs::create_dir_all(outside.join("skills/evil")).unwrap();
        fs::write(outside.join("skills/evil/s.md"), "TOPSECRET").unwrap();
        fs::write(outside.join("CLAUDE.md"), "TOPSECRET").unwrap();
        fs::write(outside.join("settings.json"), r#"{"model":"TOPSECRET"}"#).unwrap();

        let aside = d.path().join("aside");
        let src = source.clone();
        // Fires after the source root is anchored, before any entry is read.
        super::super::store::test_hook::set_after_root_anchor(Some(Box::new(move |_| {
            fs::rename(&src, &aside).unwrap();
            symlink(&outside, &src).unwrap();
        })));
        let res = store.add(&n("w"));
        super::super::store::test_hook::set_after_root_anchor(None);

        let dir = store.root().join("w");
        assert!(res.is_ok(), "{res:?}");
        let files = walk(&dir);
        assert!(
            !files.iter().any(|f| fs::read_to_string(f)
                .map(|s| s.contains("TOPSECRET"))
                .unwrap_or(false)),
            "outside data copied via swapped source root"
        );
        assert_eq!(
            fs::read_to_string(dir.join("skills/real/a.md")).unwrap(),
            "REAL"
        );
        assert_eq!(
            fs::read_to_string(dir.join("CLAUDE.md")).unwrap(),
            "REAL-CLAUDE"
        );
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
