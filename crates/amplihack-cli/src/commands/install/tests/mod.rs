use super::*;

mod binary_tests;
mod claude_command_staging_tests;
mod deploy_binary_tests;
mod helpers;
// Issue #2279: the single source of amplihack's hooks block.
mod hook_block_drift;
#[cfg(unix)]
mod hook_block_file;
mod hook_block_source;
mod hook_block_target;
mod hook_block_xpia;
mod hook_specs;
mod hook_staging_tests;
mod install_flow;
mod issue_1266_restage_loop;
mod issue_1438_skill_publication;
mod issue_1449_partial_registration;
mod issue_1526_implicit_downgrade;
mod issue_1526_implicit_mode;
mod issue_1526_launch_bootstrap;
#[cfg(unix)]
mod issue_1745_launch_repair_statusline;
// Issue #1699: the real-profile-root guard also runs in this binary, where
// the in-process install tests run.
#[cfg(unix)]
#[path = "../../../../tests/issue_1699_real_root_guard.rs"]
mod issue_1699_real_root_guard;
#[cfg(unix)]
mod issue_1954_install_lock;
#[cfg(unix)]
mod issue_1954_release_layout;
mod issue_1955_fetch_backoff;
mod issue_1955_force_refresh;
mod issue_1955_manifest;
#[cfg(unix)]
mod issue_1955_modes;
mod issue_1955_self_heal;
mod issue_1955_source_gate;
mod issue_1955_support;
#[cfg(unix)]
mod issue_1955_tag_fetch;
#[cfg(unix)]
mod issue_2279_launch_repair;
mod issue_527_tests;
mod issue_911_durable_hooks_path;
#[cfg(unix)]
mod limit_resume_install;
mod mermaid_cli_tests;
mod output_regression_tests;
mod path_conflict_tests;
mod path_precedence_tests;
#[cfg(unix)]
mod profile_refresh_install;
mod same_path_tests;
mod settings_tests;
mod stale_wrapper_repair;
mod uninstall_tests;
mod wrapper_tests;

mod interactive_tests;

#[cfg(unix)]
mod codex_alias_paths;
#[cfg(unix)]
mod codex_disabled_rollback;
#[cfg(unix)]
mod codex_failure_recovery;
#[cfg(unix)]
mod codex_plugin_tests;
#[cfg(unix)]
mod codex_runner_delivery;
#[cfg(unix)]
mod native_registration_fixture;

/// Issue #2496: a checkout's newer version is not authority to repair an install.
/// These exercise existing entry points, so failures are behavioural rather
/// than compilation failures against a proposed eligibility API.
#[cfg(unix)]
mod issue_2496 {
    use super::super::*;
    use super::helpers::{create_exe_stub, create_minimal_staged_assets};
    use super::issue_1526_implicit_downgrade::{IsolatedHome, args, changes, snapshot};
    use super::issue_1955_support::{
        FakeFetcher, newer_than_binary, older_than_binary, versioned_tree,
    };
    use std::cell::Cell;

    fn isolate(home: &Path) -> IsolatedHome {
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

    fn assert_unchanged(home: &Path, before: &std::collections::BTreeMap<PathBuf, Vec<u8>>) {
        assert_eq!(
            changes(before, &snapshot(home)),
            Vec::<String>::new(),
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
        let fetcher =
            FakeFetcher::new(|_| panic!("origin refusal must precede source acquisition"));
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
    fn checkout_is_not_authorised_to_publish_secondary_installation_assets() {
        let temp = crate::test_support::tempdir_outside_framework_roots();
        let _env = isolate(temp.path());
        version_stamp::write_installed_version(crate::VERSION).unwrap();
        assert!(
            !binary_is_current(),
            "version equality alone must not authorise implicit hook and provider writers"
        );
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
}
