//! A pipe keeps the competing parent environment active throughout installation.

use super::{helpers, issue_3416_release_preflight::snapshot, local_install};
use crate::test_support::{EnvGuard, env_lock, install_process, isolated_install_test};
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Stdio;

#[test]
fn install_child_ignores_competing_home_and_inherited_destinations() {
    const ASSIGNED: &str = "AMPLIHACK_ISOLATION_ASSIGNED_HOME";
    if let Some(assigned) = std::env::var_os(ASSIGNED) {
        let assigned = PathBuf::from(assigned);
        let source = assigned.join("source");
        helpers::create_bundle_only_source_repo(&source);
        let bin = assigned.join("stub-bin");
        let hooks = helpers::create_exe_stub(&bin, "amplihack-hooks");
        let runner = helpers::create_exe_stub(&bin, "recipe-runner-rs");
        let codex = helpers::create_exe_stub(&bin, "codex");
        let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let mut env = EnvGuard::set([
            ("AMPLIHACK_AGENT_BINARY", "codex"),
            ("AMPLIHACK_SKIP_MMDC", "1"),
            ("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL", "1"),
        ]);
        env.assign("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", hooks);
        env.assign("RECIPE_RUNNER_RS_PATH", runner);
        env.assign("AMPLIHACK_CODEX_BINARY_PATH", codex);
        env.assign("PATH", format!("{}:/usr/bin:/bin", bin.display()));
        assert!(std::env::var_os("AMPLIHACK_HOME").is_none());
        assert!(std::env::var_os("CODEX_HOME").is_none());
        // No destination is set here. All inherited destinations must have
        // been cleared, and HOME must already be correct at process start.
        let mut signal = [0];
        std::io::stdin().read_exact(&mut signal).unwrap();
        local_install(&source, None).unwrap();
        return;
    }

    let root = tempfile::tempdir().unwrap();
    let sentinel = root.path().join("sentinel");
    fs::create_dir(&sentinel).unwrap();
    fs::write(sentinel.join("keep"), "sentinel contents\n").unwrap();
    for relative in [
        ".amplihack/codex/market/plugin/bin/amplihack-hooks",
        ".codex/hooks.json",
        ".codex/config.toml",
        ".codex/plugins/cache/existing/skills/SKILL.md",
        "hooks.json",
        "config.toml",
    ] {
        let path = sentinel.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "sentinel contents\n").unwrap();
    }
    let before = snapshot(&sentinel);
    let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let mut env = EnvGuard::capture(&[]);
    env.assign("HOME", &sentinel);
    for key in [
        "AMPLIHACK_HOME",
        "CLAUDE_CONFIG_DIR",
        "CODEX_HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "CARGO_HOME",
        "XDG_CACHE_HOME",
    ] {
        env.assign(key, &sentinel);
    }
    let thread = std::thread::current();
    let name = thread.name().unwrap();
    for remove_home in [false, true] {
        let home = root
            .path()
            .join(if remove_home { "unset" } else { "changed" });
        fs::create_dir(&home).unwrap();
        let mut child = install_process::command(&home, name)
            .env(ASSIGNED, &home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if remove_home {
            env.clear("HOME");
        } else {
            env.assign("HOME", &sentinel);
        }
        child.stdin.take().unwrap().write_all(&[1]).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(home.join(".amplihack/CLAUDE.md").is_file());
        assert!(home.join(".amplihack/amplifier-bundle").is_dir());
        assert!(home.join(".claude/settings.json").is_file());
        let package = home.join(".amplihack/codex/market/plugin");
        assert!(package.join("plugin.json").is_file());
        assert!(package.join("bin/amplihack-hooks").is_file());
        assert!(package.join("skills/default-workflow/SKILL.md").is_file());
        assert!(home.join(".codex/config.toml").is_file());
        assert!(home.join(".codex/hooks.json").is_file());
        assert_eq!(snapshot(&sentinel), before);
    }
}

#[test]
fn install_helpers_restore_home_and_path_after_panic() {
    if isolated_install_test() {
        return;
    }
    for fixture in [false, true] {
        let before = {
            let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
            (std::env::var_os("HOME"), std::env::var_os("PATH"))
        };
        let result = std::panic::catch_unwind(|| {
            if fixture {
                super::issue_527_tests::with_install_env(|_, _| panic!("fixture panic"));
            } else {
                super::install_flow::with_install_env(|_| panic!("fixture panic"));
            }
        });
        assert!(result.is_err());
        let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!((std::env::var_os("HOME"), std::env::var_os("PATH")), before);
    }
}

#[test]
fn shared_home_helper_restores_home_and_profiles_after_panic() {
    if isolated_install_test() {
        return;
    }
    let original = tempfile::tempdir().unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let mut env = EnvGuard::capture(&[]);
    struct FixturePanic;
    for missing_home in [false, true] {
        if missing_home {
            env.clear("HOME");
        } else {
            env.assign("HOME", original.path());
        }
        let names = ["HOME", "CLAUDE_CONFIG_DIR", "AMPLIHACK_HOME", "CODEX_HOME"];
        for name in &names[1..] {
            env.assign(name, original.path());
        }
        let before: Vec<_> = names.map(std::env::var_os).into();
        let result = std::panic::catch_unwind(|| {
            let _home = crate::test_support::set_home(fixture.path());
            assert_eq!(
                std::env::var_os("HOME").as_deref(),
                Some(fixture.path().as_os_str())
            );
            for name in &names[1..] {
                assert!(std::env::var_os(name).is_none());
            }
            std::panic::panic_any(FixturePanic);
        });
        assert!(result.unwrap_err().is::<FixturePanic>());
        assert_eq!(names.map(std::env::var_os).to_vec(), before);
    }
}

#[test]
fn install_fixture_helpers_clear_and_restore_inherited_sources_and_profiles() {
    if isolated_install_test() {
        return;
    }
    let sentinel = tempfile::tempdir().unwrap();
    let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let names = crate::test_support::claude_profile_isolation::CLAUDE_PROFILE_ENV_VARS;
    let mut inherited = EnvGuard::capture(&[]);
    for name in names {
        inherited.assign(name, sentinel.path());
    }
    let helpers = [
        super::install_flow::fixture_environment,
        super::issue_527_tests::fixture_environment,
        super::claude_command_staging_tests::fixture_environment,
        super::issue_1438_skill_publication::fixture_environment,
        super::hook_staging_tests::fixture_environment,
    ];
    for helper in helpers {
        {
            let _fixture = helper();
            for name in names {
                assert!(std::env::var_os(name).is_none());
            }
        }
        for name in names {
            assert_eq!(
                std::env::var_os(name).as_deref(),
                Some(sentinel.path().as_os_str())
            );
        }
    }
}
