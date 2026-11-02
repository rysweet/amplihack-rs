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

/// The command the launch repair should write: the running binary, as
/// `status_line_command` resolves it.
fn expected_command() -> String {
    let exe = crate::commands::profile::statusline_binary()
        .expect("the test binary must pass the status-line ownership check");
    crate::commands::profile::home_statusline_command(&exe).unwrap()
}

/// Stage a complete framework under `home`, write `settings` (no amplihack
/// hooks) and run the launch repair. Returns the settings file afterwards.
fn repair(home: &Path, settings: Value) -> Value {
    let fixture = tempfile::tempdir().unwrap();
    let hooks_stub = create_exe_stub(&fixture.path().join("stub_bin"), "amplihack-hooks");
    let mut env = IsolatedHome::new(home);
    env.set("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_stub);
    env.set("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL", "1");

    create_minimal_staged_assets(home);
    version_stamp::write_installed_version(crate::VERSION).unwrap();
    let path = home.join(".claude/settings.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, settings.to_string()).unwrap();
    assert_eq!(
        hooks_registered_in_settings(&path).unwrap(),
        HookRegistration::Missing,
        "the fixture must send the launch down the repair path"
    );

    crate::freshness::with_provider("claude", || {
        ensure_framework_installed_with(&mut Vec::new()).expect("the launch repair must succeed")
    });
    assert_eq!(
        hooks_registered_in_settings(&path).unwrap(),
        HookRegistration::Registered,
        "the launch repair did not run"
    );
    serde_json::from_slice(&fs::read(&path).unwrap()).unwrap()
}

#[test]
fn the_launch_repair_writes_the_status_line_when_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let after = repair(tmp.path(), json!({"model": "opus"}));
    assert_eq!(
        after["statusLine"],
        json!({"type": "command", "command": expected_command()}),
        "{after}"
    );
    assert_eq!(after["model"], "opus");
}

#[test]
fn the_launch_repair_keeps_a_users_own_status_line() {
    let tmp = tempfile::tempdir().unwrap();
    let mine = json!({"type": "command", "command": "printf 'mine'", "padding": 0});
    let after = repair(tmp.path(), json!({"statusLine": mine.clone()}));
    assert_eq!(after["statusLine"], mine, "{after}");
}
