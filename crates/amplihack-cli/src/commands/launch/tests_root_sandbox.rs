//! Issue #1482: the `amplihack claude` launch path — the one recipe agent steps
//! use — decides on IS_SANDBOX before spawning and applies it to the child only.

use super::*;
use crate::binary_finder::BinaryInfo;
use amplihack_utils::root_sandbox::{IS_SANDBOX_ENV, SKIP_PERMISSIONS_FLAG, SkipPermissionsEnv};
use std::path::PathBuf;

fn claude_binary() -> BinaryInfo {
    BinaryInfo {
        name: "claude".to_string(),
        path: PathBuf::from("/usr/bin/claude"),
        version: Some("1.0.0".to_string()),
    }
}

fn child_is_sandbox(command: &std::process::Command) -> Option<String> {
    command
        .get_envs()
        .find(|(key, _)| *key == IS_SANDBOX_ENV)
        .and_then(|(_, value)| value)
        .map(|value| value.to_string_lossy().into_owned())
}

#[test]
fn root_in_a_sandbox_sets_is_sandbox_on_the_launched_claude_only() {
    let decision =
        root_sandbox_for_launch("claude", true, &[], || SkipPermissionsEnv::SetSandbox {
            signal: "/.dockerenv",
        })
        .unwrap()
        .expect("claude with the flag needs a decision");

    let parent_before = std::env::var_os(IS_SANDBOX_ENV);
    let mut cmd = build_command(&claude_binary(), false, false, true, &[]);
    assert!(
        cmd.get_args().any(|arg| arg == SKIP_PERMISSIONS_FLAG),
        "the launch passes the flag"
    );
    decision.apply(&mut cmd).unwrap();

    assert_eq!(child_is_sandbox(&cmd).as_deref(), Some("1"));
    assert_eq!(
        std::env::var_os(IS_SANDBOX_ENV),
        parent_before,
        "amplihack's own environment is not modified"
    );
}

#[test]
fn root_outside_a_sandbox_stops_the_launch_naming_is_sandbox() {
    for decision in [
        SkipPermissionsEnv::RootOutsideSandbox,
        SkipPermissionsEnv::ExplicitlyNotSandboxed {
            value: "0".to_string(),
        },
    ] {
        let error = root_sandbox_for_launch("claude", true, &[], || decision.clone())
            .expect_err("the launch must stop before spawning");
        assert!(error.to_string().contains("IS_SANDBOX=1"), "{error}");
    }
}

#[test]
fn a_user_passed_flag_is_covered_too() {
    let args = vec![SKIP_PERMISSIONS_FLAG.to_string(), "-p".to_string()];
    let error = root_sandbox_for_launch("claude", false, &args, || {
        SkipPermissionsEnv::RootOutsideSandbox
    })
    .expect_err("the flag in claude_args counts");
    assert!(error.to_string().contains("IS_SANDBOX=1"), "{error}");
}

#[test]
fn launches_without_the_flag_or_for_other_tools_do_not_probe() {
    let probe = || -> SkipPermissionsEnv { panic!("must not probe for this launch") };
    assert_eq!(
        root_sandbox_for_launch("claude", false, &[], probe).unwrap(),
        None
    );
    for tool in ["copilot", "codex", "amplifier"] {
        assert_eq!(
            root_sandbox_for_launch(tool, true, &[], probe).unwrap(),
            None
        );
    }
}

#[test]
fn non_root_and_explicit_sandbox_leave_the_child_environment_alone() {
    for decision in [
        SkipPermissionsEnv::NotRoot,
        SkipPermissionsEnv::AlreadySandboxed,
    ] {
        let decision = root_sandbox_for_launch("claude", true, &[], || decision.clone())
            .unwrap()
            .expect("claude with the flag needs a decision");
        let mut cmd = build_command(&claude_binary(), false, false, true, &[]);
        decision.apply(&mut cmd).unwrap();
        assert_eq!(child_is_sandbox(&cmd), None);
    }
}

// --- the calls in run_launch_with -------------------------------------------

#[test]
fn launch_environment_puts_is_sandbox_on_the_child_last() {
    let set_sandbox = SkipPermissionsEnv::SetSandbox {
        signal: "/.dockerenv",
    };
    // Even an inherited or builder-set value is overridden by the decision.
    let mut cmd = std::process::Command::new("claude");
    apply_launch_environment(
        &mut cmd,
        EnvBuilder::new().set(IS_SANDBOX_ENV, "0"),
        None,
        Some(&set_sandbox),
    )
    .unwrap();
    assert_eq!(child_is_sandbox(&cmd).as_deref(), Some("1"));

    for decision in [None, Some(&SkipPermissionsEnv::NotRoot)] {
        let mut cmd = std::process::Command::new("claude");
        apply_launch_environment(&mut cmd, EnvBuilder::new(), None, decision).unwrap();
        assert_eq!(child_is_sandbox(&cmd), None, "{decision:?}");
    }

    let mut cmd = std::process::Command::new("claude");
    let error = apply_launch_environment(
        &mut cmd,
        EnvBuilder::new(),
        None,
        Some(&SkipPermissionsEnv::RootOutsideSandbox),
    )
    .expect_err("a refusal is never applied");
    assert!(error.to_string().contains("IS_SANDBOX=1"), "{error}");
}

/// A sealed launch environment for `run_launch_with("claude", ..)`: `PATH`
/// holds only a fake `claude` (plus the system shell tools it needs), which
/// records the `IS_SANDBOX` it inherited in `$HOME/child-env` and exits 0;
/// auto-install is off, `HOME` and the cwd are a temp dir, and the variables
/// that would change the decision or hand the launch off are unset. So even a
/// regression can never reach the real Claude Code CLI.
#[cfg(unix)]
struct FakeClaudeLaunch {
    home: tempfile::TempDir,
    _env: crate::test_support::EnvGuard,
    _home: crate::test_support::HomeGuard,
    _cwd: crate::test_support::CwdGuard,
}

#[cfg(unix)]
impl FakeClaudeLaunch {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;

        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let claude = bin.join("claude");
        std::fs::write(
            &claude,
            "#!/bin/sh\n\
             if [ \"$1\" = --version ]; then printf '2.1.282\\n'; exit 0; fi\n\
             printf '%s' \"${IS_SANDBOX-unset}\" > \"$HOME/child-env\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = format!("{}:/usr/bin:/bin", bin.display());
        let claude_text = claude.to_string_lossy().into_owned();
        let env = crate::test_support::EnvGuard::set([
            ("PATH", path.as_str()),
            ("AMPLIHACK_CLAUDE_BINARY_PATH", claude_text.as_str()),
            ("AMPLIHACK_SKIP_AUTO_INSTALL", "1"),
            ("AMPLIHACK_NONINTERACTIVE", "1"),
            (IS_SANDBOX_ENV, ""),
            ("CLAUDE_CODE_BUBBLEWRAP", ""),
            ("AMPLIHACK_USE_DOCKER", ""),
            ("AMPLIHACK_PROMPT_DELIVERY", ""),
            (amplihack_utils::litellm_proxy::ENDPOINT_ENV, ""),
            (amplihack_utils::litellm_proxy::API_KEY_ENV, ""),
            (amplihack_utils::litellm_proxy::MODEL_ENV, ""),
        ]);
        // Unset rather than empty; the guard restores the previous values.
        for name in [
            IS_SANDBOX_ENV,
            "CLAUDE_CODE_BUBBLEWRAP",
            "AMPLIHACK_USE_DOCKER",
            "AMPLIHACK_PROMPT_DELIVERY",
            amplihack_utils::litellm_proxy::ENDPOINT_ENV,
            amplihack_utils::litellm_proxy::API_KEY_ENV,
            amplihack_utils::litellm_proxy::MODEL_ENV,
        ] {
            unsafe { std::env::remove_var(name) };
        }
        let home_guard = crate::test_support::HomeGuard::set(home.path());
        let cwd = crate::test_support::CwdGuard::set(home.path()).unwrap();
        Self {
            home,
            _env: env,
            _home: home_guard,
            _cwd: cwd,
        }
    }

    fn launch(&self, decision: fn() -> SkipPermissionsEnv) -> Result<()> {
        run_launch_with(
            "claude",
            "claude",
            false,
            false,
            false,
            true,
            true,
            false,
            true,
            None,
            Vec::new(),
            amplihack_utils::launch_target::OverrideOrigin::User,
            None,
            decision,
        )
    }

    /// What the fake `claude` saw, or `None` when it never ran.
    fn child_is_sandbox(&self) -> Option<String> {
        std::fs::read_to_string(self.home.path().join("child-env")).ok()
    }
}

/// The launched `claude` gets `IS_SANDBOX=1` when amplihack enables it, and
/// nothing when it does not: the decision reaches the spawned child.
#[cfg(unix)]
#[test]
fn claude_launch_passes_the_decision_to_the_spawned_child() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for (decision, expected) in [
        (
            (|| SkipPermissionsEnv::SetSandbox {
                signal: "/.dockerenv",
            }) as fn() -> SkipPermissionsEnv,
            "1",
        ),
        (
            || SkipPermissionsEnv::NormalizeExplicit {
                value: "yes".to_string(),
            },
            "1",
        ),
        (|| SkipPermissionsEnv::NotRoot, "unset"),
        (|| SkipPermissionsEnv::AlreadySandboxed, "unset"),
    ] {
        let launch = FakeClaudeLaunch::new();
        launch
            .launch(decision)
            .expect("the fake claude launches and exits 0");
        assert_eq!(
            launch.child_is_sandbox().as_deref(),
            Some(expected),
            "{:?}",
            decision()
        );
    }
}

/// The decision in `run_launch_with` stops `amplihack claude` before anything
/// else runs (update check, bootstrap, binary lookup, spawn). Under the fake
/// launch, a regression reaches only the fake `claude`, which then records it.
#[cfg(unix)]
#[test]
fn claude_launch_stops_up_front_when_claude_code_would_refuse() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for decision in [
        (|| SkipPermissionsEnv::RootOutsideSandbox) as fn() -> SkipPermissionsEnv,
        || SkipPermissionsEnv::ExplicitlyNotSandboxed {
            value: "0".to_string(),
        },
    ] {
        let launch = FakeClaudeLaunch::new();
        let error = launch
            .launch(decision)
            .expect_err("the launch must stop before spawning");
        assert!(format!("{error:#}").contains("IS_SANDBOX=1"), "{error:#}");
        assert_eq!(launch.child_is_sandbox(), None, "claude must not start");
    }
}
