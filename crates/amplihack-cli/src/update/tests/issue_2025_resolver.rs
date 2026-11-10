//! Issue #2025: `amplihack update` installs `amplihack-asset-resolver` from
//! the release archive when the archive has one, and carries on without it
//! for older archives. Without this, an update leaves the 590 MB unstripped
//! resolver from an earlier source install beside the new `amplihack`.
//!
//! The resolver is optional only in being absent. A symlink entry or an I/O
//! error is still an error, and a resolver that is present but fails to stage
//! rolls the whole release back.

use super::super::UpdateRelease;
use super::super::install::{
    BinaryInstallPlan, InstallArchiveLayout, binary_filename, find_binary, find_optional_binary,
    install_into_release, plan_downloaded_binary_install,
};
use crate::commands::install::release_layout;
use crate::path_conflicts::InstallTargetDecision;
use std::fs;
use std::path::{Path, PathBuf};

const RESOLVER: &str = "amplihack-asset-resolver";

fn write_executable(dir: &Path, name: &str, body: &str) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

fn user_bin_decision(install_dir: PathBuf) -> InstallTargetDecision {
    InstallTargetDecision::PreferredUserBin {
        install_dir,
        reason: "test".into(),
    }
}

// ── Archive lookup ──

#[test]
fn binary_filename_maps_resolver() {
    let want = if cfg!(windows) {
        "amplihack-asset-resolver.exe"
    } else {
        "amplihack-asset-resolver"
    };
    assert_eq!(binary_filename(RESOLVER), want);
}

#[test]
fn find_optional_binary_returns_none_when_absent() {
    let temp = tempfile::tempdir().unwrap();
    write_executable(&temp.path().join("pkg"), "amplihack", "#!/bin/sh\n");

    let found = find_optional_binary(temp.path(), binary_filename(RESOLVER))
        .expect("absence is not an error");

    assert_eq!(found, None);
}

#[test]
fn find_optional_binary_finds_nested_resolver() {
    let temp = tempfile::tempdir().unwrap();
    let resolver = write_executable(&temp.path().join("pkg").join("bin"), RESOLVER, "x");

    assert_eq!(
        find_optional_binary(temp.path(), binary_filename(RESOLVER)).unwrap(),
        Some(resolver)
    );
}

#[cfg(unix)]
#[test]
fn find_optional_binary_rejects_symlink() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = write_executable(outside.path(), RESOLVER, "#!/bin/sh\n");
    let root = temp.path().join("extract");
    fs::create_dir_all(&root).unwrap();
    std::os::unix::fs::symlink(&target, root.join(RESOLVER)).unwrap();

    let result = find_optional_binary(&root, binary_filename(RESOLVER));

    assert!(
        result.is_err(),
        "a symlinked resolver in the archive must be an error, not Ok(None) or Ok(Some): {result:?}"
    );
}

#[test]
fn find_optional_binary_propagates_traversal_errors() {
    let temp = tempfile::tempdir().unwrap();
    let not_a_dir = temp.path().join("not-a-directory");
    fs::write(&not_a_dir, b"x").unwrap();

    let err = find_optional_binary(&not_a_dir, binary_filename(RESOLVER)).unwrap_err();

    assert!(
        err.to_string()
            .contains("failed to read extracted archive directory"),
        "got: {err}"
    );
}

#[test]
fn find_binary_missing_file_error_text_is_unchanged() {
    let temp = tempfile::tempdir().unwrap();

    let err = find_binary(temp.path(), binary_filename("amplihack")).unwrap_err();

    assert_eq!(
        err.to_string(),
        format!(
            "binary '{}' not found in downloaded archive",
            binary_filename("amplihack")
        )
    );
}

// ── The plan ──

#[test]
fn plan_includes_resolver_when_archive_has_one() {
    let temp = tempfile::tempdir().unwrap();
    let archive = temp.path().join("archive");
    let user_bin = temp.path().join("home/.local/bin");
    let layout = InstallArchiveLayout {
        amplihack: write_executable(&archive, "amplihack", "a"),
        hooks: write_executable(&archive, "amplihack-hooks", "h"),
        resolver: Some(write_executable(&archive, RESOLVER, "r")),
    };

    let plan = plan_downloaded_binary_install(layout, user_bin_decision(user_bin.clone())).unwrap();

    assert_eq!(
        plan.resolver_destination,
        Some(user_bin.join(binary_filename(RESOLVER)))
    );
}

#[test]
fn plan_omits_resolver_for_older_archive() {
    let temp = tempfile::tempdir().unwrap();
    let archive = temp.path().join("archive");
    let user_bin = temp.path().join("home/.local/bin");
    let layout = InstallArchiveLayout {
        amplihack: write_executable(&archive, "amplihack", "a"),
        hooks: write_executable(&archive, "amplihack-hooks", "h"),
        resolver: None,
    };

    let plan = plan_downloaded_binary_install(layout, user_bin_decision(user_bin.clone())).unwrap();

    assert_eq!(
        plan,
        BinaryInstallPlan {
            amplihack_destination: user_bin.join(binary_filename("amplihack")),
            hooks_destination: user_bin.join(binary_filename("amplihack-hooks")),
            resolver_destination: None,
        }
    );
}

// ── Staging into a release ──

struct ReleaseFixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
    archive: PathBuf,
    user_bin: PathBuf,
}

impl ReleaseFixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let user_bin = home.join(".local/bin");
        fs::create_dir_all(&user_bin).unwrap();
        Self {
            archive: temp.path().join("archive"),
            home,
            user_bin,
            _temp: temp,
        }
    }

    fn release() -> UpdateRelease {
        UpdateRelease {
            version: "99.0.0".into(),
            asset_url: "https://github.com/rysweet/amplihack-rs/releases/download/v99.0.0/x.tar.gz"
                .into(),
            checksum_url: None,
        }
    }

    fn plan(&self, with_resolver: bool) -> BinaryInstallPlan {
        BinaryInstallPlan {
            amplihack_destination: self.user_bin.join(binary_filename("amplihack")),
            hooks_destination: self.user_bin.join(binary_filename("amplihack-hooks")),
            resolver_destination: with_resolver
                .then(|| self.user_bin.join(binary_filename(RESOLVER))),
        }
    }

    fn releases(&self) -> PathBuf {
        release_layout::releases_dir(&self.home.join(".amplihack"))
    }
}

#[cfg(unix)]
#[test]
fn install_into_release_links_resolver_from_archive() {
    let fx = ReleaseFixture::new();
    let layout = InstallArchiveLayout {
        amplihack: write_executable(&fx.archive, "amplihack", "#!/bin/sh\nexit 0\n"),
        hooks: write_executable(&fx.archive, "amplihack-hooks", "#!/bin/sh\nexit 0\n"),
        resolver: Some(write_executable(
            &fx.archive,
            RESOLVER,
            "#!/bin/sh\n# stripped resolver\n",
        )),
    };
    let plan = fx.plan(true);

    install_into_release(&fx.home, &ReleaseFixture::release(), &layout, &plan).unwrap();

    let link = plan.resolver_destination.unwrap();
    let meta = link.symlink_metadata().expect("resolver link exists");
    assert!(
        meta.file_type().is_symlink(),
        "{} is not a link",
        link.display()
    );
    let target = fs::read_link(&link).unwrap();
    assert!(
        target.starts_with(fx.releases()),
        "resolver link names {} outside the releases directory",
        target.display()
    );
    assert_eq!(
        fs::read(&link).unwrap(),
        b"#!/bin/sh\n# stripped resolver\n"
    );
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o7777,
        0o755
    );
}

#[cfg(unix)]
#[test]
fn install_into_release_without_resolver_leaves_old_one_alone() {
    let fx = ReleaseFixture::new();
    let old = fx.user_bin.join(RESOLVER);
    fs::write(&old, b"old resolver").unwrap();
    let layout = InstallArchiveLayout {
        amplihack: write_executable(&fx.archive, "amplihack", "#!/bin/sh\nexit 0\n"),
        hooks: write_executable(&fx.archive, "amplihack-hooks", "#!/bin/sh\nexit 0\n"),
        resolver: None,
    };

    install_into_release(
        &fx.home,
        &ReleaseFixture::release(),
        &layout,
        &fx.plan(false),
    )
    .expect("an archive without a resolver installs as before");

    assert_eq!(fs::read(&old).unwrap(), b"old resolver");
}

/// S3: a resolver present in the archive is required. Failing to stage it
/// rolls back the whole release, and the earlier binaries stay current.
#[cfg(unix)]
#[test]
fn resolver_stage_failure_rolls_back_release() {
    let fx = ReleaseFixture::new();
    let old_amplihack = fx.user_bin.join("amplihack");
    let old_hooks = fx.user_bin.join("amplihack-hooks");
    fs::write(&old_amplihack, b"old amplihack").unwrap();
    fs::write(&old_hooks, b"old hooks").unwrap();
    // A directory where the resolver file should be: the copy fails.
    let bad_resolver = fx.archive.join("bad").join(RESOLVER);
    fs::create_dir_all(&bad_resolver).unwrap();
    let layout = InstallArchiveLayout {
        amplihack: write_executable(&fx.archive, "amplihack", "#!/bin/sh\nexit 0\n"),
        hooks: write_executable(&fx.archive, "amplihack-hooks", "#!/bin/sh\nexit 0\n"),
        resolver: Some(bad_resolver),
    };

    let result = install_into_release(
        &fx.home,
        &ReleaseFixture::release(),
        &layout,
        &fx.plan(true),
    );

    assert!(
        result.is_err(),
        "a failed resolver stage must fail the update"
    );
    assert_eq!(fs::read(&old_amplihack).unwrap(), b"old amplihack");
    assert_eq!(fs::read(&old_hooks).unwrap(), b"old hooks");
    assert!(
        !old_amplihack
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
            && !old_hooks
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink(),
        "a link was switched before the resolver staged"
    );
    assert!(fx.user_bin.join(RESOLVER).symlink_metadata().is_err());
    let leftovers: Vec<PathBuf> = fs::read_dir(fx.releases())
        .map(|it| it.map(|e| e.unwrap().path()).collect())
        .unwrap_or_default();
    assert!(
        leftovers.is_empty(),
        "the unpublished release was not removed: {leftovers:?}"
    );
}

// checksum_failure_stages_no_resolver: there is no seam between
// `verify_sha256` and `extract_archive` in `download_and_replace`. The order
// (verify, then extract, then `find_optional_binary`) means a checksum
// failure returns before any archive entry, the resolver included, is read.
