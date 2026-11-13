//! #2029: interactive and worker/recipe command paths clear inherited probe
//! suppression. Inspecting env_remove avoids modifying this test's process.
#![cfg(unix)]

use super::command::build_command_for_dir;
use crate::binary_finder::BinaryInfo;
use std::ffi::OsStr;

#[test]
fn interactive_and_subprocess_launches_explicitly_remove_the_probe_marker() {
    let _env = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir_in("/tmp").unwrap();
    let _home = crate::test_support::HomeGuard::set(tmp.path());
    let _cwd = crate::test_support::CwdGuard::set(tmp.path()).unwrap();
    let isolated = tmp.path().to_str().unwrap();
    let _vars = crate::test_support::EnvGuard::set([
        ("AMPLIHACK_HOME", isolated),
        ("AMPLIHACK_ROOT", isolated),
        ("CLAUDE_CONFIG_DIR", isolated),
        ("CODEX_HOME", isolated),
        ("XDG_CONFIG_HOME", isolated),
        ("AMPLIHACK_CLAUDE_PROFILE_ROOT", isolated),
        ("AMPLIHACK_USAGE_PROBE", "1"),
    ]);
    let binary = BinaryInfo {
        name: "claude".into(),
        path: tmp.path().join("unused-claude"),
        version: None,
    };
    let contract = tmp.path().join("contract.md");
    for subprocess_safe in [false, true] {
        let cmd = build_command_for_dir(
            &binary,
            false,
            false,
            false,
            &[],
            None,
            subprocess_safe,
            None,
            Some(&contract),
            None,
        )
        .unwrap();
        assert_eq!(
            cmd.get_envs()
                .find(|(key, _)| *key == OsStr::new("AMPLIHACK_USAGE_PROBE"))
                .map(|(_, value)| value),
            Some(None),
            "launch path with subprocess_safe={subprocess_safe} leaked probe suppression"
        );
    }
}
