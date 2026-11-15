//! Issue #1745 — the launch repair writes the `statusLine` entry.
//!
//! `ensure_framework_installed_with` runs before every launch. When
//! `~/.claude/settings.json` has no amplihack hooks it rewrites the file, and
//! it passes the home status-line command to `ensure_settings_json`
//! (`install/mod.rs`). These tests go through that function, not through
//! `ensure_settings_json` directly, so a change that stops the launch repair
//! from passing the command fails here.

use super::helpers::*;
use super::issue_1526_implicit_downgrade::IsolatedHome;
use super::*;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

/// The command the launch repair should write with nothing deployed: the
/// running binary, as `stable_self_binary` resolves it.
fn expected_command() -> String {
    let exe = crate::commands::profile::statusline_binary()
        .expect("the test binary must pass the status-line ownership check");
    crate::commands::profile::home_statusline_command(&exe).unwrap()
}

/// Stage a complete framework under `home`, write `settings` (no amplihack
/// hooks) and run the launch repair. Returns the settings file afterwards.
fn repair(home: &Path, settings: Value) -> Value {
    let fixture = crate::test_support::tempdir_outside_framework_roots();
    let hooks_stub = create_exe_stub(&fixture.path().join("stub_bin"), "amplihack-hooks");
    let mut env = IsolatedHome::installed(home);
    env.set("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_stub);
    env.set("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL", "1");

    create_minimal_staged_assets(home);
    version_stamp::write_installed_version(crate::VERSION).unwrap();
    let path = home.join(".claude/settings.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, settings.to_string()).unwrap();
    let target = hook_block::HookTarget {
        bin: hooks_stub.clone(),
        limit_resume: true,
    };
    assert!(
        matches!(
            hook_block::drift_file(&path, &target),
            hook_block::FileDrift::Drift(_)
        ),
        "the fixture must send the launch down the repair path"
    );

    crate::freshness::with_provider("claude", || {
        ensure_framework_installed_with(&mut Vec::new()).expect("the launch repair must succeed")
    });
    assert_eq!(
        hook_block::drift_file(&path, &target),
        hook_block::FileDrift::Clean,
        "the launch repair did not run"
    );
    serde_json::from_slice(&fs::read(&path).unwrap()).unwrap()
}

#[test]
fn the_launch_repair_writes_the_status_line_when_absent() {
    let tmp = crate::test_support::tempdir_outside_framework_roots();
    let after = repair(tmp.path(), json!({"model": "opus"}));
    assert_eq!(
        after["statusLine"],
        json!({"type": "command", "command": expected_command()}),
        "{after}"
    );
    assert_eq!(after["model"], "opus");
}

/// A home in the trust walk's reach: `/tmp` is writable by others, so a
/// binary under it would be refused. The test binary's own directory is not.
fn trusted_home() -> tempfile::TempDir {
    let exe = std::env::current_exe().unwrap();
    tempfile::tempdir_in(exe.parent().unwrap()).unwrap()
}

/// Issue #911 for the status line: a deployed copy is what a durable entry
/// records, so cleaning the build directory leaves the line working. An
/// entry left by a build directory is replaced by it.
#[test]
fn the_launch_repair_records_the_deployed_binary_not_the_running_one() {
    for settings in [
        json!({"model": "opus"}),
        json!({"statusLine": {
            "type": "command",
            "command": "'/gone/target/release/amplihack' profile statusline",
        }}),
    ] {
        let tmp = trusted_home();
        let deployed = create_exe_stub(&tmp.path().join(".local/bin"), "amplihack");
        let after = repair(tmp.path(), settings);
        let want =
            crate::commands::profile::home_statusline_command(&deployed.canonicalize().unwrap())
                .unwrap();
        assert_ne!(want, expected_command());
        assert_eq!(after["statusLine"]["command"], want.as_str(), "{after}");
    }
}

#[test]
fn a_deployed_binary_other_users_can_replace_is_refused_not_passed_over() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = trusted_home();
    let deployed = create_exe_stub(&tmp.path().join(".local/bin"), "amplihack");
    fs::set_permissions(&deployed, fs::Permissions::from_mode(0o777)).unwrap();
    let after = repair(tmp.path(), json!({"model": "opus"}));
    assert_eq!(after.get("statusLine"), None, "{after}");
}

#[test]
fn a_build_output_is_warned_about_and_a_deployed_path_is_not() {
    let warning = binary::build_output_warning(Path::new("/w/target/release/amplihack"));
    assert!(warning.is_some_and(|w| w.contains("build output")));
    let deployed = Path::new("/home/me/.local/bin/amplihack");
    assert_eq!(binary::build_output_warning(deployed), None);
}

#[test]
fn the_launch_repair_keeps_a_users_own_status_line() {
    let tmp = crate::test_support::tempdir_outside_framework_roots();
    let mine = json!({"type": "command", "command": "printf 'mine'", "padding": 0});
    let after = repair(tmp.path(), json!({"statusLine": mine.clone()}));
    assert_eq!(after["statusLine"], mine, "{after}");
}
