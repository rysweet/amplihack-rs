//! Contracts for #2182: registered artifacts are linked, never copied.
//! No real binaries, host discovery, or HOME writes.
use super::super::test_support::artifacts::{ArtifactRegistry, LinkOps};
use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const LIMIT: u64 = 1024 * 1024;

struct Fixture {
    _temp: TempDir,
    app: PathBuf,
    source: PathBuf,
    bin: PathBuf,
}
impl Fixture {
    fn new(size: u64) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let app = home.join(".amplihack");
        let bin = home.join(".local/bin");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&app).unwrap();
        let source = temp.path().join("built-resolver");
        fs::File::create(&source).unwrap().set_len(size).unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o555)).unwrap();
        Self {
            _temp: temp,
            app,
            source,
            bin,
        }
    }
    fn registry(&self) -> ArtifactRegistry {
        let mut registry = ArtifactRegistry::new();
        registry.register(&self.source).unwrap();
        registry
    }
}

struct ForcedLinks {
    fail_symlink: bool,
}
impl LinkOps for ForcedLinks {
    fn hard_link(&self, _: &Path, _: &Path) -> io::Result<()> {
        Err(io::Error::from(io::ErrorKind::PermissionDenied))
    }
    fn symlink(&self, src: &Path, dst: &Path) -> io::Result<()> {
        if self.fail_symlink {
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        } else {
            symlink(src, dst)
        }
    }
}

#[test]
fn hardlink_failure_falls_back_to_verified_symlink() {
    let f = Fixture::new(LIMIT + 1);
    let dst = f.bin.join("resolver");
    f.registry()
        .link_with(
            &f.source,
            &dst,
            &f.bin,
            &ForcedLinks {
                fail_symlink: false,
            },
        )
        .unwrap();
    assert!(fs::symlink_metadata(&dst).unwrap().file_type().is_symlink());
}

#[test]
fn total_link_failure_never_falls_back_to_copy() {
    let f = Fixture::new(LIMIT + 1);
    let dst = f.bin.join("resolver");
    assert!(
        f.registry()
            .link_with(&f.source, &dst, &f.bin, &ForcedLinks { fail_symlink: true })
            .is_err()
    );
    assert!(fs::symlink_metadata(dst).is_err());
}

#[test]
fn escaped_destination_is_rejected_before_mutation() {
    let f = Fixture::new(16);
    let outside = f._temp.path().join("outside");
    fs::write(&outside, b"untouched").unwrap();
    let dst = f.bin.join("../../..").join("outside");
    assert!(
        f.registry()
            .link_with(
                &f.source,
                &dst,
                &f.bin,
                &ForcedLinks {
                    fail_symlink: false
                }
            )
            .is_err()
    );
    assert_eq!(fs::read(outside).unwrap(), b"untouched");
}

#[test]
fn registration_rejects_missing_directory_and_nonexecutable_sources() {
    let f = Fixture::new(16);
    let mut registry = ArtifactRegistry::new();
    assert!(registry.register(&f.bin.join("missing")).is_err());
    assert!(registry.register(&f.bin).is_err());
    fs::set_permissions(&f.source, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(registry.register(&f.source).is_err());
}

#[test]
fn replaced_registered_source_is_rejected() {
    let f = Fixture::new(LIMIT + 1);
    let registry = f.registry();
    let original = fs::metadata(&f.source).unwrap().ino();
    fs::rename(&f.source, f._temp.path().join("original")).unwrap();
    fs::write(&f.source, b"replacement").unwrap();
    fs::set_permissions(&f.source, fs::Permissions::from_mode(0o755)).unwrap();
    assert_ne!(fs::metadata(&f.source).unwrap().ino(), original);
    assert!(
        registry
            .link_with(
                &f.source,
                &f.bin.join("resolver"),
                &f.bin,
                &ForcedLinks {
                    fail_symlink: false
                }
            )
            .is_err()
    );
}

#[test]
fn final_release_staging_preserves_large_artifact_identity_and_permissions() {
    use super::super::{binary::stage_binary, release_layout::ReleaseTxn};
    // Covers the shared staging boundary for CLI, resolver, and hooks, rather
    // than accepting a link which a subsequent installation silently copies.
    for mode in [0o700, 0o750, 0o555] {
        for name in ["amplihack", "amplihack-asset-resolver", "amplihack-hooks"] {
            let f = Fixture::new(LIMIT + 1);
            fs::set_permissions(&f.source, fs::Permissions::from_mode(mode)).unwrap();
            let before = fs::metadata(&f.source).unwrap();
            let mut txn = ReleaseTxn::allocate(&f.app, "2182.0.0").unwrap();
            let published = f.bin.join(name);
            assert!(stage_binary(&mut txn, &f.source, &published).unwrap());
            let staged = txn.bin_dir().join(name);
            let metadata = fs::metadata(&staged).unwrap();
            assert_eq!(
                (metadata.dev(), metadata.ino()),
                (before.dev(), before.ino()),
                "same-filesystem staging must hardlink {name}"
            );
            assert_eq!(
                fs::metadata(&f.source).unwrap().permissions().mode(),
                before.permissions().mode(),
                "link staging must not chmod the source"
            );
            let report = txn.commit().unwrap();
            assert!(report.failed.is_none(), "{report:?}");
            assert_eq!(
                fs::metadata(&f.source).unwrap().permissions().mode(),
                before.permissions().mode(),
                "release commit must not chmod the linked {name} source"
            );
        }
    }
}

#[test]
fn symlinked_destination_parent_cannot_escape_confinement() {
    let f = Fixture::new(16);
    let outside = f._temp.path().join("outside-dir");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, f.bin.join("escape")).unwrap();
    assert!(
        f.registry()
            .link_with(
                &f.source,
                &f.bin.join("escape/resolver"),
                &f.bin,
                &ForcedLinks {
                    fail_symlink: false
                }
            )
            .is_err()
    );
    assert!(!outside.join("resolver").exists());
}

#[test]
fn staging_rejects_small_invalid_sources_before_writing() {
    use super::super::{binary::stage_binary, release_layout::ReleaseTxn};
    for kind in ["nonexecutable", "directory", "dangling", "cycle"] {
        let f = Fixture::new(16);
        fs::remove_file(&f.source).unwrap();
        match kind {
            "nonexecutable" => fs::write(&f.source, b"invalid").unwrap(),
            "directory" => fs::create_dir(&f.source).unwrap(),
            "dangling" => symlink(f._temp.path().join("missing"), &f.source).unwrap(),
            _ => symlink(&f.source, &f.source).unwrap(),
        }
        let published = f.bin.join("invalid");
        fs::write(&published, b"existing publication").unwrap();
        let mut txn = ReleaseTxn::allocate(&f.app, "2182.0.0").unwrap();
        assert!(
            stage_binary(&mut txn, &f.source, &published).is_err(),
            "{kind}"
        );
        assert!(!txn.bin_dir().join("invalid").exists());
        assert_eq!(fs::read(published).unwrap(), b"existing publication");
    }
}

/// The discovery a production install uses, called directly: test builds
/// stage a stub in `deploy_binaries_into`, so without these nothing in the
/// test suite would reach the sibling and PATH lookup that ships.
mod production_resolver_discovery {
    use super::super::super::binary::discover_resolver;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    const RESOLVER: &str = "amplihack-asset-resolver";

    fn executable(path: &Path) -> PathBuf {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        path.to_path_buf()
    }

    struct Layout {
        _temp: tempfile::TempDir,
        self_src: PathBuf,
        sibling: PathBuf,
        on_path: PathBuf,
        path: std::ffi::OsString,
    }

    fn layout() -> Layout {
        let temp = tempfile::tempdir().unwrap();
        let self_src = executable(&temp.path().join("release/amplihack"));
        let sibling = temp.path().join("release").join(RESOLVER);
        let empty = temp.path().join("empty");
        fs::create_dir_all(&empty).unwrap();
        let on_path = executable(&temp.path().join("path-bin").join(RESOLVER));
        let path = std::env::join_paths([empty, temp.path().join("path-bin")]).unwrap();
        Layout {
            _temp: temp,
            self_src,
            sibling,
            on_path,
            path,
        }
    }

    #[test]
    fn an_executable_sibling_wins_over_path() {
        let l = layout();
        executable(&l.sibling);
        assert_eq!(
            discover_resolver(&l.self_src, true, Some(&l.path)),
            Some(l.sibling.clone())
        );
    }

    #[test]
    fn a_non_executable_sibling_falls_back_to_path() {
        let l = layout();
        executable(&l.sibling);
        fs::set_permissions(&l.sibling, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            discover_resolver(&l.self_src, true, Some(&l.path)),
            Some(l.on_path.clone())
        );
    }

    #[test]
    fn path_is_searched_only_when_allowed() {
        let l = layout();
        assert_eq!(
            discover_resolver(&l.self_src, true, Some(&l.path)),
            Some(l.on_path.clone())
        );
        assert_eq!(discover_resolver(&l.self_src, false, Some(&l.path)), None);
        assert_eq!(discover_resolver(&l.self_src, true, None), None);
    }
}
