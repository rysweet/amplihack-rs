//! Unit tests for `sweep.rs` (issue #1808): the apply-time re-checks and the
//! failure paths that must stay visible. `collect` and `apply_items` are run
//! separately so a test can change the filesystem between them (§8 test 3).

use super::*;
use std::os::unix::fs::{PermissionsExt, symlink};

struct Fx {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    work: WorkRoot,
    hook_base: PathBuf,
}

impl Fx {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap();
        let work = WorkRoot {
            path: root.join("work"),
            explicit: false,
        };
        let hook_base = root.join("cache/amplihack");
        for dir in [work.runs_dir(), work.targets_dir(), hook_base.clone()] {
            fs::create_dir_all(&dir).unwrap();
        }
        Self {
            _tmp: tmp,
            root,
            work,
            hook_base,
        }
    }

    fn settings(&self, categories: &[Category]) -> Settings {
        Settings {
            work_root: self.work.clone(),
            keep: Duration::ZERO,
            log_age: Duration::MAX,
            archive_dir: self.work.path.join("archive"),
            categories: categories.iter().copied().collect(),
            apply: true,
            report_unrecorded: false,
            deadline: None,
            recipe_log_dir: None,
            codex_sessions_dir: None,
            claude_projects_dir: None,
            hook_cache_base: Some(self.hook_base.clone()),
        }
    }

    /// An ended, dead run that owns `targets` and has a `runtime/` folder.
    fn run(&self, targets: &[&Path]) -> PathBuf {
        let dir = self
            .work
            .runs_dir()
            .join("0123abcd-0000-0000-0000-000000000001");
        fs::create_dir_all(dir.join("runtime/tmp")).unwrap();
        let report = RunReport {
            schema_version: RUN_SCHEMA_VERSION,
            run_id: "0123abcd-0000-0000-0000-000000000001".into(),
            recipe: "test".into(),
            pid: 0,
            pid_start_ticks: None,
            cwd: String::new(),
            started_at: now_rfc3339(),
            ended_at: Some(now_rfc3339()),
            outcome: "success".into(),
            attempts: Vec::new(),
            steps: Vec::new(),
            log_path: None,
        };
        fs::write(dir.join(RUN_JSON), serde_json::to_vec(&report).unwrap()).unwrap();
        let mut owned = Vec::new();
        for t in targets {
            owned.extend(format_owned_line(&OwnedPath {
                kind: OwnedKind::Target,
                path: t.to_path_buf(),
                branch: None,
            }));
        }
        fs::write(dir.join(OWNED_PATHS), owned).unwrap();
        dir
    }
}

fn dir_with_file(path: &Path) -> PathBuf {
    fs::create_dir_all(path).unwrap();
    fs::write(path.join("keep-me"), "data").unwrap();
    path.to_path_buf()
}

fn entry<'a>(entries: &'a [Entry], path: &Path) -> &'a Entry {
    entries
        .iter()
        .find(|e| e.path == path)
        .unwrap_or_else(|| panic!("no entry for {}: {entries:#?}", path.display()))
}

/// §8 test 3: a recorded path whose symlinked parent is repointed between
/// collection and apply is refused, and the folder it now names survives.
#[test]
fn symlinked_parent_repointed_after_collection_is_refused() {
    let fx = Fx::new();
    dir_with_file(&fx.work.targets_dir().join("foo"));
    let outside = dir_with_file(&fx.root.join("projects/foo"));
    let lnk = fx.root.join("lnk");
    symlink(fx.work.targets_dir(), &lnk).unwrap();
    let recorded = lnk.join("foo");
    fx.run(&[&recorded]);
    let settings = fx.settings(&[Category::RunTarget]);

    let items = collect(&settings).unwrap();
    fs::remove_file(&lnk).unwrap();
    symlink(fx.root.join("projects"), &lnk).unwrap();
    let entries = apply_items(items, &settings, true);

    let e = entry(&entries, &recorded);
    assert_eq!(e.action, Action::Skipped, "{e:?}");
    assert!(e.reason.contains("resolves"), "{e:?}");
    assert!(outside.join("keep-me").exists(), "outside folder deleted");
}

/// A contained item whose canonical path changes to another managed folder
/// is still skipped: only the folder seen at collection may be removed.
#[test]
fn canonical_change_inside_managed_roots_is_skipped() {
    let fx = Fx::new();
    let name = "amplihack-precommit-target-0123456789ab";
    dir_with_file(&fx.work.targets_dir().join(name));
    let other = dir_with_file(&fx.hook_base.join(name));
    let lnk = fx.root.join("lnk");
    symlink(fx.work.targets_dir(), &lnk).unwrap();
    let recorded = lnk.join(name);
    fx.run(&[&recorded]);
    let settings = fx.settings(&[Category::RunTarget]);

    let items = collect(&settings).unwrap();
    fs::remove_file(&lnk).unwrap();
    symlink(&fx.hook_base, &lnk).unwrap();
    let entries = apply_items(items, &settings, true);

    let e = entry(&entries, &recorded);
    assert_eq!(e.action, Action::Skipped, "{e:?}");
    assert!(other.join("keep-me").exists());
    assert!(fx.work.targets_dir().join(name).join("keep-me").exists());
}

/// P0-2: `run.json` that becomes unreadable after collection means skip,
/// for run targets as well as run-tmp.
#[test]
fn unreadable_run_json_at_apply_skips() {
    let fx = Fx::new();
    let target = dir_with_file(&fx.work.targets_dir().join("t1"));
    let run_dir = fx.run(&[&target]);
    let settings = fx.settings(&[Category::RunTarget, Category::RunTmp]);

    let items = collect(&settings).unwrap();
    assert_eq!(
        items.iter().filter(|i| !matches!(i.op, Op::Skip)).count(),
        2,
        "{items:#?}"
    );
    fs::write(run_dir.join(RUN_JSON), "{ not json").unwrap();
    let entries = apply_items(items, &settings, true);

    for path in [target.clone(), run_dir.join("runtime")] {
        let e = entry(&entries, &path);
        assert_eq!(e.action, Action::Skipped, "{e:?}");
        assert!(e.reason.contains("run.json"), "{e:?}");
        assert!(path.exists());
    }
}

/// The happy path still removes, so the refusals above are not vacuous.
#[test]
fn unchanged_items_are_removed() {
    let fx = Fx::new();
    let target = dir_with_file(&fx.work.targets_dir().join("t1"));
    let run_dir = fx.run(&[&target]);
    let settings = fx.settings(&[Category::RunTarget, Category::RunTmp]);

    let entries = run_sweep(&settings, true).unwrap();

    assert_eq!(entry(&entries, &target).action, Action::Removed);
    assert!(!target.exists() && !run_dir.join("runtime").exists());
    assert!(
        !run_dir.join(OWNED_PATHS).exists(),
        "an empty owned-paths is removed (P2-1)"
    );
}

/// P1-5: an unreadable `owned-paths` is reported, not treated as empty, and
/// nothing of that run is deleted.
#[test]
fn unreadable_owned_paths_is_reported_as_skipped() {
    let fx = Fx::new();
    let target = dir_with_file(&fx.work.targets_dir().join("t1"));
    let run_dir = fx.run(&[&target]);
    let owned = run_dir.join(OWNED_PATHS);
    let real = fx.root.join("elsewhere-owned-paths");
    fs::rename(&owned, &real).unwrap();
    symlink(&real, &owned).unwrap();
    let settings = fx.settings(&[Category::RunTarget, Category::UntaggedTarget]);

    let entries = run_sweep(&settings, true).unwrap();

    let e = entry(&entries, &owned);
    assert_eq!(e.action, Action::Skipped, "{entries:#?}");
    assert!(e.reason.starts_with("owned-paths unreadable: "), "{e:?}");
    assert!(target.join("keep-me").exists(), "{entries:#?}");
}

/// P1-5: a corrupt `cleanup-record.json` is kept aside, not overwritten.
#[test]
fn corrupt_cleanup_record_is_renamed_aside() {
    let fx = Fx::new();
    let run_dir = fx.run(&[]);
    fs::write(run_dir.join(CLEANUP_RECORD), "{ corrupt").unwrap();

    write_record(&run_dir, "0123abcd-0000-0000-0000-000000000001", &[]).unwrap();

    let aside: Vec<_> = fs::read_dir(&run_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("cleanup-record.json.corrupt-"))
        .collect();
    assert_eq!(aside.len(), 1, "{aside:?}");
    assert_eq!(
        fs::read_to_string(run_dir.join(&aside[0])).unwrap(),
        "{ corrupt"
    );
    let fresh: CleanupRecord =
        serde_json::from_slice(&fs::read(run_dir.join(CLEANUP_RECORD)).unwrap()).unwrap();
    assert_eq!(fresh.sweeps.len(), 1);
}

/// P1-5: a record that exists but cannot be read is an error, not a reset.
#[test]
fn unreadable_cleanup_record_is_an_error() {
    let fx = Fx::new();
    let run_dir = fx.run(&[]);
    let record = run_dir.join(CLEANUP_RECORD);
    symlink(fx.root.join("elsewhere"), &record).unwrap();

    assert!(write_record(&run_dir, "0123abcd-0000-0000-0000-000000000001", &[]).is_err());
    assert!(fs::symlink_metadata(&record).unwrap().is_symlink());
}

/// P1-5: a folder that cannot be listed counts as holding a Cargo lock.
#[test]
fn unlistable_folder_counts_as_cargo_lock_held() {
    if euid() == 0 {
        return;
    }
    let fx = Fx::new();
    let dir = dir_with_file(&fx.root.join("locked"));
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o000)).unwrap();
    let held = cargo_lock_held(&dir);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(held);
}

/// P1-3: the end-of-run sweep only covers run-owned categories.
#[test]
fn end_of_run_categories_are_run_owned_only() {
    assert_eq!(
        END_OF_RUN_CATEGORIES,
        [Category::RunTmp, Category::RunWorktree, Category::RunTarget]
    );
}
