//! Collect/apply integration contracts, using a deterministic mutation between
//! phases rather than a race or a timing-dependent helper process.
//! Safety hold: do not execute before the production guard commit.

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
