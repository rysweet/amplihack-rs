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
fn unavailable_identity_or_claim_primitive_preserves_contents() {
    isolated(|fx| {
        for event_name in ["identity", "rename"] {
            let owned = OwnedRunDir::create(&fx.external, event_name, "run").unwrap();
            let sentinel = fx.sentinel(owned.path());
            let hook = Hook::install(move |event, _| {
                if event == event_name {
                    return Err(std::io::Error::from_raw_os_error(libc::ENOSYS).into());
                }
                Ok(())
            });
            preserved(owned.remove());
            drop(hook);
            fx.assert_sentinel(&sentinel);
        }
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
