//! Issue #1699: a child started through `isolated(..)` must never see the
//! developer's `AMPLIHACK_CLAUDE_PROFILE_ROOT`, or an `amplihack install` under
//! a temp HOME would refresh the real profiles' settings.json.
//!
//! `env_clear()` is the main control; `isolated(..)` also removes
//! `PROFILE_ROOT_ENV` explicitly after copying the kept variables, so a future
//! change to the kept list cannot let it through. After `env_clear()`, std's
//! `env_remove` deletes the key rather than recording `(key, None)`, so the
//! environment test only catches a kept list that copies the variable; it
//! still passes with the `env_remove` line deleted. The source assertion pins
//! that line. Nothing is spawned.

use crate::isolated_amplihack::{PROFILE_ROOT_ENV, isolated};
use crate::support::quiet_process::{child_set_var, in_quiet_process};
use std::ffi::OsStr;
use std::process::Command;

fn env_of<'a>(cmd: &'a Command, key: &str) -> Option<Option<&'a OsStr>> {
    cmd.get_envs()
        .find(|(k, _)| *k == OsStr::new(key))
        .map(|(_, v)| v)
}

#[test]
fn isolated_never_passes_the_profile_root() {
    in_quiet_process(
        module_path!(),
        "isolated_never_passes_the_profile_root",
        || {
            child_set_var(PROFILE_ROOT_ENV, "/home/someone/.claude-profiles");
            let cmd = isolated("x");
            assert!(
                matches!(env_of(&cmd, PROFILE_ROOT_ENV), None | Some(None)),
                "isolated() must never pass {PROFILE_ROOT_ENV} to the child"
            );
        },
    );
}

#[test]
fn profile_root_env_name_matches_the_binary() {
    assert_eq!(PROFILE_ROOT_ENV, "AMPLIHACK_CLAUDE_PROFILE_ROOT");
    let source = include_str!("../../amplihack-utils/src/rotator_status/root.rs");
    assert!(
        source.contains(&format!("\"{PROFILE_ROOT_ENV}\"")),
        "the binary reads its profile root from {PROFILE_ROOT_ENV}"
    );
}

#[test]
fn isolated_removes_the_profile_root_after_the_kept_vars() {
    let source = include_str!("../../../tests/support/isolated_amplihack.rs");
    let kept_loop = source
        .find("for (key, value) in std::env::vars_os()")
        .expect("isolated(..) copies the kept variables in a loop");
    let removal = source
        .find("command.env_remove(PROFILE_ROOT_ENV);")
        .expect("isolated(..) must remove PROFILE_ROOT_ENV explicitly");
    assert!(
        removal > kept_loop,
        "isolated(..) must remove {PROFILE_ROOT_ENV} after copying the kept variables"
    );
}

/// An integration test that calls `run_install` in process is outside both
/// `isolated(..)` and the library's `cfg!(test)` gate, so the refresh runs
/// against whatever root it inherited. Each such file must remove the
/// variable itself.
#[test]
fn in_process_installs_remove_the_profile_root() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut offenders = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        let path = entry.path();
        // This file names the patterns it searches for.
        if path.extension().is_none_or(|ext| ext != "rs") || path.ends_with(file!()) {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap();
        if source.contains("run_install(None") || source.contains("run_install(Some") {
            let removal = format!("child_remove_var(\"{PROFILE_ROOT_ENV}\")");
            if !source.contains(&removal) {
                offenders.push(path);
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "these tests call run_install in process without removing \
         {PROFILE_ROOT_ENV}: {offenders:?}"
    );
}
