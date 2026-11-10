//! Red-phase contracts for #2182. No real binaries, host discovery, or HOME writes.
//!
//! Proposed internal API: ArtifactRegistry::{new, register, link_with},
//! LinkOps::{hard_link, symlink}, and footprint::verify(home, app, registry).
//! `link_with` confines destinations to its explicit root and never copies.
use super::super::test_support::{
    artifacts::{ArtifactRegistry, LinkOps},
    footprint::verify,
};
use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const LIMIT: u64 = 1024 * 1024;

struct Fixture {
    _temp: TempDir,
    home: PathBuf,
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
            home,
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
    fn release(&self, name: &str) -> PathBuf {
        let bin = self.app.join("releases").join(name).join("bin");
        fs::create_dir_all(&bin).unwrap();
        bin.join("amplihack-asset-resolver")
    }
}

#[test]
fn independent_file_limit_is_inclusive() {
    for (size, accepted) in [(LIMIT - 1, true), (LIMIT, true), (LIMIT + 1, false)] {
        let f = Fixture::new(size);
        fs::copy(&f.source, f.bin.join("resolver")).unwrap();
        assert_eq!(
            verify(&f.home, &f.app, &ArtifactRegistry::new()).is_ok(),
            accepted,
            "size={size}"
        );
    }
}

#[test]
fn registered_hardlinks_and_symlinks_are_accepted() {
    let f = Fixture::new(LIMIT + 1);
    fs::hard_link(&f.source, f.bin.join("hard")).unwrap();
    symlink(&f.source, f.bin.join("symbolic")).unwrap();
    let report = verify(&f.home, &f.app, &f.registry()).unwrap();
    assert_eq!(report.oversized_independent_bytes, 0);
    assert_eq!(report.verified_linked_logical_bytes, 2 * (LIMIT + 1));
}

#[test]
fn unrelated_hardlink_is_not_proof_of_provenance() {
    let f = Fixture::new(LIMIT + 1);
    let unrelated = f._temp.path().join("unrelated");
    fs::copy(&f.source, &unrelated).unwrap();
    fs::hard_link(&unrelated, f.bin.join("resolver")).unwrap();
    assert!(verify(&f.home, &f.app, &f.registry()).is_err());
}

#[test]
fn publication_symlink_cannot_conceal_copied_release_target() {
    let f = Fixture::new(LIMIT + 1);
    let copied = f.release("published");
    fs::copy(&f.source, &copied).unwrap();
    symlink(&copied, f.bin.join("resolver")).unwrap();
    assert!(verify(&f.home, &f.app, &f.registry()).is_err());
}

#[test]
fn unpublished_and_retained_release_copies_are_checked() {
    for release in [".unpublished", "old-retained"] {
        let f = Fixture::new(LIMIT + 1);
        fs::copy(&f.source, f.release(release)).unwrap();
        assert!(verify(&f.home, &f.app, &f.registry()).is_err(), "{release}");
    }
}

#[test]
fn dangling_and_cyclic_links_fail_closed() {
    for cyclic in [false, true] {
        let f = Fixture::new(1);
        let dst = f.bin.join("resolver");
        symlink(
            if cyclic {
                dst.clone()
            } else {
                f.bin.join("missing")
            },
            dst,
        )
        .unwrap();
        assert!(verify(&f.home, &f.app, &f.registry()).is_err());
    }
}

#[test]
fn release_copy_cannot_be_registered_to_legitimize_itself() {
    let f = Fixture::new(LIMIT + 1);
    let copied = f.release("copied");
    fs::copy(&f.source, &copied).unwrap();
    let mut registry = f.registry();
    // Either registration or verification must reject a release-local source.
    if registry.register(&copied).is_ok() {
        assert!(verify(&f.home, &f.app, &registry).is_err());
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
    verify(&f.home, &f.app, &f.registry()).unwrap();
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
    for name in ["amplihack", "amplihack-asset-resolver", "amplihack-hooks"] {
        let f = Fixture::new(LIMIT + 1);
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
        verify(&f.home, &f.app, &f.registry()).unwrap();
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
fn directory_boundaries_reject_links_and_unexpected_types() {
    for boundary in [
        ".local",
        ".local/bin",
        ".amplihack/releases",
        ".amplihack/releases/old",
        ".amplihack/releases/old/bin",
    ] {
        for kind in ["dangling", "cycle", "external", "file"] {
            let f = Fixture::new(16);
            let path = f.home.join(boundary);
            if path.is_dir() {
                fs::remove_dir_all(&path).unwrap();
            }
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            match kind {
                "dangling" => symlink(f._temp.path().join("missing"), &path).unwrap(),
                "cycle" => symlink(&path, &path).unwrap(),
                "external" => symlink(f._temp.path(), &path).unwrap(),
                _ => fs::write(&path, b"not a directory").unwrap(),
            }
            assert!(
                verify(&f.home, &f.app, &f.registry()).is_err(),
                "{boundary}: {kind}"
            );
        }
    }
}

#[test]
fn small_external_links_require_registration_but_release_publications_work() {
    let f = Fixture::new(16);
    symlink(&f.source, f.bin.join("external")).unwrap();
    assert!(verify(&f.home, &f.app, &ArtifactRegistry::new()).is_err());
    verify(&f.home, &f.app, &f.registry()).unwrap();
    fs::remove_file(f.bin.join("external")).unwrap();
    let release = f.release("small");
    fs::copy(&f.source, &release).unwrap();
    symlink(&release, f.bin.join("published")).unwrap();
    verify(&f.home, &f.app, &ArtifactRegistry::new()).unwrap();
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

#[test]
fn nested_uninspected_bin_is_not_a_confined_release_target() {
    let f = Fixture::new(16);
    let hidden = f.app.join("releases/old/hidden/bin");
    fs::create_dir_all(&hidden).unwrap();
    fs::copy(&f.source, hidden.join("resolver")).unwrap();
    symlink(hidden.join("resolver"), f.bin.join("resolver")).unwrap();
    assert!(verify(&f.home, &f.app, &ArtifactRegistry::new()).is_err());
}
