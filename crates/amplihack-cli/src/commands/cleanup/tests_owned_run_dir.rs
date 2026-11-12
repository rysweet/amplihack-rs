//! Behavioural red-phase contracts. DO NOT RUN until the complete production
//! authority and fixture isolation have been committed. No case targets a host
//! shared root. Existing lifecycle APIs are used so failures describe behavior,
//! rather than merely the absence of a proposed internal API.

#[path = "../../../tests/common/run_fixture.rs"]
mod run_fixture;

use super::run_dir::{RunScope, RunWorkspace, RuntimeGuard, runtime_dir_for};
use super::run_tmp_root::{RuntimeChoice, TmpRootInputs, choose_runtime_dir, external_dir_name};
use super::work_root::WorkRoot;
use crate::test_support::{EnvGuard, env_lock};
use run_fixture::RunFixture;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

pub(super) struct UnitEnvironment {
    _restore: EnvGuard,
    pub fixture: RunFixture,
    _lock: std::sync::MutexGuard<'static, ()>,
}
impl UnitEnvironment {
    pub fn new() -> Self {
        let lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let fixture = RunFixture::new();
        let values = fixture.environment();
        let keys: Vec<_> = values.iter().map(|(k, _)| *k).collect();
        let restore = EnvGuard::capture(&keys);
        for (key, value) in values {
            // SAFETY: lock is retained until after environment restoration.
            unsafe { std::env::set_var(key, value) };
        }
        Self {
            _restore: restore,
            fixture,
            _lock: lock,
        }
    }
}

pub(super) fn isolated(body: impl FnOnce(&RunFixture)) {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let fx = RunFixture::new();
    let values = fx.environment();
    let keys: Vec<_> = values.iter().map(|(k, _)| *k).collect();
    let _restore = EnvGuard::capture(&keys);
    for (key, value) in values {
        // SAFETY: the crate-wide lock remains held through restoration.
        unsafe { std::env::set_var(key, value) };
    }
    body(&fx);
}

fn begin(fx: &RunFixture, work: &Path, external: bool) -> (RunScope, RuntimeGuard) {
    let inputs = TmpRootInputs {
        run_tmp_root: external.then(|| fx.external.clone().into_os_string()),
        home: Some(fx.home.clone()),
        // SAFETY: geteuid has no preconditions.
        euid: unsafe { libc::geteuid() },
        ..TmpRootInputs::default()
    };
    RunScope::begin_with(
        WorkRoot {
            path: work.to_path_buf(),
            explicit: true,
        },
        "owned-cleanup-contract",
        &fx.base,
        &inputs,
    )
    .unwrap()
}

fn marker(runtime: &Path) -> PathBuf {
    let files: Vec<_> = fs::read_dir(runtime)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| fs::symlink_metadata(p).unwrap().is_file())
        .collect();
    assert_eq!(
        files.len(),
        1,
        "exclusive creation must write one internal ownership record before arming cleanup"
    );
    files[0].clone()
}

fn copy_files(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let destination = to.join(path.file_name().unwrap());
        if path.is_dir() {
            copy_files(&path, &destination);
        } else {
            fs::copy(&path, &destination).unwrap();
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Finish {
    Explicit,
    Drop,
    Unwind,
}

fn finish(guard: RuntimeGuard, how: Finish) {
    match how {
        Finish::Explicit => {
            // Preservation cases assert filesystem state in their caller;
            // dedicated completion tests below assert the returned error.
            let _outcome = guard.remove();
        }
        Finish::Drop => drop(guard),
        Finish::Unwind => {
            let error = std::panic::catch_unwind(move || {
                let _guard = guard;
                panic!("original step failure");
            })
            .expect_err("fixture must unwind");
            assert_eq!(
                error.downcast_ref::<&str>(),
                Some(&"original step failure"),
                "cleanup must not replace the original panic"
            );
        }
    }
}

#[test]
fn temp_root_and_its_contents_survive_every_lifecycle_exit() {
    isolated(|fx| {
        let sentinel = fx.sentinel(&fx.tmp);
        for how in [Finish::Explicit, Finish::Drop, Finish::Unwind] {
            finish(RuntimeGuard::new(fx.tmp.clone()), how);
            fx.assert_sentinel(&sentinel);
        }
    });
}

#[test]
fn an_unmarked_preexisting_directory_is_never_adopted() {
    isolated(|fx| {
        let dir = fx.external.join("preexisting");
        let sentinel = fx.sentinel(&dir);
        finish(RuntimeGuard::new(dir.clone()), Finish::Explicit);
        fx.assert_sentinel(&sentinel);
        assert_eq!(
            fs::read_dir(dir).unwrap().count(),
            1,
            "cleanup must not manufacture ownership evidence"
        );
    });
}

#[test]
fn copied_creation_marker_does_not_authorize_a_different_directory() {
    isolated(|fx| {
        let (_scope, guard) = begin(fx, &fx.work, false);
        let record = marker(guard.path());
        let impostor = fx.external.join("impostor");
        let sentinel = fx.sentinel(&impostor);
        fs::copy(&record, impostor.join(record.file_name().unwrap())).unwrap();
        finish(RuntimeGuard::new(impostor), Finish::Explicit);
        fx.assert_sentinel(&sentinel);
        drop(guard);
    });
}

#[test]
#[cfg(target_os = "linux")]
fn valid_owned_children_are_removed_in_all_three_layouts_and_exit_paths() {
    isolated(|fx| {
        use std::os::fd::AsRawFd;
        let work_fd = fs::File::open(&fx.work).unwrap();
        // Keep allocation inside this fixture even when the launcher's path
        // is long. Linux resolves this short alias to the same private root.
        let short_work = PathBuf::from(format!("/proc/self/fd/{}", work_fd.as_raw_fd()));
        for layout in ["default", "short", "external"] {
            for how in [Finish::Explicit, Finish::Drop, Finish::Unwind] {
                let work = if layout == "default" {
                    short_work.clone()
                } else if layout == "short" {
                    fx.work.join("long".repeat(20))
                } else {
                    fx.work.clone()
                };
                let (scope, guard) = begin(fx, &work, layout == "external");
                let runtime = guard.path().to_path_buf();
                let expected_parent = match layout {
                    "default" => scope.run_dir.clone(),
                    "short" => work.join("r"),
                    _ => fx.external.clone(),
                };
                assert_eq!(
                    runtime.parent(),
                    Some(fs::canonicalize(&expected_parent).unwrap().as_path()),
                    "layout coverage: {layout}"
                );
                let _record = marker(&runtime);
                let sibling =
                    fx.sentinel(&fs::canonicalize(&expected_parent).unwrap().join("sibling"));
                finish(guard, how);
                assert!(
                    !runtime.exists(),
                    "{layout}, {how:?}: an authorized child should be removed"
                );
                fx.assert_sentinel(&sibling);
                assert!(
                    scope.run_dir.join("run.json").is_file(),
                    "durable report survives"
                );
            }
        }
    });
}

#[test]
fn missing_invalid_or_unsafe_marker_preserves_owned_directory() {
    isolated(|fx| {
        for kind in [
            "missing",
            "malformed",
            "unsupported-version",
            "oversized",
            "symlink",
            "hardlink",
            "unreadable",
        ] {
            let (_scope, guard) = begin(fx, &fx.work, false);
            let record = marker(guard.path());
            let sentinel = fx.sentinel(guard.path());
            match kind {
                "missing" => fs::remove_file(&record).unwrap(),
                "malformed" => fs::write(&record, b"{not an ownership record}").unwrap(),
                "unsupported-version" => {
                    let mut value: serde_json::Value =
                        serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
                    value["schema_version"] = u64::MAX.into();
                    fs::write(&record, serde_json::to_vec(&value).unwrap()).unwrap();
                }
                "oversized" => fs::write(&record, vec![b' '; 2 * 1024 * 1024]).unwrap(),
                "symlink" => {
                    let saved = fx.evidence.join("marker-target");
                    fs::rename(&record, &saved).unwrap();
                    symlink(saved, &record).unwrap();
                }
                "hardlink" => fs::hard_link(&record, fx.evidence.join("marker-hardlink")).unwrap(),
                "unreadable" => {
                    fs::set_permissions(&record, fs::Permissions::from_mode(0o0)).unwrap()
                }
                _ => unreachable!(),
            }
            finish(guard, Finish::Explicit);
            fx.assert_sentinel(&sentinel);
        }
    });
}

#[test]
#[cfg(target_os = "linux")]
fn removing_an_owned_tree_never_follows_an_interior_symlink() {
    isolated(|fx| {
        let (_scope, guard) = begin(fx, &fx.work, false);
        let runtime = guard.path().to_path_buf();
        let sibling = fx.base.join("outside-tree");
        let sentinel = fx.sentinel(&sibling);
        symlink(&sibling, runtime.join("tmp/link-outside")).unwrap();
        finish(guard, Finish::Explicit);
        assert!(!runtime.exists());
        fx.assert_sentinel(&sentinel);
    });
}

#[test]
fn planned_external_prefix_accepts_59_bytes_and_refuses_60_without_escaping_fixture() {
    isolated(|fx| {
        let id = "3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3";
        let length = fx.base.as_os_str().len();
        assert!(
            length < 58,
            "provide a shorter private outer TMPDIR for exact path-boundary coverage"
        );
        for bytes in [59, 60] {
            let root = fx.base.join("x".repeat(bytes - length - 1));
            fs::create_dir(&root).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            assert_eq!(root.as_os_str().len(), bytes);
            let chosen = choose_runtime_dir(
                &fx.work,
                id,
                &TmpRootInputs {
                    run_tmp_root: Some(root.clone().into_os_string()),
                    home: Some(fx.home.clone()),
                    // SAFETY: geteuid has no preconditions.
                    euid: unsafe { libc::geteuid() },
                    ..TmpRootInputs::default()
                },
            );
            match (bytes, chosen.choice) {
                (
                    59,
                    RuntimeChoice::External {
                        dir,
                        root: selected,
                    },
                ) => {
                    assert_eq!(selected, root);
                    assert_eq!(dir.file_name().unwrap(), "ahr-3f0c9a526d1e");
                    assert_eq!(dir.join("tmp").as_os_str().len(), 80);
                    // SAFETY: isolated() holds the environment lock and restores this key.
                    unsafe { std::env::set_var("AMPLIHACK_RUN_TMP_ROOT", &root) };
                    let dry = RunWorkspace::dry_runtime().unwrap();
                    assert_eq!(dry.runtime_dir().parent(), Some(root.as_path()));
                    assert!(dry.runtime_dir().join("tmp").as_os_str().len() <= 80);
                    drop(dry);
                }
                (60, RuntimeChoice::Default(_)) => {
                    assert!(chosen.warning.is_some());
                    assert_eq!(fs::read_dir(root).unwrap().count(), 0);
                }
                (_, choice) => panic!("wrong choice for {bytes}-byte root: {choice:?}"),
            }
        }
    });
}

#[test]
fn two_individually_valid_records_must_describe_the_same_creation() {
    isolated(|fx| {
        let (_first, first_guard) = begin(fx, &fx.work, false);
        let (_second, second_guard) = begin(fx, &fx.work, false);
        let first_marker = marker(first_guard.path());
        let second_marker = marker(second_guard.path());
        let sentinel = fx.sentinel(first_guard.path());
        fs::copy(second_marker, first_marker).unwrap();
        finish(first_guard, Finish::Explicit);
        fx.assert_sentinel(&sentinel);
        drop(second_guard);
    });
}

#[test]
#[cfg(target_os = "linux")]
fn missing_external_record_cannot_be_repaired_from_an_internal_marker() {
    isolated(|fx| {
        let (_scope, guard) = begin(fx, &fx.work, true);
        let internal = marker(guard.path());
        // The external root contains only this run and authority metadata.
        // Remove record files outside the candidate without relying on their
        // private names or serialization. Never recursively delete metadata.
        fn remove_external_files(dir: &Path, candidate: &Path) -> usize {
            let mut count = 0;
            for entry in fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                if entry.path() == candidate {
                    continue;
                }
                if entry.file_type().unwrap().is_dir() {
                    count += remove_external_files(&entry.path(), candidate);
                } else if entry.file_type().unwrap().is_file() {
                    fs::remove_file(entry.path()).unwrap();
                    count += 1;
                }
            }
            count
        }
        assert!(
            remove_external_files(&fx.external, guard.path()) > 0,
            "creation needs independently retained external evidence"
        );
        let sentinel = fx.sentinel(guard.path());
        let bytes = fs::read(&internal).unwrap();
        finish(guard, Finish::Explicit);
        fx.assert_sentinel(&sentinel);
        assert_eq!(
            fs::read(internal).unwrap(),
            bytes,
            "cleanup cannot rewrite evidence to adopt a directory"
        );
    });
}

#[test]
fn valid_ownership_cannot_override_a_new_tmpdir_or_home_protection() {
    isolated(|fx| {
        for key in ["TMPDIR", "HOME"] {
            let (_scope, guard) = begin(fx, &fx.work, false);
            let sentinel = fx.sentinel(guard.path());
            let _restore = EnvGuard::capture(&[key]);
            // SAFETY: isolated() owns the shared lock through this scope.
            unsafe { std::env::set_var(key, guard.path()) };
            finish(guard, Finish::Explicit);
            fx.assert_sentinel(&sentinel);
        }
    });
}

#[test]
fn protected_root_alias_is_rejected_even_with_valid_creation_evidence() {
    isolated(|fx| {
        let (_scope, guard) = begin(fx, &fx.work, false);
        let sentinel = fx.sentinel(guard.path());
        let alias = fx.base.join("temp-alias");
        symlink(guard.path(), &alias).unwrap();
        // SAFETY: isolated() owns the shared environment lock.
        unsafe { std::env::set_var("TMPDIR", alias.join(".")) };
        finish(guard, Finish::Explicit);
        fx.assert_sentinel(&sentinel);
    });
}

#[test]
fn wrong_parent_is_not_approved_by_recomputing_root_from_candidate() {
    isolated(|fx| {
        let (_scope, guard) = begin(fx, &fx.work, false);
        let moved = fx.external.join("relocated");
        fs::rename(guard.path(), &moved).unwrap();
        let sentinel = fx.sentinel(&moved);
        // The unchanged creation evidence belongs to its original independent root.
        finish(RuntimeGuard::new(moved), Finish::Explicit);
        fx.assert_sentinel(&sentinel);
        drop(guard);
    });
}

#[test]
fn candidate_replacement_does_not_inherit_ownership_from_copied_records() {
    isolated(|fx| {
        let (_scope, guard) = begin(fx, &fx.work, false);
        let path = guard.path().to_path_buf();
        let original = fx.base.join("original");
        fs::rename(&path, &original).unwrap();
        copy_files(&original, &path);
        let replacement = fx.sentinel(&path);
        let original_sentinel = fx.sentinel(&original);
        finish(guard, Finish::Explicit);
        fx.assert_sentinel(&replacement);
        fx.assert_sentinel(&original_sentinel);
    });
}

#[test]
fn symlink_replacement_preserves_the_link_and_its_destination() {
    isolated(|fx| {
        let (_scope, guard) = begin(fx, &fx.work, false);
        let path = guard.path().to_path_buf();
        fs::rename(&path, fx.base.join("original")).unwrap();
        let target = fx.base.join("unrelated");
        let sentinel = fx.sentinel(&target);
        symlink(&target, &path).unwrap();
        finish(guard, Finish::Explicit);
        assert!(fs::symlink_metadata(path).unwrap().file_type().is_symlink());
        fx.assert_sentinel(&sentinel);
    });
}

#[test]
fn root_replacement_preserves_a_copied_candidate_at_the_same_path() {
    isolated(|fx| {
        let (_scope, guard) = begin(fx, &fx.work, true);
        assert_eq!(guard.path().parent(), Some(fx.external.as_path()));
        let path = guard.path().to_path_buf();
        let saved = fx.base.join("saved-root");
        fs::rename(&fx.external, &saved).unwrap();
        copy_files(&saved, &fx.external);
        let sentinel = fx.sentinel(&path);
        finish(guard, Finish::Explicit);
        fx.assert_sentinel(&sentinel);
    });
}

#[test]
fn dry_run_drop_cannot_bypass_a_protected_root_refusal() {
    isolated(|fx| {
        let workspace = RunWorkspace::dry_runtime().unwrap();
        let runtime = workspace.runtime_dir().to_path_buf();
        let sentinel = fx.sentinel(&runtime);
        // SAFETY: isolated() owns the shared environment lock.
        unsafe { std::env::set_var("TMPDIR", &runtime) };
        drop(workspace);
        fx.assert_sentinel(&sentinel);
    });
}

#[test]
fn chooser_does_not_adopt_or_mark_an_existing_leaf() {
    isolated(|fx| {
        let id = "3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3";
        let leaf = fx.external.join(external_dir_name(id).unwrap());
        let sentinel = fx.sentinel(&leaf);
        let chosen = choose_runtime_dir(
            &fx.work,
            id,
            &TmpRootInputs {
                run_tmp_root: Some(fx.external.clone().into_os_string()),
                home: Some(fx.home.clone()),
                // SAFETY: geteuid has no preconditions.
                euid: unsafe { libc::geteuid() },
                ..TmpRootInputs::default()
            },
        );
        assert!(
            matches!(chosen.choice, RuntimeChoice::Default(ref p) if *p == runtime_dir_for(&fx.work, id))
        );
        fx.assert_sentinel(&sentinel);
        assert_eq!(
            fs::read_dir(leaf).unwrap().count(),
            1,
            "exclusive creation never grants ownership to an existing directory"
        );
    });
}

// Red-phase interface contracts: explicit completion must return a useful
// error instead of only logging a failed authorization. The current unit-return
// signatures deliberately do not satisfy these tests. Execution remains held.
#[test]
fn explicit_guard_completion_returns_a_protected_path_refusal() {
    isolated(|fx| {
        let (_scope, guard) = begin(fx, &fx.work, false);
        let runtime = guard.path().to_owned();
        let sentinel = fx.sentinel(&runtime);
        // SAFETY: isolated holds the shared environment lock through restoration.
        unsafe { std::env::set_var("TMPDIR", &runtime) };
        let error = guard
            .remove()
            .expect_err("completion must expose a cleanup refusal");
        let diagnostic = format!("{error:#}");
        assert!(diagnostic.contains(&runtime.display().to_string()));
        assert!(
            diagnostic.len() > runtime.as_os_str().len(),
            "caller needs the preservation reason as well as the path"
        );
        fx.assert_sentinel(&sentinel);
    });
}

#[test]
fn workspace_completion_propagates_cleanup_failure_and_finalizes_the_report() {
    isolated(|fx| {
        let (scope, guard) = begin(fx, &fx.work, false);
        let report_path = scope.run_dir.join("run.json");
        let record = marker(guard.path());
        let sentinel = fx.sentinel(guard.path());
        fs::remove_file(record).unwrap();
        let workspace = RunWorkspace::Scoped {
            scope: Box::new(scope),
            guard,
        };
        let error = workspace
            .finish(super::run_dir::Outcome::Success, vec![], None, false)
            .expect_err("workspace caller must observe failure to authorize cleanup");
        assert!(!error.to_string().is_empty());
        fx.assert_sentinel(&sentinel);
        let report: serde_json::Value =
            serde_json::from_slice(&fs::read(report_path).unwrap()).unwrap();
        assert!(
            report["ended_at"].is_string(),
            "cleanup refusal must not lose the run report"
        );
        assert_eq!(report["outcome"], "success");
    });
}

#[test]
#[cfg(not(target_os = "linux"))]
fn unavailable_platform_guarantees_preserve_the_owned_directory_and_report_why() {
    use super::owned_run_dir::{CleanupOutcome, OwnedRunDir};
    isolated(|fx| {
        let owned = OwnedRunDir::create(&fx.external, "owned", "run").unwrap();
        let sentinel = fx.sentinel(owned.path());
        match owned.remove() {
            CleanupOutcome::Unsupported(reason) => assert!(!reason.is_empty()),
            other => panic!("unsupported platform must preserve and explain: {other:?}"),
        }
        fx.assert_sentinel(&sentinel);
        assert!(OwnedRunDir::load(&fx.external, owned.path(), "run").is_err());
    });
}

#[test]
fn protected_descendants_survive_removal_of_their_owned_ancestor() {
    isolated(|fx| {
        for key in ["TMPDIR", "HOME"] {
            let (_scope, guard) = begin(fx, &fx.work, false);
            let nested = guard.path().join("tmp/protected");
            let sentinel = fx.sentinel(&nested);
            let _restore = EnvGuard::capture(&[key]);
            unsafe { std::env::set_var(key, &nested) };
            assert!(guard.remove().is_err());
            fx.assert_sentinel(&sentinel);
        }
    });
}
