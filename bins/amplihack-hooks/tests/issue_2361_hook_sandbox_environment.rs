//! Issue #2361: a hook child started through `HookSandbox` sees only the
//! environment the sandbox builds, never the rest of the test process's.
//!
//! The test sets variables that steer where hooks and their helpers write
//! (and one that no list names) in its own process, runs `env -0` through
//! `HookSandbox::command`, and compares the child's whole environment with the
//! one `tests/support/isolated_hook.rs` documents: the kept variables of the
//! test process, the no-prompt git variables and the four sandbox
//! directories. A sandbox that inherits the environment and removes a fixed
//! list fails here on the first variable missing from that list.
//!
//! This file holds one test so that its `set_var` calls cannot race another
//! test thread of the same binary.
#![cfg(unix)]

#[path = "../../../tests/support/isolated_hook.rs"]
mod isolated_hook;

use isolated_hook::{HookSandbox, NO_GIT_PROMPT_ENV, is_kept};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;

/// Set in the test process; none of them may reach a hook child. The
/// `AMPLIHACK_*` names steer writes in the hooks or in tools they start;
/// `ISSUE_2361_UNLISTED` stands for a variable nobody has thought to list.
const LEAK_CANARIES: &[&str] = &[
    "AMPLIHACK_SIGNAL_STATE_DIR",
    "AMPLIHACK_GRAPH_DB_PATH",
    "AMPLIHACK_KUZU_DB_PATH",
    "AMPLIHACK_ROOT",
    "AMPLIHACK_WORK_ROOT",
    "AMPLIHACK_WORK_ROOT_EXPLICIT",
    "REPO_PATH",
    "CLAUDE_CONFIG_DIR",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "ISSUE_2361_UNLISTED",
];

const LEAK_VALUE: &str = "/nonexistent/issue-2361-leaked";

fn parse_env0(stdout: &[u8]) -> BTreeMap<OsString, OsString> {
    stdout
        .split(|b| *b == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let eq = entry
                .iter()
                .position(|b| *b == b'=')
                .unwrap_or_else(|| panic!("env -0 entry without '=': {entry:?}"));
            (
                OsStr::from_bytes(&entry[..eq]).to_os_string(),
                OsStr::from_bytes(&entry[eq + 1..]).to_os_string(),
            )
        })
        .collect()
}

#[test]
fn hook_children_see_only_the_isolated_environment() {
    for name in LEAK_CANARIES {
        // SAFETY: this binary holds this one test, so no other thread reads
        // or writes the environment while it runs.
        unsafe { std::env::set_var(name, LEAK_VALUE) };
    }

    let sandbox = HookSandbox::new();
    let out = sandbox
        .command("env")
        .arg("-0")
        .output()
        .expect("run env in the hook sandbox");
    assert!(
        out.status.success(),
        "env -0 failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let child = parse_env0(&out.stdout);

    let mut expected: BTreeMap<OsString, OsString> = std::env::vars_os()
        .filter(|(key, _)| key.to_str().is_some_and(is_kept))
        .collect();
    for (name, value) in NO_GIT_PROMPT_ENV {
        expected.insert(name.into(), value.into());
    }
    for (name, dir) in [
        ("HOME", sandbox.home()),
        ("AMPLIHACK_HOME", sandbox.amplihack_home()),
        ("AMPLIHACK_CLAUDE_PROFILE_ROOT", sandbox.profiles()),
        ("CLAUDE_PROJECT_DIR", sandbox.project()),
    ] {
        expected.insert(name.into(), dir.into_os_string());
    }

    let leaked: Vec<&OsString> = child
        .keys()
        .filter(|k| !expected.contains_key(*k))
        .collect();
    assert!(
        leaked.is_empty(),
        "variables outside the sandbox environment reached the hook child: {leaked:?}"
    );
    assert_eq!(
        child, expected,
        "the hook child's environment is exactly the kept variables, the no-prompt git variables and the sandbox directories"
    );
}
