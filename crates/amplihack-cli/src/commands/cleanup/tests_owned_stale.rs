//! Collect/apply integration contracts, using a deterministic mutation between
//! phases rather than a race or a timing-dependent helper process.
//! Safety hold: do not execute before the production guard commit.

#![cfg(target_os = "linux")]

use super::super::run_dir::RunScope;
use super::super::run_tmp_root::TmpRootInputs;
use super::super::tests_owned_run_dir::isolated;
use super::*;

fn settings(work: &WorkRoot) -> Settings {
    Settings {
        work_root: work.clone(),
        keep: Duration::ZERO,
        log_age: Duration::MAX,
        archive_dir: work.path.join("archive"),
        categories: [Category::RunTmp].into_iter().collect(),
        apply: true,
        report_unrecorded: false,
        deadline: None,
        recipe_log_dir: None,
        codex_sessions_dir: None,
        claude_projects_dir: None,
        hook_cache_base: None,
    }
}

fn mark_dead(run: &Path) {
    let path = run.join("run.json");
    let mut report: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    report["pid"] = 0.into();
    report["pid_start_ticks"] = serde_json::Value::Null;
    report["ended_at"] = "2000-01-01T00:00:00Z".into();
    report["outcome"] = "success".into();
    fs::write(path, serde_json::to_vec(&report).unwrap()).unwrap();
}

/// Exercise stale authorization independently of the chooser's socket-path
/// length limit. Even a long private launcher path must reach the evidence
/// checks here; chooser boundary coverage lives in tests_owned_run_dir.rs.
fn external_run(
    work: &WorkRoot,
    base: &Path,
    external: &Path,
) -> (RunScope, super::super::owned_run_dir::OwnedRunDir) {
    let (scope, guard) = RunScope::begin_with(
        work.clone(),
        "stale-external-contract",
        base,
        &TmpRootInputs::default(),
    )
    .unwrap();
    drop(guard);
    let leaf = super::super::run_tmp_root::external_dir_name(&scope.run_id).unwrap();
    let owned =
        super::super::owned_run_dir::OwnedRunDir::create(external, &leaf, &scope.run_id).unwrap();
    let report_path = scope.run_dir.join("run.json");
    let mut report: serde_json::Value =
        serde_json::from_slice(&fs::read(&report_path).unwrap()).unwrap();
    report["runtime_dir"] = owned.path().to_string_lossy().to_string().into();
    report["runtime_root"] = external.to_string_lossy().to_string().into();
    fs::write(report_path, serde_json::to_vec(&report).unwrap()).unwrap();
    (scope, owned)
}

#[test]
fn stale_apply_preserves_a_replacement_after_collection() {
    isolated(|fx| {
        let work = WorkRoot {
            path: fx.work.clone(),
            explicit: true,
        };
        let (scope, guard) = RunScope::begin_with(
            work.clone(),
            "stale-contract",
            &fx.base,
            &TmpRootInputs::default(),
        )
        .unwrap();
        mark_dead(&scope.run_dir);
        let runtime = guard.path().to_path_buf();
        let settings = settings(&work);
        let items = collect(&settings).unwrap();
        assert!(
            items
                .iter()
                .any(|item| item.path == runtime && matches!(item.op, Op::RemoveRunTmp { .. })),
            "must collect an eligible real creation before replacement"
        );
        let original = fx.base.join("original-runtime");
        fs::rename(&runtime, &original).unwrap();
        fs::create_dir(&runtime).unwrap();
        // Copy all regular marker files: path and record shape cannot grant
        // the replacement the identity of the original directory.
        for entry in fs::read_dir(&original).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_file() {
                fs::copy(entry.path(), runtime.join(entry.file_name())).unwrap();
            }
        }
        let sentinel = fx.sentinel(&runtime);
        let entries = apply_items(items, &settings, true);
        let entry = entries
            .iter()
            .find(|entry| entry.path == runtime)
            .expect("replacement must be reported");
        assert_eq!(entry.action, Action::Skipped, "{entries:?}");
        fx.assert_sentinel(&sentinel);
        assert!(original.is_dir());
        drop(guard);
        fx.assert_sentinel(&sentinel);
    });
}

#[test]
fn stale_apply_rechecks_a_new_protected_temp_root() {
    isolated(|fx| {
        let work = WorkRoot {
            path: fx.work.clone(),
            explicit: true,
        };
        let (scope, guard) = RunScope::begin_with(
            work.clone(),
            "stale-contract",
            &fx.base,
            &TmpRootInputs::default(),
        )
        .unwrap();
        mark_dead(&scope.run_dir);
        let runtime = guard.path().to_path_buf();
        let sentinel = fx.sentinel(&runtime);
        let settings = settings(&work);
        let items = collect(&settings).unwrap();
        assert!(
            items
                .iter()
                .any(|item| item.path == runtime && matches!(item.op, Op::RemoveRunTmp { .. }))
        );
        // SAFETY: isolated() retains the crate-wide lock and restores TMPDIR.
        unsafe { std::env::set_var("TMPDIR", &runtime) };
        let entries = apply_items(items, &settings, true);
        let entry = entries
            .iter()
            .find(|entry| entry.path == runtime)
            .expect("protected candidate must be reported");
        assert_eq!(entry.action, Action::Skipped, "{entries:?}");
        fx.assert_sentinel(&sentinel);
        drop(guard);
        fx.assert_sentinel(&sentinel);
    });
}

#[test]
#[cfg(target_os = "linux")]
fn stale_apply_removes_owned_child_but_preserves_unmarked_sibling() {
    isolated(|fx| {
        let work = WorkRoot {
            path: fx.work.clone(),
            explicit: true,
        };
        let (scope, guard) = RunScope::begin_with(
            work.clone(),
            "stale-contract",
            &fx.base,
            &TmpRootInputs::default(),
        )
        .unwrap();
        mark_dead(&scope.run_dir);
        let runtime = guard.path().to_path_buf();
        let sibling = fx.sentinel(&runtime.parent().unwrap().join("sibling"));
        let settings = settings(&work);
        let entries = apply_items(collect(&settings).unwrap(), &settings, true);
        let entry = entries
            .iter()
            .find(|entry| entry.path == runtime)
            .expect("owned candidate must be reported");
        assert_eq!(entry.action, Action::Removed, "{entries:?}");
        assert!(!runtime.exists());
        fx.assert_sentinel(&sibling);
        assert!(scope.run_dir.join("run.json").is_file());
        drop(guard);
    });
}

#[test]
fn stale_apply_rereads_both_ownership_records_instead_of_trusting_discovery() {
    isolated(|fx| {
        for evidence in ["internal", "external"] {
            let work = WorkRoot {
                path: fx.work.clone(),
                explicit: true,
            };
            let (scope, guard) = RunScope::begin_with(
                work.clone(),
                "stale-evidence-contract",
                &fx.base,
                &TmpRootInputs::default(),
            )
            .unwrap();
            mark_dead(&scope.run_dir);
            let runtime = guard.path().to_owned();
            let sentinel = fx.sentinel(&runtime);
            let settings = settings(&work);
            let items = collect(&settings).unwrap();
            assert!(items.iter().any(|item| {
                item.path == runtime && matches!(item.op, Op::RemoveRunTmp { .. })
            }));
            let record = if evidence == "internal" {
                runtime.join(".amplihack-owner.json")
            } else {
                runtime
                    .parent()
                    .unwrap()
                    .join(".amplihack-ownership")
                    .join(format!(
                        "{}.json",
                        runtime.file_name().unwrap().to_str().unwrap()
                    ))
            };
            fs::write(&record, b"invalidated after discovery").unwrap();
            let entries = apply_items(items, &settings, true);
            let entry = entries.iter().find(|entry| entry.path == runtime).unwrap();
            assert_eq!(entry.action, Action::Skipped, "{evidence}: {entries:?}");
            fx.assert_sentinel(&sentinel);
            assert_eq!(fs::read(record).unwrap(), b"invalidated after discovery");
            drop(guard);
            fx.assert_sentinel(&sentinel);
        }
    });
}

#[test]
fn stale_apply_rechecks_the_independently_recorded_external_parent() {
    isolated(|fx| {
        let work = WorkRoot {
            path: fx.work.clone(),
            explicit: true,
        };
        let (scope, guard) = external_run(&work, &fx.base, &fx.external);
        let runtime = guard.path().to_owned();
        assert_eq!(runtime.parent(), Some(fx.external.as_path()));
        mark_dead(&scope.run_dir);
        let sentinel = fx.sentinel(&runtime);
        let settings = settings(&work);
        let items = collect(&settings).unwrap();
        assert!(
            items
                .iter()
                .any(|item| { item.path == runtime && matches!(item.op, Op::RemoveRunTmp { .. }) })
        );
        let report_path = scope.run_dir.join("run.json");
        let mut report: serde_json::Value =
            serde_json::from_slice(&fs::read(&report_path).unwrap()).unwrap();
        report["runtime_root"] = fx.evidence.to_string_lossy().to_string().into();
        fs::write(report_path, serde_json::to_vec(&report).unwrap()).unwrap();
        let entries = apply_items(items, &settings, true);
        let entry = entries.iter().find(|entry| entry.path == runtime).unwrap();
        assert_eq!(entry.action, Action::Skipped, "{entries:?}");
        fx.assert_sentinel(&sentinel);
        drop(guard);
        fx.assert_sentinel(&sentinel);
    });
}

#[test]
fn stale_collection_does_not_adopt_valid_records_moved_to_another_parent() {
    isolated(|fx| {
        let work = WorkRoot {
            path: fx.work.clone(),
            explicit: true,
        };
        let (scope, guard) = external_run(&work, &fx.base, &fx.external);
        let original = guard.path().to_owned();
        assert_eq!(original.parent(), Some(fx.external.as_path()));
        let moved = fx.evidence.join(original.file_name().unwrap());
        fs::rename(&original, &moved).unwrap();
        fs::rename(
            fx.external.join(".amplihack-ownership"),
            fx.evidence.join(".amplihack-ownership"),
        )
        .unwrap();
        let sentinel = fx.sentinel(&moved);
        mark_dead(&scope.run_dir);
        let report_path = scope.run_dir.join("run.json");
        let mut report: serde_json::Value =
            serde_json::from_slice(&fs::read(&report_path).unwrap()).unwrap();
        // Only the candidate changes. Its independently recorded parent does
        // not, even though both matching creation records moved with it.
        report["runtime_dir"] = moved.to_string_lossy().to_string().into();
        fs::write(report_path, serde_json::to_vec(&report).unwrap()).unwrap();
        let settings = settings(&work);
        let entries = apply_items(collect(&settings).unwrap(), &settings, true);
        let entry = entries.iter().find(|entry| entry.path == moved).unwrap();
        assert_eq!(entry.action, Action::Skipped, "{entries:?}");
        fx.assert_sentinel(&sentinel);
        drop(guard);
        fx.assert_sentinel(&sentinel);
    });
}

#[test]
fn interrupted_claim_in_use_is_preserved_at_discovery_and_at_apply() {
    use super::super::owned_run_dir::OwnedRunDir;
    use std::os::unix::fs::symlink;
    isolated(|fx| {
        let work = WorkRoot {
            path: fx.work.clone(),
            explicit: true,
        };
        let (scope, guard) = RunScope::begin_with(
            work.clone(),
            "busy-claim",
            &fx.base,
            &TmpRootInputs::default(),
        )
        .unwrap();
        let runtime = guard.path().to_owned();
        drop(guard);
        let parent = if runtime == scope.run_dir.join("runtime") {
            scope.run_dir.clone()
        } else {
            work.short_runtime_dir()
        };
        let owned = OwnedRunDir::create(
            &parent,
            runtime.file_name().unwrap().to_str().unwrap(),
            &scope.run_id,
        )
        .unwrap();
        fx.sentinel(owned.path());
        let record: serde_json::Value =
            serde_json::from_slice(&fs::read(runtime.join(".amplihack-owner.json")).unwrap())
                .unwrap();
        let claim = parent.join(format!(
            ".amplihack-claim-{}",
            record["nonce"].as_str().unwrap()
        ));
        fs::rename(&runtime, &claim).unwrap();
        drop(owned);
        mark_dead(&scope.run_dir);
        let settings = settings(&work);
        let items = collect(&settings).unwrap();
        assert!(
            items
                .iter()
                .any(|item| item.path == runtime && matches!(item.op, Op::RemoveRunTmp { .. }))
        );
        let process = fx.proc_root.join("999999");
        fs::create_dir(&process).unwrap();
        symlink(&claim, process.join("cwd")).unwrap();
        let entries = apply_items(items, &settings, true);
        assert!(
            entries
                .iter()
                .any(|entry| entry.path == runtime && entry.action == Action::Skipped)
        );
        fx.assert_sentinel(&claim.join("sentinel"));
        let entries = run_sweep(&settings, true).unwrap();
        assert!(
            entries
                .iter()
                .any(|entry| entry.path == runtime && entry.action == Action::Skipped)
        );
        fx.assert_sentinel(&claim.join("sentinel"));
        fs::remove_file(process.join("cwd")).unwrap();
        let entries = run_sweep(&settings, true).unwrap();
        assert!(
            entries
                .iter()
                .any(|entry| entry.path == runtime && entry.action == Action::Removed)
        );
        assert!(!claim.exists());
    });
}
