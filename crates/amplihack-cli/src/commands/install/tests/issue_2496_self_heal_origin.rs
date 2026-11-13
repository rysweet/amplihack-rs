//! Issue #2496: a checkout's newer version is not authority to repair an
//! install. These exercise existing entry points, so failures are behavioural
//! rather than compilation failures against a proposed eligibility API.

use super::super::*;
use super::helpers::{create_exe_stub, create_minimal_staged_assets};
use super::issue_1526_implicit_downgrade::{IsolatedHome, args};
use super::issue_1955_support::{
    FakeFetcher, newer_than_binary, older_than_binary, versioned_tree,
};
use std::cell::Cell;

pub(super) fn isolate(home: &Path) -> IsolatedHome {
    let mut env = IsolatedHome::new(home);
    // Recipe TMPDIRs can themselves be under the live installation. Use
    // this fixture's clean scratch base for child state and the HOME guard.
    env.set("TMPDIR", home.parent().unwrap());
    for (key, dir) in [
        ("AMPLIHACK_CLAUDE_PROFILE_ROOT", "profiles"),
        ("CLAUDE_CONFIG_DIR", ".claude"),
        ("CODEX_HOME", ".codex"),
        ("XDG_CONFIG_HOME", ".config"),
        ("XDG_CACHE_HOME", ".cache"),
    ] {
        env.set(key, home.join(dir));
    }
    env.set("AMPLIHACK_TEST_HOME_GUARD", "1");
    env.set("AMPLIHACK_ROTATE_AUTOSTART", "0");
    env.set("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL", "1");
    // Reuse the existing executable fixtures; never discover installed
    // hooks or a runner through the developer's ambient PATH.
    let bin = home.join("fixture-bin");
    let hooks = create_exe_stub(&bin, "amplihack-hooks");
    let runner = create_exe_stub(&bin, "recipe-runner-rs");
    env.set("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", hooks);
    env.set("RECIPE_RUNNER_RS_PATH", runner);
    env.set(
        "PATH",
        std::env::join_paths([bin, PathBuf::from("/usr/bin"), PathBuf::from("/bin")]).unwrap(),
    );
    env
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Entry {
    Directory(u32),
    File(u32, Vec<u8>),
    Link(PathBuf),
}

pub(super) fn snapshot(root: &Path) -> std::collections::BTreeMap<PathBuf, Entry> {
    use std::os::unix::fs::PermissionsExt;
    fn walk(root: &Path, dir: &Path, out: &mut std::collections::BTreeMap<PathBuf, Entry>) {
        for item in fs::read_dir(dir).unwrap() {
            let path = item.unwrap().path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            let mode = metadata.permissions().mode() & 0o7777;
            let entry = if metadata.file_type().is_symlink() {
                Entry::Link(fs::read_link(&path).unwrap())
            } else if metadata.is_dir() {
                Entry::Directory(mode)
            } else {
                assert!(metadata.is_file(), "unexpected fixture entry: {path:?}");
                Entry::File(mode, fs::read(&path).unwrap())
            };
            out.insert(path.strip_prefix(root).unwrap().to_owned(), entry);
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                walk(root, &path, out);
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk(root, root, &mut out);
    out
}

pub(super) fn assert_unchanged(home: &Path, before: &std::collections::BTreeMap<PathBuf, Entry>) {
    let after = snapshot(home);
    let changed: Vec<_> = before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .collect();
    assert_eq!(
        changed,
        Vec::<&PathBuf>::new(),
        "refusal must precede cache, lock, source and installation writes"
    );
}

#[test]
fn checkout_self_heal_refuses_before_cache_lock_or_installer() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path();
    let _env = isolate(home);
    create_minimal_staged_assets(home);
    version_stamp::write_installed_version(&older_than_binary()).unwrap();
    let before = snapshot(home);
    let calls = Cell::new(0);
    let mut notice = Vec::new();

    let result = crate::self_heal::ensure_assets_match_binary_version_with(
        &args(&["amplihack", "recipe", "list"]),
        &mut notice,
        || {
            calls.set(calls.get() + 1);
            Ok(())
        },
    );

    result.expect("refusing repair must still allow command dispatch");
    assert_eq!(calls.get(), 0, "checkout must never reach the installer");
    assert_unchanged(home, &before);
}

#[test]
fn satisfied_checkout_self_heal_is_quiet_and_does_not_prime_cache() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path();
    let _env = isolate(home);
    create_minimal_staged_assets(home);
    version_stamp::write_installed_version(crate::VERSION).unwrap();
    let before = snapshot(home);
    let mut notice = Vec::new();

    crate::self_heal::ensure_assets_match_binary_version_with(
        &args(&["amplihack", "recipe", "list"]),
        &mut notice,
        || panic!("a satisfied install requires no repair"),
    )
    .unwrap();

    assert!(notice.is_empty(), "{notice:?}");
    assert_unchanged(home, &before);
}

#[test]
fn matching_stamp_does_not_authorise_checkout_repair_of_an_incompatible_bundle() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path();
    let _env = isolate(home);
    create_minimal_staged_assets(home);
    version_stamp::write_installed_version(crate::VERSION).unwrap();
    fs::write(
        home.join(".amplihack/amplifier-bundle/recipes/smart-orchestrator.yaml"),
        "steps: [invalid recipe",
    )
    .unwrap();
    let before = snapshot(home);
    let calls = Cell::new(0);

    crate::self_heal::ensure_assets_match_binary_version_with(
        &args(&["amplihack", "recipe", "list"]),
        &mut Vec::new(),
        || {
            calls.set(calls.get() + 1);
            Ok(())
        },
    )
    .expect("refusing repair leaves ordinary command validation to dispatch");

    assert_eq!(
        calls.get(),
        0,
        "an incompatible bundle is no authority to self-install"
    );
    assert_unchanged(home, &before);
}

#[test]
fn opted_out_compatibility_assessment_is_read_only() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path();
    let mut env = isolate(home);
    env.set("AMPLIHACK_SKIP_AUTO_INSTALL", "1");
    create_minimal_staged_assets(home);
    version_stamp::write_installed_version(&older_than_binary()).unwrap();
    fs::write(
        home.join(".amplihack/.bundle-compat-cache.json"),
        b"stale cache sentinel\n",
    )
    .unwrap();
    let before = snapshot(home);
    let mut notice = Vec::new();

    crate::self_heal::ensure_assets_match_binary_version_with(
        &args(&["amplihack", "launch"]),
        &mut notice,
        || panic!("the opt-out forbids installation"),
    )
    .unwrap();

    assert_unchanged(home, &before);
    assert!(
        String::from_utf8(notice)
            .unwrap()
            .contains("AMPLIHACK_SKIP_AUTO_INSTALL")
    );
}

#[test]
fn launch_bootstrap_from_checkout_refuses_missing_assets_without_writing() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let mut env = isolate(&home);
    let source = versioned_tree(&temp.path().join("source"), crate::VERSION);
    env.set("AMPLIHACK_HOME", &source);
    version_stamp::write_installed_version(&older_than_binary()).unwrap();
    let before = snapshot(&home);
    let mut notice = Vec::new();

    let result = ensure_framework_installed_with(&mut notice);

    assert_unchanged(&home, &before);
    result.expect("bootstrap refusal leaves normal asset validation to the caller");
}

#[test]
fn checkout_with_assets_cannot_top_up_commands_or_reconcile_hooks() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let mut env = isolate(&home);
    let source = versioned_tree(&temp.path().join("source"), crate::VERSION);
    env.set("AMPLIHACK_HOME", source);
    create_minimal_staged_assets(&home);
    version_stamp::write_installed_version(crate::VERSION).unwrap();
    fs::create_dir(home.join(".claude")).unwrap();
    fs::write(
        home.join(".claude/settings.json"),
        b"{\"model\":\"sentinel\"}\n",
    )
    .unwrap();
    fs::create_dir(home.join(".codex")).unwrap();
    fs::write(home.join(".codex/config.toml"), b"model = \"sentinel\"\n").unwrap();
    let before = snapshot(&home);

    let result = ensure_framework_installed_with(&mut Vec::new());

    assert_unchanged(&home, &before);
    result.expect("usable assets permit the checkout command to continue");
}

#[test]
fn lower_level_implicit_install_refuses_before_source_acquisition() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let mut env = isolate(&home);
    // A recognised but mismatching source forces the fetch route if the
    // provenance check is missing. No network or timing is involved.
    let source = versioned_tree(&temp.path().join("source"), &older_than_binary());
    env.set("AMPLIHACK_HOME", source);
    let fetcher = FakeFetcher::new(|_| panic!("origin refusal must precede source acquisition"));
    let before = snapshot(&home);

    let result = run_install_with_fetcher(
        None,
        false,
        false,
        false,
        InstallMode::Implicit,
        &framework_source::BinaryIdentity {
            // Outrank every real checkout fallback, so source resolution
            // must reach our fetch tripwire if origin enforcement is late.
            version: newer_than_binary(),
            may_fetch: true,
        },
        &fetcher,
    );

    assert!(
        result.is_err(),
        "direct implicit installation must refuse a checkout"
    );
    assert_eq!(fetcher.call_count(), 0);
    assert_unchanged(&home, &before);
}

#[test]
fn secondary_implicit_local_install_refuses_before_taking_the_lock() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let _env = isolate(&home);
    let source = versioned_tree(&temp.path().join("source"), crate::VERSION);
    let before = snapshot(&home);

    let result = local_install_scoped(&source, None, false, InstallMode::Implicit);

    assert_unchanged(&home, &before);
    assert!(
        result.is_err(),
        "a direct implicit caller must refuse checkout provenance"
    );
}

#[test]
fn eligible_opted_out_install_refuses_before_acquisition_or_locking() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let mut env = isolate(&home);
    let _origin = release_layout::TestEligibility::installed();
    env.set("AMPLIHACK_SKIP_AUTO_INSTALL", "1");
    let fetcher = FakeFetcher::new(|_| panic!("the opt-out must precede source acquisition"));
    let before = snapshot(&home);

    let result = run_install_with_fetcher(
        None,
        false,
        false,
        false,
        InstallMode::Implicit,
        &framework_source::BinaryIdentity {
            version: newer_than_binary(),
            may_fetch: true,
        },
        &fetcher,
    );

    let error = result.expect_err("eligible origin cannot override automatic-install suppression");
    assert!(format!("{error:#}").contains("AMPLIHACK_SKIP_AUTO_INSTALL"));
    assert_eq!(fetcher.call_count(), 0);
    assert_unchanged(&home, &before);
}

#[test]
fn lower_level_binary_deployment_refuses_checkout_before_allocating_a_release() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let _env = isolate(&home);
    version_stamp::write_installed_version(&older_than_binary()).unwrap();
    let before = snapshot(&home);

    let result = binary::deploy_binaries(InstallMode::Implicit, Some(&older_than_binary()));

    assert!(result.is_err(), "a newer checkout is still ineligible");
    assert_unchanged(&home, &before);
}

#[test]
fn lower_level_binary_deployment_honours_opt_out_for_an_eligible_origin() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let mut env = isolate(&home);
    let _origin = release_layout::TestEligibility::installed();
    env.set("AMPLIHACK_SKIP_AUTO_INSTALL", "1");
    let before = snapshot(&home);

    let result = binary::deploy_binaries(InstallMode::Implicit, None);

    assert!(
        result.is_err(),
        "eligible origin cannot override the opt-out"
    );
    assert_unchanged(&home, &before);
}
