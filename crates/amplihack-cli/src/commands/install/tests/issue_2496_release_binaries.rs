//! Explicit installation must obtain binaries separately from framework assets.
//! These call the existing coordinator, so the red tests demonstrate checkout
//! publication and side effects rather than depending on a proposed resolver API.
use super::super::*;
use super::issue_1955_support::{FakeFetcher, older_than_binary, versioned_tree};
use super::issue_2496_self_heal_origin::{assert_unchanged, isolate, snapshot};
use crate::test_support::release_fixture::{Downloads, native_release};
use std::os::unix::fs::{PermissionsExt, symlink};

fn installed_binaries(home: &Path, version: &str, native: &Path) -> PathBuf {
    let root = home.join(".amplihack/releases").join(version);
    fs::create_dir_all(root.join("bin")).unwrap();
    fs::create_dir_all(home.join(".local/bin")).unwrap();
    for name in ["amplihack", "amplihack-hooks", "amplihack-asset-resolver"] {
        fs::copy(native, root.join("bin").join(name)).unwrap();
        symlink(
            root.join("bin").join(name),
            home.join(".local/bin").join(name),
        )
        .unwrap();
    }
    fs::write(root.join(".complete"), b"").unwrap();
    fs::write(
        root.join(".installed-by"),
        format!("format=1\nversion={version}\nexe=\npid=1\npost_update=0\n"),
    )
    .unwrap();
    root
}

fn explicit(source: &Path) -> Result<()> {
    let fetcher = FakeFetcher::new(|_| panic!("matching local framework assets require no fetch"));
    run_install_with_fetcher(
        Some(source.to_owned()),
        false,
        false,
        false,
        InstallMode::Explicit,
        &framework_source::BinaryIdentity {
            version: crate::VERSION.to_owned(),
            may_fetch: false,
        },
        &fetcher,
    )
}

#[test]
fn explicit_install_publishes_selected_release_bytes_instead_of_the_checkout() {
    let _downloads = Downloads::unavailable();
    let native = crate::test_support::release_fixture::native_release().join("release");
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let mut env = isolate(&home);
    env.set("RELEASE_FIXTURE_VERSION", crate::VERSION);
    let source = versioned_tree(&temp.path().join("source"), crate::VERSION);
    let original = installed_binaries(&home, crate::VERSION, &native);
    let before = snapshot(&original);

    explicit(&source).expect("an eligible matching release remains installable from a checkout");

    for name in ["amplihack", "amplihack-hooks", "amplihack-asset-resolver"] {
        let link = home.join(".local/bin").join(name);
        let target = fs::read_link(&link).expect("publication must switch a link");
        assert!(release_layout::is_complete_release_path(
            &home.join(".amplihack/releases"),
            &link
        ));
        assert!(
            fs::read(&link).unwrap() == fs::read(original.join("bin").join(name)).unwrap(),
            "{name} must come from the selected release, regardless of PATH or hooks overrides"
        );
        assert_eq!(target.file_name().unwrap(), name);
    }
    assert_unchanged(&original, &before);
}

#[test]
fn explicit_local_install_uses_the_selected_version_for_receipt_and_stamp() {
    let _downloads = Downloads::unavailable();
    let native = crate::test_support::release_fixture::native_release().join("release");
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let mut env = isolate(&home);
    let selected = older_than_binary();
    env.set("RELEASE_FIXTURE_VERSION", &selected);
    let source = versioned_tree(&temp.path().join("source"), &selected);
    installed_binaries(&home, &selected, &native);

    explicit(&source).expect("--local deliberately selects the older release");

    assert_eq!(
        version_stamp::read_binaries_version().unwrap().as_deref(),
        Some(selected.as_str())
    );
    assert_eq!(
        version_stamp::read_installed_version().unwrap().as_deref(),
        Some(selected.as_str())
    );
    let active = fs::canonicalize(home.join(".local/bin/amplihack")).unwrap();
    let root = active.parent().unwrap().parent().unwrap();
    assert_eq!(
        release_layout::read_installed_by(root).unwrap().version,
        selected
    );
    assert!(
        fs::read(active).unwrap() == fs::read(native).unwrap(),
        "published bytes must match the selected release"
    );
}

#[test]
fn explicit_install_without_release_binaries_refuses_without_installation_or_profile_writes() {
    let _downloads = Downloads::unavailable();
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let _env = isolate(&home);
    let source = versioned_tree(&temp.path().join("source"), crate::VERSION);
    let before = snapshot(&home);

    let result = explicit(&source);

    assert_unchanged(&home, &before);
    let error = result.expect_err("assets and PATH hooks cannot authorise publishing a checkout");
    let message = format!("{error:#}");
    assert!(
        message.contains("release") && message.contains("executable"),
        "{message}"
    );
}

#[test]
fn invalid_installed_executables_cannot_be_substituted_by_the_checkout() {
    let _downloads = Downloads::unavailable();
    let native = crate::test_support::release_fixture::native_release().join("release");
    for case in [
        "incomplete",
        "escaping-hooks",
        "non-executable-main",
        "missing-hooks",
        "wrong-version",
    ] {
        let temp = crate::test_support::tempdir_outside_framework_roots();
        let home = temp.path().join("home");
        fs::create_dir(&home).unwrap();
        let mut env = isolate(&home);
        env.set("RELEASE_FIXTURE_VERSION", crate::VERSION);
        let source = versioned_tree(&temp.path().join("source"), crate::VERSION);
        let release = installed_binaries(&home, crate::VERSION, &native);
        let hooks = release.join("bin/amplihack-hooks");
        match case {
            "incomplete" => fs::remove_file(release.join(".complete")).unwrap(),
            "escaping-hooks" => {
                fs::remove_file(&hooks).unwrap();
                symlink(&native, &hooks).unwrap();
            }
            "non-executable-main" => fs::set_permissions(
                release.join("bin/amplihack"),
                fs::Permissions::from_mode(0o600),
            )
            .unwrap(),
            "missing-hooks" => fs::remove_file(&hooks).unwrap(),
            "wrong-version" => env.set("RELEASE_FIXTURE_VERSION", older_than_binary()),
            _ => unreachable!(),
        }
        let before = snapshot(&home);

        let result = explicit(&source);

        assert_unchanged(&home, &before);
        assert!(
            result.is_err(),
            "{case}: unsuitable executables require a release source"
        );
    }
}

#[test]
fn unavailable_release_executables_refuse_before_framework_fetch_or_its_cache_write() {
    let _downloads = Downloads::unavailable();
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let mut env = isolate(&home);
    let source = versioned_tree(&temp.path().join("old-source"), &older_than_binary());
    env.set("AMPLIHACK_HOME", source);
    let fetcher =
        FakeFetcher::new(|_| panic!("release executable refusal must precede asset acquisition"));
    let before = snapshot(&home);

    let result = run_install_with_fetcher(
        None,
        false,
        true,
        false,
        InstallMode::Explicit,
        &framework_source::BinaryIdentity {
            version: crate::VERSION.to_owned(),
            may_fetch: true,
        },
        &fetcher,
    );

    assert!(result.is_err());
    assert_eq!(fetcher.call_count(), 0);
    assert_unchanged(&home, &before);
}

#[test]
fn changed_selected_executables_are_refused_at_the_deployment_boundary() {
    let native = native_release().join("release");
    let _downloads = Downloads::unavailable();
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let mut env = isolate(&home);
    env.set("RELEASE_FIXTURE_VERSION", crate::VERSION);
    let selected = installed_binaries(&home, crate::VERSION, &native);
    let resolved = binary::ResolvedBinaries::resolve(crate::VERSION).unwrap();
    let mut txn =
        release_layout::ReleaseTxn::allocate(&home.join(".amplihack"), crate::VERSION).unwrap();
    let main = selected.join("bin/amplihack");
    fs::remove_file(&main).unwrap();
    symlink(&native, &main).unwrap();
    let before = snapshot(&home);

    let result = binary::deploy_binaries_into(&mut txn, InstallMode::Explicit, None, &resolved);

    assert!(
        result.is_err(),
        "a resolved path must not become a lasting permission grant"
    );
    assert_unchanged(&home, &before);
}

#[test]
fn explicit_install_uses_the_exact_selected_archive_and_keeps_extracted_binaries_alive() {
    let native = native_release().join("release");
    let selected = older_than_binary();
    let downloads = Downloads::release(&selected, &native, true);
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let mut env = isolate(&home);
    env.set("RELEASE_FIXTURE_VERSION", &selected);
    let source = versioned_tree(&temp.path().join("source"), &selected);

    explicit(&source)
        .expect("an exact release archive can supply binaries independently of local assets");

    assert!(
        !downloads.calls.borrow().is_empty(),
        "framework assets alone cannot supply release binaries"
    );
    assert!(
        downloads
            .calls
            .borrow()
            .iter()
            .all(|url| url.contains(&format!("/v{selected}")) && !url.ends_with("/latest"))
    );
    for name in ["amplihack", "amplihack-hooks", "amplihack-asset-resolver"] {
        assert!(
            fs::read(home.join(".local/bin").join(name)).unwrap() == fs::read(&native).unwrap(),
            "{name} must remain available from archive extraction through publication"
        );
    }
    assert_eq!(
        version_stamp::read_installed_version().unwrap().as_deref(),
        Some(selected.as_str())
    );
}

#[test]
fn an_archive_missing_required_hooks_fails_before_locking_or_profile_changes() {
    let native = native_release().join("release");
    let selected = older_than_binary();
    let _downloads = Downloads::release(&selected, &native, false);
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let _env = isolate(&home);
    let source = versioned_tree(&temp.path().join("source"), &selected);
    let before = snapshot(&home);

    let result = explicit(&source);

    assert_unchanged(&home, &before);
    assert!(
        result.is_err(),
        "missing archive companions cannot be taken from PATH or a checkout"
    );
}
