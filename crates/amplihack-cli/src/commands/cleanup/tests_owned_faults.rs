//! Deterministic filesystem failure and replacement contracts. All mutation is
//! confined to private fixtures; no test mounts or touches a shared root.
use super::super::tests_owned_run_dir::isolated;
use super::*;

struct Hook;
impl Hook {
    fn install(hook: impl FnMut(&str, &Path) -> Result<()> + 'static) -> Self {
        TEST_HOOK.with(|h| *h.borrow_mut() = Some(Box::new(hook)));
        Self
    }
}
impl Drop for Hook {
    fn drop(&mut self) {
        TEST_HOOK.with(|h| h.borrow_mut().take());
    }
}
fn preserved(result: CleanupOutcome) {
    assert!(matches!(result, CleanupOutcome::Preserved(_)), "{result:?}");
}

#[test]
fn incomplete_initialization_never_arms_or_repairs_ownership() {
    isolated(|fx| {
        let hook = Hook::install(|event, _| {
            ensure!(
                event != "after_internal_record",
                "injected external record write failure"
            );
            Ok(())
        });
        assert!(OwnedRunDir::create(&fx.external, "partial", "run").is_err());
        drop(hook);
        let path = fx.external.join("partial");
        let sentinel = fx.sentinel(&path);
        assert!(path.join(MARKER).is_file());
        assert!(OwnedRunDir::load(&fx.external, &path, "run").is_err());
        assert!(OwnedRunDir::create(&fx.external, "partial", "run").is_err());
        fx.assert_sentinel(&sentinel);
    });
}

#[test]
fn candidate_moved_after_claim_survives() {
    isolated(|fx| {
        let owned = OwnedRunDir::create(&fx.external, "run", "run").unwrap();
        fx.sentinel(owned.path());
        let moved = fx.base.join("moved");
        let destination = moved.clone();
        let hook = Hook::install(move |event, path| {
            if event == "after_claim" {
                fs::rename(path, &destination)?;
            }
            Ok(())
        });
        preserved(owned.remove());
        drop(hook);
        fx.assert_sentinel(&moved.join("sentinel"));
    });
}

#[test]
fn moved_claim_is_rechecked_immediately_before_unlink() {
    isolated(|fx| {
        let owned = OwnedRunDir::create(&fx.external, "run", "run").unwrap();
        fx.sentinel(owned.path());
        let moved = fx.base.join("moved");
        let destination = moved.clone();
        let mut moved_once = false;
        let hook = Hook::install(move |event, path| {
            if event == "before_unlink" && !moved_once {
                moved_once = true;
                fs::rename(path, &destination)?;
            }
            Ok(())
        });
        assert!(matches!(owned.remove(), CleanupOutcome::Failed(_)));
        drop(hook);
        fx.assert_sentinel(&moved.join("sentinel"));
    });
}

#[test]
fn interior_directory_replacement_preserves_both_trees() {
    isolated(|fx| {
        let owned = OwnedRunDir::create(&fx.external, "run", "run").unwrap();
        let child = owned.path().join("nested");
        fx.sentinel(&child);
        let saved = fx.base.join("saved-child");
        let destination = saved.clone();
        let mut moved = false;
        let hook = Hook::install(move |event, path| {
            if event == "before_unlink" && path.file_name().is_some_and(|n| n == "nested") && !moved
            {
                moved = true;
                fs::rename(path, &destination)?;
                fs::create_dir(path)?;
                fs::write(path.join("replacement"), b"keep")?;
            }
            Ok(())
        });
        assert!(matches!(owned.remove(), CleanupOutcome::Failed(_)));
        drop(hook);
        fx.assert_sentinel(&saved.join("sentinel"));
        assert_eq!(fs::read(child.join("replacement")).unwrap(), b"keep");
    });
}

#[test]
fn failed_claim_restoration_never_overwrites_an_existing_leaf() {
    isolated(|fx| {
        let owned = OwnedRunDir::create(&fx.external, "run", "run").unwrap();
        fx.sentinel(owned.path());
        let original = owned.path().to_owned();
        let hook = Hook::install(move |event, _| {
            if event == "after_claim" {
                fs::create_dir(&original)?;
                fs::write(original.join("replacement"), b"keep")?;
                bail!("injected validation failure after claim");
            }
            Ok(())
        });
        preserved(owned.remove());
        drop(hook);
        assert_eq!(fs::read(owned.path().join("replacement")).unwrap(), b"keep");
        let claims: Vec<_> = fs::read_dir(&fx.external)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".amplihack-claim-")
            })
            .collect();
        assert_eq!(claims.len(), 1);
        fx.assert_sentinel(&claims[0].join("sentinel"));
    });
}

#[test]
fn mount_boundary_validation_never_traverses_a_different_mount() {
    isolated(|fx| {
        let owned = OwnedRunDir::create(&fx.external, "run", "run").unwrap();
        let sentinel = fx.sentinel(owned.path());
        // Inject an expected mount other than this real descriptor's mount;
        // exercises the same check used for same-device bind mounts.
        assert!(
            sys::walk(
                &owned.candidate,
                owned.record.candidate.mount + 1,
                true,
                false,
                &|| Ok(())
            )
            .is_err()
        );
        fx.assert_sentinel(&sentinel);
    });
}

#[test]
fn independently_supplied_wrong_parent_refuses_valid_creation_records() {
    isolated(|fx| {
        let owned = OwnedRunDir::create(&fx.external, "run", "run").unwrap();
        let sentinel = fx.sentinel(owned.path());
        assert!(OwnedRunDir::load(&fx.work, owned.path(), "run").is_err());
        // A retained authority whose target spelling is changed also fails.
        let mut changed = owned.clone();
        changed.path = fx.work.join("run");
        preserved(changed.remove());
        fx.assert_sentinel(&sentinel);
    });
}

#[test]
fn missing_or_corrupt_marker_immediately_before_removal_preserves_contents() {
    isolated(|fx| {
        for (leaf, corrupt) in [("missing", false), ("corrupt", true)] {
            let owned = OwnedRunDir::create(&fx.external, leaf, "run").unwrap();
            let sentinel = fx.sentinel(owned.path());
            let mut changed = false;
            let hook = Hook::install(move |event, path| {
                if event == "before_unlink" && !changed {
                    changed = true;
                    if corrupt {
                        fs::write(path.join(MARKER), b"invalid")?;
                    } else {
                        fs::remove_file(path.join(MARKER))?;
                    }
                }
                Ok(())
            });
            assert!(matches!(owned.remove(), CleanupOutcome::Failed(_)));
            drop(hook);
            fx.assert_sentinel(&sentinel);
        }
    });
}

#[test]
fn configured_temp_root_allows_only_its_owned_child_to_be_removed() {
    isolated(|fx| {
        let root_sentinel = fx.sentinel(&fx.tmp);
        let sibling = fx.sentinel(&fx.tmp.join("sibling"));
        let owned = OwnedRunDir::create(&fx.tmp, "owned", "run").unwrap();
        assert!(matches!(owned.remove(), CleanupOutcome::Removed));
        assert!(!owned.path().exists());
        fx.assert_sentinel(&root_sentinel);
        fx.assert_sentinel(&sibling);
    });
}

#[test]
fn changing_environment_does_not_discard_initial_protected_observations() {
    isolated(|fx| {
        let owned = OwnedRunDir::create(&fx.external, "owned", "run").unwrap();
        let sentinel = fx.sentinel(owned.path());
        // The original TMPDIR observation must still resolve, even after both
        // current environment-derived protections move to existing new paths.
        fs::rename(&fx.tmp, fx.base.join("old-temp")).unwrap();
        let next = fx.base.join("next-temp");
        fs::create_dir(&next).unwrap();
        // SAFETY: isolated retains the shared lock and restores these values.
        unsafe {
            std::env::set_var("TMPDIR", &next);
            std::env::set_var("HOME", &next);
        }
        preserved(owned.remove());
        fx.assert_sentinel(&sentinel);
    });
}

#[test]
fn invalid_external_evidence_is_never_reconstructed_from_internal_marker() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    isolated(|fx| {
        for kind in ["missing", "malformed", "symlink", "hardlink", "unreadable"] {
            let owned = OwnedRunDir::create(&fx.external, kind, "run").unwrap();
            let sentinel = fx.sentinel(owned.path());
            let record = fx.external.join(META).join(record_name(kind));
            match kind {
                "missing" => fs::remove_file(&record).unwrap(),
                "malformed" => fs::write(&record, b"invalid").unwrap(),
                "symlink" => {
                    let saved = fx.evidence.join("external-record");
                    fs::rename(&record, &saved).unwrap();
                    symlink(saved, &record).unwrap();
                }
                "hardlink" => {
                    fs::hard_link(&record, fx.evidence.join("external-hardlink")).unwrap()
                }
                "unreadable" => {
                    fs::set_permissions(&record, fs::Permissions::from_mode(0)).unwrap()
                }
                _ => unreachable!(),
            }
            preserved(owned.remove());
            fx.assert_sentinel(&sentinel);
        }
    });
}

#[test]
fn root_replaced_immediately_before_unlink_preserves_original_and_replacement() {
    isolated(|fx| {
        let owned = OwnedRunDir::create(&fx.external, "owned", "run").unwrap();
        fx.sentinel(owned.path());
        let root = fx.external.clone();
        let saved = fx.base.join("saved-root");
        let destination = saved.clone();
        let mut moved = false;
        let hook = Hook::install(move |event, _| {
            if event == "before_unlink" && !moved {
                moved = true;
                fs::rename(&root, &destination)?;
                fs::create_dir(&root)?;
                fs::write(root.join("replacement"), b"keep")?;
            }
            Ok(())
        });
        assert!(matches!(owned.remove(), CleanupOutcome::Failed(_)));
        drop(hook);
        fx.assert_sentinel(&saved.join("owned/sentinel"));
        assert_eq!(fs::read(fx.external.join("replacement")).unwrap(), b"keep");
    });
}

// Commit the complete guard and fixture isolation before executing these
// distinct-outcome and interrupted-claim contracts.

#[test]
fn already_removed_owned_directory_is_a_distinct_successful_noop() {
    isolated(|fx| {
        let owned = OwnedRunDir::create(&fx.external, "owned", "run").unwrap();
        let sibling = fx.sentinel(&fx.external.join("sibling"));
        assert!(matches!(owned.remove(), CleanupOutcome::Removed));
        assert!(matches!(owned.remove(), CleanupOutcome::AlreadyAbsent));
        fx.assert_sentinel(&sibling);

        // Absence of the original identity cannot authorize a new directory
        // subsequently installed at the same leaf.
        let replacement = fx.sentinel(owned.path());
        preserved(owned.remove());
        fx.assert_sentinel(&replacement);
    });
}

#[test]
fn unsupported_filesystem_guarantees_are_distinct_and_leave_retry_evidence() {
    isolated(|fx| {
        for primitive in ["identity", "rename"] {
            let owned = OwnedRunDir::create(&fx.external, primitive, "run").unwrap();
            let sentinel = fx.sentinel(owned.path());
            let record_path = fx.external.join(META).join(record_name(primitive));
            let before = fs::read(&record_path).unwrap();
            let hook = Hook::install(move |event, _| {
                if event == primitive {
                    return Err(std::io::Error::from_raw_os_error(libc::ENOSYS).into());
                }
                Ok(())
            });
            let outcome = owned.remove();
            drop(hook);
            match outcome {
                CleanupOutcome::Unsupported(reason) => assert!(!reason.is_empty()),
                other => {
                    panic!("missing required primitive must be reported distinctly: {other:?}")
                }
            }
            fx.assert_sentinel(&sentinel);
            assert_eq!(fs::read(&record_path).unwrap(), before);
            let reloaded = OwnedRunDir::load(&fx.external, owned.path(), "run").unwrap();
            assert!(matches!(reloaded.remove(), CleanupOutcome::Removed));
        }
    });
}

#[test]
fn interrupted_claim_can_be_reloaded_using_the_original_independent_parent() {
    isolated(|fx| {
        let owned = OwnedRunDir::create(&fx.external, "owned", "run").unwrap();
        fx.sentinel(owned.path());
        let sibling = fx.sentinel(&fx.external.join("sibling"));
        let record_path = fx.external.join(META).join(record_name("owned"));
        let claim = std::rc::Rc::new(std::cell::RefCell::new(None::<PathBuf>));
        let observed = claim.clone();
        let hook = Hook::install(move |event, path| {
            if event == "after_claim" {
                *observed.borrow_mut() = Some(path.to_owned());
                panic!("simulate interruption without claim restoration");
            }
            Ok(())
        });
        let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owned.remove()));
        drop(hook);
        assert!(interrupted.is_err());
        let claim = claim
            .borrow()
            .clone()
            .expect("must reach the claim boundary");
        fx.assert_sentinel(&claim.join("sentinel"));
        assert!(!owned.path().exists());
        assert!(
            record_path.is_file(),
            "interruption must retain external retry evidence"
        );
        // Discard live descriptors: retry must use durable evidence rather
        // than authority retained only in the interrupted process.
        let original = owned.path().to_owned();
        drop(owned);
        let recovered = OwnedRunDir::load(&fx.external, &original, "run")
            .expect("verified interrupted claim must be recoverable from its original leaf");
        assert!(matches!(recovered.remove(), CleanupOutcome::Removed));
        assert!(!original.exists() && !claim.exists());
        assert!(
            !record_path.exists(),
            "discard evidence only after successful removal"
        );
        fx.assert_sentinel(&sibling);
    });
}

#[test]
fn interrupted_claim_with_a_replacement_at_the_original_leaf_preserves_both() {
    isolated(|fx| {
        let owned = OwnedRunDir::create(&fx.external, "owned", "run").unwrap();
        fx.sentinel(owned.path());
        let claim = std::rc::Rc::new(std::cell::RefCell::new(None::<PathBuf>));
        let observed = claim.clone();
        let hook = Hook::install(move |event, path| {
            if event == "after_claim" {
                *observed.borrow_mut() = Some(path.to_owned());
                panic!("simulate interruption");
            }
            Ok(())
        });
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owned.remove())).is_err());
        drop(hook);
        let claim = claim.borrow().clone().unwrap();
        let replacement = fx.sentinel(owned.path());
        // Even copying a complete marker cannot transfer the retained claim's
        // identity to the replacement. Recovery must never replace this leaf.
        fs::copy(claim.join(MARKER), owned.path().join(MARKER)).unwrap();
        assert!(OwnedRunDir::load(&fx.external, owned.path(), "run").is_err());
        fx.assert_sentinel(&replacement);
        fx.assert_sentinel(&claim.join("sentinel"));
        assert!(fx.external.join(META).join(record_name("owned")).is_file());
    });
}

#[test]
fn invalid_leaf_or_existing_entry_never_gains_creation_authority() {
    use std::os::unix::fs::symlink;
    isolated(|fx| {
        let sentinel = fx.sentinel(&fx.external.join("existing"));
        let linked_sentinel = fx.sentinel(&fx.evidence.join("link-target"));
        symlink(fx.evidence.join("link-target"), fx.external.join("linked")).unwrap();
        let absolute = fx.base.join("absolute").to_string_lossy().into_owned();
        for leaf in [
            "".to_owned(),
            ".".to_owned(),
            "..".to_owned(),
            "../escape".to_owned(),
            "nested/leaf".to_owned(),
            ".amplihack-claim-fake".to_owned(),
            "with\0nul".to_owned(),
            "x".repeat(129),
            absolute,
            "existing".to_owned(),
            "linked".to_owned(),
        ] {
            assert!(
                OwnedRunDir::create(&fx.external, &leaf, "run").is_err(),
                "{leaf:?}"
            );
        }
        fx.assert_sentinel(&sentinel);
        fx.assert_sentinel(&linked_sentinel);
        assert!(!fx.external.join("existing").join(MARKER).exists());
        assert!(!fx.evidence.join("link-target").join(MARKER).exists());
        assert!(!fx.base.join("escape").exists());
        assert!(!fx.base.join("absolute").exists());
    });
}

#[test]
fn stale_sweep_recovers_an_interrupted_claim_from_durable_creation_evidence() {
    use super::super::Category;
    use super::super::run_dir::{Outcome, RunScope};
    use super::super::run_tmp_root::TmpRootInputs;
    use super::super::sweep::{Action, Settings, run_sweep};
    use super::super::work_root::WorkRoot;
    use std::time::Duration;

    isolated(|fx| {
        let work = WorkRoot {
            path: fx.work.clone(),
            explicit: true,
        };
        let (mut scope, guard) = RunScope::begin_with(
            work.clone(),
            "interrupted-stale-contract",
            &fx.base,
            &TmpRootInputs::default(),
        )
        .unwrap();
        let runtime = guard.path().to_owned();
        // Select the parent from the layout contract, independently of any
        // discovered claim or ownership-record content.
        let parent = if runtime == scope.run_dir.join("runtime") {
            scope.run_dir.clone()
        } else {
            work.short_runtime_dir()
        };
        drop(guard);
        let owned = OwnedRunDir::create(
            &parent,
            runtime.file_name().unwrap().to_str().unwrap(),
            &scope.run_id,
        )
        .unwrap();
        fx.sentinel(owned.path());
        let sibling = fx.sentinel(&parent.join("sibling"));
        let claim = std::rc::Rc::new(std::cell::RefCell::new(None::<PathBuf>));
        let observed = claim.clone();
        let hook = Hook::install(move |event, path| {
            if event == "after_claim" {
                *observed.borrow_mut() = Some(path.to_owned());
                panic!("simulate process interruption after claiming");
            }
            Ok(())
        });
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owned.remove())).is_err());
        drop(hook);
        let claim = claim.borrow().clone().unwrap();
        fx.assert_sentinel(&claim.join("sentinel"));
        drop(owned);
        scope.finish(Outcome::Success, vec![], None);
        let settings = Settings {
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
        };
        let entries = run_sweep(&settings, true).unwrap();
        assert!(
            entries.iter().any(|entry| {
                entry.run_id.as_deref() == Some(scope.run_id.as_str())
                    && entry.category == Category::RunTmp
                    && entry.action == Action::Removed
            }),
            "stale discovery must recover the verified claim: {entries:?}"
        );
        assert!(!runtime.exists() && !claim.exists());
        fx.assert_sentinel(&sibling);
        assert!(scope.run_dir.join("run.json").is_file());
    });
}

#[test]
fn selected_parent_replacement_before_creation_never_grants_authority() {
    isolated(|fx| {
        let selected = RunRoot::select(&fx.external).unwrap();
        let saved = fx.base.join("selected-root");
        fs::rename(&fx.external, &saved).unwrap();
        let replacement = fx.sentinel(&fx.external);
        let original = fx.sentinel(&saved);
        assert!(OwnedRunDir::create_selected(&selected, "owned", "run").is_err());
        assert!(!fx.external.join("owned").exists());
        assert!(!saved.join("owned").exists());
        fx.assert_sentinel(&replacement);
        fx.assert_sentinel(&original);
    });
}
