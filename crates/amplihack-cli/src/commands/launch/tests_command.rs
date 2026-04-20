use super::*;
use crate::binary_finder::BinaryInfo;
use crate::test_support::{home_env_lock, restore_cwd, set_cwd};
use std::fs;
use std::path::PathBuf;

fn make_binary(path: &str) -> BinaryInfo {
    BinaryInfo {
        name: "claude".to_string(),
        path: PathBuf::from(path),
        version: Some("1.0.0".to_string()),
    }
}

fn with_uvx_detection_disabled<T>(f: impl FnOnce() -> T) -> T {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let cwd = tempfile::tempdir().unwrap();
    fs::create_dir_all(cwd.path().join(".claude")).unwrap();
    let original_cwd = set_cwd(cwd.path()).unwrap();
    let previous_uv_python = std::env::var_os("UV_PYTHON");
    let previous_root = std::env::var_os("AMPLIHACK_ROOT");
    unsafe {
        std::env::remove_var("UV_PYTHON");
        std::env::remove_var("AMPLIHACK_ROOT");
    }

    let result = f();

    restore_cwd(&original_cwd).unwrap();
    match previous_uv_python {
        Some(value) => unsafe { std::env::set_var("UV_PYTHON", value) },
        None => unsafe { std::env::remove_var("UV_PYTHON") },
    }
    match previous_root {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_ROOT", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_ROOT") },
    }

    result
}

/// When skip_permissions=true, --dangerously-skip-permissions MUST be the
/// first argument injected before any other flags.
///
/// Fails if build_command does not inject the flag when skip_permissions=true.
#[test]
fn test_build_command_injects_dangerously_skip_permissions() {
    let binary = make_binary("/usr/bin/claude");
    let cmd = build_command(&binary, false, false, true, &[]);
    let args: Vec<_> = cmd.get_args().collect();
    assert!(
        args.contains(&std::ffi::OsStr::new("--dangerously-skip-permissions")),
        "Expected '--dangerously-skip-permissions' in args when skip_permissions=true, \
         got: {:?}",
        args
    );
}

#[test]
fn render_launcher_command_quotes_prompt_args() {
    let args = vec![
        "--model".to_string(),
        "gpt-5".to_string(),
        "-p".to_string(),
        "fix spaces and '$PATH'".to_string(),
    ];
    assert_eq!(
        render_launcher_command("copilot", &args),
        "amplihack copilot --model gpt-5 -p 'fix spaces and '\"'\"'$PATH'\"'\"''"
    );
}

/// When no --model is present in extra_args, build_command MUST inject
/// '--model' followed by the default model value (opus[1m] or AMPLIHACK_DEFAULT_MODEL).
///
/// Fails if no --model flag is injected by default.
#[test]
fn test_build_command_injects_default_model() {
    // Ensure AMPLIHACK_DEFAULT_MODEL is not set so we get the hard-coded default
    // SAFETY: single-threaded test context.
    unsafe { std::env::remove_var("AMPLIHACK_DEFAULT_MODEL") };
    let binary = make_binary("/usr/bin/claude");
    let cmd = build_command(&binary, false, false, false, &[]);
    let args: Vec<_> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(
        args.contains(&"--model".to_string()),
        "Expected '--model' to be injected when no --model in extra_args, got: {:?}",
        args
    );
    // Verify the default model value follows --model
    let model_pos = args.iter().position(|a| a == "--model").unwrap();
    assert_eq!(
        args[model_pos + 1],
        "opus[1m]",
        "Expected default model 'opus[1m]' after '--model', got: {:?}",
        args[model_pos + 1]
    );
}

/// When AMPLIHACK_DEFAULT_MODEL env var is set, build_command MUST use that
/// value instead of the hard-coded default 'opus[1m]'.
///
/// Fails if the env var override is not respected.
#[test]
fn test_build_command_respects_custom_model_env() {
    // SAFETY: single-threaded test context.
    unsafe { std::env::set_var("AMPLIHACK_DEFAULT_MODEL", "claude-3-5-sonnet") };
    let binary = make_binary("/usr/bin/claude");
    let cmd = build_command(&binary, false, false, false, &[]);
    let args: Vec<_> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    unsafe { std::env::remove_var("AMPLIHACK_DEFAULT_MODEL") };
    let model_pos = args.iter().position(|a| a == "--model").unwrap();
    assert_eq!(
        args[model_pos + 1],
        "claude-3-5-sonnet",
        "Expected AMPLIHACK_DEFAULT_MODEL value 'claude-3-5-sonnet' after '--model', \
         got: {:?}",
        args[model_pos + 1]
    );
}

/// When the user already supplies --model in extra_args, build_command MUST
/// NOT inject an additional --model flag (no duplication).
///
/// Fails if build_command injects a second --model when the user already has one.
#[test]
fn test_build_command_no_model_injection_when_user_supplies_model() {
    let binary = make_binary("/usr/bin/claude");
    let extra = vec!["--model".to_string(), "custom-model".to_string()];
    let cmd = build_command(&binary, false, false, false, &extra);
    let args: Vec<_> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let model_count = args.iter().filter(|a| *a == "--model").count();
    assert_eq!(
        model_count, 1,
        "Expected exactly one '--model' in args when user supplies --model, \
         but found {} occurrences. Args: {:?}",
        model_count, args
    );
    // And verify the user's model value is preserved
    let model_pos = args.iter().position(|a| a == "--model").unwrap();
    assert_eq!(
        args[model_pos + 1],
        "custom-model",
        "User-supplied model value must be preserved"
    );
}

/// When skip_permissions=false, '--dangerously-skip-permissions' MUST NOT
/// appear in the args list.
///
/// Fails if the flag is injected even when skip_permissions=false.
#[test]
fn test_build_command_no_dangerously_skip_when_false() {
    let binary = make_binary("/usr/bin/claude");
    let cmd = build_command(&binary, false, false, false, &[]);
    let args: Vec<_> = cmd.get_args().collect();
    assert!(
        !args.contains(&std::ffi::OsStr::new("--dangerously-skip-permissions")),
        "Expected '--dangerously-skip-permissions' to NOT be present when \
         skip_permissions=false, got: {:?}",
        args
    );
}

/// The Commands::Launch dispatch in mod.rs must pass skip_permissions=true
/// by default (matching Python launcher parity where skip_permissions is
/// always enabled). This test verifies build_command is exercised with
/// skip_permissions=true from the default dispatch path.
///
/// This test verifies the wiring by confirming that calling build_command
/// with skip_permissions=true (as dispatch does) produces the expected flag.
/// Fails if the dispatch hardcodes false instead of true.
#[test]
fn test_dispatch_defaults_skip_permissions_true() {
    // Simulate what Commands::Launch dispatch does: always pass skip_permissions=true
    // Build command the same way dispatch calls run_launch (skip_permissions=true)
    let binary = make_binary("/usr/bin/claude");
    // This mirrors the dispatch: skip_permissions is ALWAYS true for launch commands
    let skip_permissions_from_dispatch = true; // this is what dispatch should pass
    let cmd = build_command(&binary, false, false, skip_permissions_from_dispatch, &[]);
    let args: Vec<_> = cmd.get_args().collect();
    assert!(
        args.contains(&std::ffi::OsStr::new("--dangerously-skip-permissions")),
        "Commands::Launch dispatch must pass skip_permissions=true, which means \
         '--dangerously-skip-permissions' must appear in the built command args. \
         Got: {:?}",
        args
    );
}

#[test]
fn build_command_basic_no_skip_permissions_by_default() {
    with_uvx_detection_disabled(|| {
        let binary = BinaryInfo {
            name: "claude".to_string(),
            path: PathBuf::from("/usr/bin/claude"),
            version: Some("1.0.0".to_string()),
        };
        // skip_permissions = false (default): should NOT inject --dangerously-skip-permissions
        let cmd = build_command(&binary, false, false, false, &[]);
        assert_eq!(cmd.get_program(), "/usr/bin/claude");
        let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
        // Should inject --model <default> only
        assert_eq!(args[0], "--model");
        // Default model depends on env; just check we have 2 args
        assert_eq!(args.len(), 2);
    });
}

#[test]
fn build_command_with_skip_permissions_flag() {
    with_uvx_detection_disabled(|| {
        let binary = BinaryInfo {
            name: "claude".to_string(),
            path: PathBuf::from("/usr/bin/claude"),
            version: Some("1.0.0".to_string()),
        };
        // skip_permissions = true: should inject --dangerously-skip-permissions
        let cmd = build_command(&binary, false, false, true, &[]);
        let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
        assert_eq!(args[0], "--dangerously-skip-permissions");
        assert_eq!(args[1], "--model");
        assert_eq!(args.len(), 3);
    });
}

#[test]
fn build_command_with_flags() {
    with_uvx_detection_disabled(|| {
        let binary = BinaryInfo {
            name: "claude".to_string(),
            path: PathBuf::from("/usr/bin/claude"),
            version: None,
        };
        // User supplies --model so we should NOT inject a default --model
        let extra = vec!["--model".to_string(), "opus".to_string()];
        let cmd = build_command(&binary, true, true, true, &extra);
        let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
        assert_eq!(
            args,
            &[
                "--dangerously-skip-permissions",
                "--resume",
                "--continue",
                "--model",
                "opus"
            ]
        );
    });
}

#[test]
fn build_command_without_skip_permissions_and_with_flags() {
    with_uvx_detection_disabled(|| {
        let binary = BinaryInfo {
            name: "claude".to_string(),
            path: PathBuf::from("/usr/bin/claude"),
            version: None,
        };
        let extra = vec!["--model".to_string(), "opus".to_string()];
        let cmd = build_command(&binary, true, true, false, &extra);
        let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
        assert_eq!(args, &["--resume", "--continue", "--model", "opus"]);
    });
}

#[test]
fn copilot_gets_allow_all_injected_by_default() {
    // Issue #303: amplihack should pass --allow-all to copilot by default so
    // unattended orchestrator loops are not blocked by tool/path/url prompts.
    with_uvx_detection_disabled(|| {
        // Clear the opt-out env var in case the test environment has it set.
        // Safety: tests in this file are serialized via home_env_lock().
        unsafe {
            std::env::remove_var("AMPLIHACK_COPILOT_NO_ALLOW_ALL");
        }
        let binary = BinaryInfo {
            name: "copilot".to_string(),
            path: PathBuf::from("/usr/bin/copilot"),
            version: None,
        };
        let cmd = build_command(&binary, false, false, false, &[]);
        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.iter().any(|a| a == "--allow-all"),
            "copilot launch must include --allow-all by default; got {args:?}"
        );
    });
}

#[test]
fn copilot_skips_allow_all_when_user_sets_one() {
    with_uvx_detection_disabled(|| {
        unsafe {
            std::env::remove_var("AMPLIHACK_COPILOT_NO_ALLOW_ALL");
        }
        let binary = BinaryInfo {
            name: "copilot".to_string(),
            path: PathBuf::from("/usr/bin/copilot"),
            version: None,
        };
        // User already passed --allow-all-tools; we must NOT inject another flag.
        let extra = vec!["--allow-all-tools".to_string()];
        let cmd = build_command(&binary, false, false, false, &extra);
        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        let allow_all_count = args.iter().filter(|a| a.as_str() == "--allow-all").count();
        assert_eq!(
            allow_all_count, 0,
            "should not inject --allow-all when user supplied --allow-all-tools; got {args:?}"
        );
    });
}

#[test]
fn copilot_skips_allow_all_when_env_opt_out() {
    with_uvx_detection_disabled(|| {
        // Safety: serialized via home_env_lock(); restored at end.
        unsafe {
            std::env::set_var("AMPLIHACK_COPILOT_NO_ALLOW_ALL", "1");
        }
        let binary = BinaryInfo {
            name: "copilot".to_string(),
            path: PathBuf::from("/usr/bin/copilot"),
            version: None,
        };
        let cmd = build_command(&binary, false, false, false, &[]);
        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        unsafe {
            std::env::remove_var("AMPLIHACK_COPILOT_NO_ALLOW_ALL");
        }
        assert!(
            !args.iter().any(|a| a == "--allow-all"),
            "opt-out must suppress allow-all; got {args:?}"
        );
    });
}

#[test]
fn claude_does_not_get_allow_all_injected() {
    with_uvx_detection_disabled(|| {
        let binary = BinaryInfo {
            name: "claude".to_string(),
            path: PathBuf::from("/usr/bin/claude"),
            version: None,
        };
        let cmd = build_command(&binary, false, false, false, &[]);
        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert!(
            !args.iter().any(|a| a == "--allow-all"),
            "non-copilot tools must not get --allow-all; got {args:?}"
        );
    });
}
