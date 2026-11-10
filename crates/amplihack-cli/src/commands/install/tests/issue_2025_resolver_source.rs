//! Issue #2025: where `amplihack install` takes `amplihack-asset-resolver`
//! from.
//!
//! The sibling of the running binary is preferred. When there is none, a copy
//! found on PATH is used only if it does not sit in a cargo build directory
//! (`target`, `target-*`, `debug`, `release`), checked on both the PATH entry
//! and its canonical path. A refused copy leaves the slot as it was. This is
//! what kept the 590 MB unstripped debug build out of `~/.local/bin`.
//!
//! The selector tests call `binary::choose_resolver_source` directly. The
//! `Fixture` tests run the installer end to end, in the style of
//! `issue_1526_implicit_mode.rs`; they skip when a real resolver sits beside
//! the test executable, because that sibling would win over PATH.

use super::helpers::*;
use super::issue_1526_implicit_downgrade::{IsolatedHome, assert_home_is};
use super::*;
use binary::{ResolverSource, choose_resolver_source, looks_like_build_artifact};
use std::fs;
use std::path::{Path, PathBuf};

const RESOLVER: &str = "amplihack-asset-resolver";

fn write_exe(dir: &Path, name: &str, body: &str) -> PathBuf {
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

/// A temp root that is not itself under a build directory, so the
/// "plain directory" cases mean what they say.
fn plain_tempdir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        !looks_like_build_artifact(&dir.path().join("x")),
        "fixture: TMPDIR {} is under a build directory; these tests need a plain one",
        dir.path().display()
    );
    dir
}

fn never_called() -> Option<PathBuf> {
    panic!("the PATH lookup must not run here")
}

// ── The selector ──

#[test]
fn sibling_wins_over_path() {
    let tmp = plain_tempdir();
    let bin = tmp.path().join("bin");
    let self_src = write_exe(&bin, "amplihack", "#!/bin/sh\n");
    let sibling = write_exe(&bin, RESOLVER, "#!/bin/sh\n# sibling\n");

    let chosen = choose_resolver_source(&self_src, true, never_called);

    assert_eq!(chosen, ResolverSource::Sibling(sibling));
}

/// Documented in install-command.md: the build-directory check applies only
/// to a PATH copy. `./target/debug/amplihack install` deploys its sibling.
#[test]
fn sibling_in_build_dir_is_still_used() {
    let tmp = plain_tempdir();
    let bin = tmp.path().join("target").join("debug");
    let self_src = write_exe(&bin, "amplihack", "#!/bin/sh\n");
    let sibling = write_exe(&bin, RESOLVER, "#!/bin/sh\n");

    let chosen = choose_resolver_source(&self_src, true, never_called);

    assert_eq!(chosen, ResolverSource::Sibling(sibling));
}

#[cfg(unix)]
#[test]
fn non_executable_sibling_falls_back_to_path() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = plain_tempdir();
    let bin = tmp.path().join("bin");
    let self_src = write_exe(&bin, "amplihack", "#!/bin/sh\n");
    let sibling = write_exe(&bin, RESOLVER, "#!/bin/sh\n");
    fs::set_permissions(&sibling, fs::Permissions::from_mode(0o644)).unwrap();
    let on_path = write_exe(&tmp.path().join("usr-bin"), RESOLVER, "#!/bin/sh\n");

    let chosen = choose_resolver_source(&self_src, true, || Some(on_path.clone()));

    assert_eq!(
        chosen,
        ResolverSource::Path(on_path.canonicalize().unwrap())
    );
}

#[test]
fn path_copy_in_target_debug_is_refused() {
    let tmp = plain_tempdir();
    let self_src = write_exe(&tmp.path().join("bin"), "amplihack", "#!/bin/sh\n");
    let on_path = write_exe(
        &tmp.path().join("checkout").join("target").join("debug"),
        RESOLVER,
        "#!/bin/sh\n",
    );

    let chosen = choose_resolver_source(&self_src, true, || Some(on_path.clone()));

    assert!(
        matches!(chosen, ResolverSource::RefusedBuildArtifact(ref p) if p.ends_with(RESOLVER)),
        "a PATH copy under target/debug must be refused, got {chosen:?}"
    );
}

#[test]
fn path_copy_in_each_build_dir_shape_is_refused() {
    let tmp = plain_tempdir();
    let self_src = write_exe(&tmp.path().join("bin"), "amplihack", "#!/bin/sh\n");
    for dir in [
        tmp.path().join("target"),
        tmp.path().join("target-x86_64").join("bin"),
        tmp.path().join("out").join("release"),
        tmp.path().join("out").join("debug"),
    ] {
        let on_path = write_exe(&dir, RESOLVER, "#!/bin/sh\n");
        let chosen = choose_resolver_source(&self_src, true, || Some(on_path.clone()));
        assert!(
            matches!(chosen, ResolverSource::RefusedBuildArtifact(_)),
            "{} must be refused, got {chosen:?}",
            on_path.display()
        );
    }
}

#[cfg(unix)]
#[test]
fn path_symlink_into_build_dir_is_refused() {
    let tmp = plain_tempdir();
    let self_src = write_exe(&tmp.path().join("bin"), "amplihack", "#!/bin/sh\n");
    let real = write_exe(
        &tmp.path().join("checkout").join("target").join("release"),
        RESOLVER,
        "#!/bin/sh\n",
    );
    let link_dir = tmp.path().join("cargo-bin");
    fs::create_dir_all(&link_dir).unwrap();
    let link = link_dir.join(RESOLVER);
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let chosen = choose_resolver_source(&self_src, true, || Some(link.clone()));

    assert!(
        matches!(chosen, ResolverSource::RefusedBuildArtifact(_)),
        "a PATH symlink whose target is in a build directory must be refused, got {chosen:?}"
    );
}

#[test]
fn path_copy_outside_build_dir_is_used() {
    let tmp = plain_tempdir();
    let self_src = write_exe(&tmp.path().join("bin"), "amplihack", "#!/bin/sh\n");
    let on_path = write_exe(&tmp.path().join("usr-bin"), RESOLVER, "#!/bin/sh\n");

    let chosen = choose_resolver_source(&self_src, true, || Some(on_path.clone()));

    assert_eq!(
        chosen,
        ResolverSource::Path(on_path.canonicalize().unwrap())
    );
}

/// S6: the path that is checked is the path that is copied, so `Path(..)`
/// carries the canonical target of a symlink.
#[cfg(unix)]
#[test]
fn selector_returns_canonical_path() {
    let tmp = plain_tempdir();
    let self_src = write_exe(&tmp.path().join("bin"), "amplihack", "#!/bin/sh\n");
    let real = write_exe(&tmp.path().join("store"), RESOLVER, "#!/bin/sh\n");
    let link_dir = tmp.path().join("usr-bin");
    fs::create_dir_all(&link_dir).unwrap();
    let link = link_dir.join(RESOLVER);
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let chosen = choose_resolver_source(&self_src, true, || Some(link.clone()));

    assert_eq!(chosen, ResolverSource::Path(real.canonicalize().unwrap()));
}

/// S6: when canonicalize fails, the raw path is used for both the check and
/// the copy; the check is never skipped.
#[test]
fn uncanonicalizable_path_is_checked_raw() {
    let tmp = plain_tempdir();
    let self_src = write_exe(&tmp.path().join("bin"), "amplihack", "#!/bin/sh\n");
    let missing_plain = tmp.path().join("gone").join(RESOLVER);
    let missing_build = tmp.path().join("target").join("debug").join(RESOLVER);

    assert_eq!(
        choose_resolver_source(&self_src, true, || Some(missing_plain.clone())),
        ResolverSource::Path(missing_plain.clone())
    );
    assert!(matches!(
        choose_resolver_source(&self_src, true, || Some(missing_build.clone())),
        ResolverSource::RefusedBuildArtifact(_)
    ));
}

#[test]
fn implicit_with_filled_slot_skips_path() {
    let tmp = plain_tempdir();
    let self_src = write_exe(&tmp.path().join("bin"), "amplihack", "#!/bin/sh\n");

    let chosen = choose_resolver_source(&self_src, false, never_called);

    assert_eq!(chosen, ResolverSource::None);
}

#[test]
fn nothing_found_is_none() {
    let tmp = plain_tempdir();
    let self_src = write_exe(&tmp.path().join("bin"), "amplihack", "#!/bin/sh\n");

    assert_eq!(
        choose_resolver_source(&self_src, true, || None),
        ResolverSource::None
    );
}

// ── The installer ──

/// An isolated HOME, a hooks stub, and a PATH whose first entry is
/// `resolver_dir`. Everything lives outside HOME.
struct Fixture {
    _home: tempfile::TempDir,
    _outside: tempfile::TempDir,
    home: PathBuf,
    repo: PathBuf,
    _env: IsolatedHome,
}

impl Fixture {
    /// `None` when a real resolver sits beside the test executable: the
    /// installer would take that sibling and never consult PATH.
    fn new(resolver_dir: &str) -> Option<(Self, PathBuf)> {
        let exe = std::env::current_exe().unwrap();
        let running = resolve_running_binary_source(&exe);
        if running.parent().is_some_and(|dir| {
            dir.join(crate::path_conflicts::binary_filename(RESOLVER))
                .exists()
        }) {
            eprintln!(
                "skipping: a sibling {RESOLVER} beside {} wins over PATH",
                running.display()
            );
            return None;
        }
        let home_dir = tempfile::tempdir().unwrap();
        let outside_dir = plain_tempdir();
        let home = home_dir.path().to_path_buf();
        let outside = outside_dir.path().to_path_buf();
        let stub_bin = outside.join("stub_bin");
        create_exe_stub(&stub_bin, "python3");
        let hooks_stub = create_exe_stub(&stub_bin, "amplihack-hooks");
        let runner_stub = create_exe_stub(&stub_bin, "recipe-runner-rs");
        let resolver = write_exe(
            &outside.join(resolver_dir),
            RESOLVER,
            "#!/bin/sh\n# resolver stub from PATH\n",
        );
        let repo = outside.join("local-repo");
        create_source_repo(&repo);

        let mut env = IsolatedHome::new(&home);
        env.set("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_stub);
        env.set("RECIPE_RUNNER_RS_PATH", &runner_stub);
        env.set("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL", "1");
        let mut path_dirs = vec![resolver.parent().unwrap().to_path_buf(), stub_bin];
        path_dirs.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        env.set("PATH", std::env::join_paths(path_dirs).unwrap());
        Some((
            Self {
                _home: home_dir,
                _outside: outside_dir,
                home,
                repo,
                _env: env,
            },
            resolver,
        ))
    }

    fn installed_resolver(&self) -> PathBuf {
        crate::path_conflicts::preferred_user_bin(&self.home)
            .join(crate::path_conflicts::binary_filename(RESOLVER))
    }

    fn install(&self, mode: InstallMode) {
        assert_home_is(&self.home);
        local_install_scoped(&self.repo, None, false, mode)
            .unwrap_or_else(|e| panic!("{mode:?} install failed: {e:#}"));
    }
}

#[test]
fn explicit_install_does_not_link_build_dir_resolver() {
    let Some((fx, resolver)) = Fixture::new("checkout/target/debug") else {
        return;
    };
    assert!(looks_like_build_artifact(&resolver));

    fx.install(InstallMode::Explicit);

    let installed = fx.installed_resolver();
    assert!(
        installed.symlink_metadata().is_err(),
        "a resolver from {} was installed at {}",
        resolver.display(),
        installed.display()
    );
    // Nothing in the new release names it either.
    let releases = fx.home.join(".amplihack").join("releases");
    if releases.is_dir() {
        for release in fs::read_dir(&releases).unwrap() {
            let staged = release.unwrap().path().join("bin").join(RESOLVER);
            assert!(
                !staged.exists(),
                "the refused resolver was staged at {}",
                staged.display()
            );
        }
    }
}

#[test]
fn explicit_install_links_plain_path_resolver() {
    let Some((fx, resolver)) = Fixture::new("usr-bin") else {
        return;
    };
    assert!(!looks_like_build_artifact(&resolver));

    fx.install(InstallMode::Explicit);

    assert_eq!(
        fs::read(fx.installed_resolver())
            .unwrap_or_else(|e| panic!("no resolver installed from a plain PATH directory: {e}")),
        fs::read(&resolver).unwrap(),
        "the staged resolver must be the PATH copy, byte for byte"
    );
}

#[test]
fn refused_path_copy_keeps_existing_slot() {
    let Some((fx, _resolver)) = Fixture::new("checkout/target/release") else {
        return;
    };
    let installed = fx.installed_resolver();
    fs::create_dir_all(installed.parent().unwrap()).unwrap();
    let sentinel = b"#!/bin/sh\n# resolver from an earlier install\n".to_vec();
    fs::write(&installed, &sentinel).unwrap();

    fx.install(InstallMode::Explicit);

    let meta = installed.symlink_metadata().expect("slot was removed");
    assert!(
        !meta.file_type().is_symlink(),
        "a refused PATH copy relinked {}",
        installed.display()
    );
    assert_eq!(
        fs::read(&installed).unwrap(),
        sentinel,
        "a refused PATH copy changed the installed resolver"
    );
}

#[test]
fn implicit_install_with_filled_slot_keeps_it() {
    let Some((fx, _resolver)) = Fixture::new("checkout/target/debug") else {
        return;
    };
    let installed = fx.installed_resolver();
    fs::create_dir_all(installed.parent().unwrap()).unwrap();
    let sentinel = b"#!/bin/sh\n# kept\n".to_vec();
    fs::write(&installed, &sentinel).unwrap();

    fx.install(InstallMode::Implicit);

    assert_eq!(fs::read(&installed).unwrap(), sentinel);
}
