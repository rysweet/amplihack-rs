//! Regression tests for issue #505: the recipe runner and Claude Code
//! `settings.json` templates reference `~/.amplihack/.claude/tools/amplihack/hooks/<script>.py`
//! paths, but the `amplihack install` pipeline does not stage those Python
//! hook scripts at the canonical location, so every nested recipe-runner
//! launch fails to find them.
//!
//! Per Decision 1 in the requirements doc, the fix is to ship the Python
//! hook scripts inside `amplifier-bundle/tools/amplihack/hooks/` so the
//! existing `("tools/amplihack", "tools/amplihack")` entry in
//! `BUNDLE_DIR_MAPPING` (types.rs:65) propagates them recursively into
//! `~/.amplihack/.claude/tools/amplihack/hooks/`.
//!
//! These tests assert both halves of that contract:
//!   1. The bundle source tree at `amplifier-bundle/tools/amplihack/hooks/`
//!      ships every script that downstream consumers reference.
//!   2. After `local_install`, the canonical staged path contains those
//!      same scripts as non-empty regular files (not stubs / not just a
//!      directory).
//!
//! TDD note: these tests are expected to FAIL until the implementation
//! phase adds the Python hook scripts to the bundle source tree.

use super::helpers::*;
use super::*;
use std::fs;
use std::path::{Path, PathBuf};

/// Hook scripts that `settings.json` templates and the recipe runner
/// invoke by absolute path under `~/.amplihack/.claude/tools/amplihack/hooks/`.
/// Sourced from issue #505's acceptance criteria.
const REQUIRED_HOOK_SCRIPTS: &[&str] = &[
    "session_end.py",
    "post_tool_use.py",
    "stop.py",
    "session_stop.py",
    "user_prompt_submit.py",
    "precommit_prefs.py",
];

fn workspace_amplifier_bundle() -> PathBuf {
    // CARGO_MANIFEST_DIR points at crates/amplihack-cli; walk up two levels
    // to reach the workspace root that owns `amplifier-bundle/`.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("workspace root must be two levels above the crate manifest")
        .to_path_buf();
    workspace_root.join("amplifier-bundle")
}

#[test]
fn bundle_source_tree_ships_python_hook_scripts() {
    // Issue #505 root cause: the bundle source tree currently has no
    // `tools/amplihack/hooks/` subdirectory, so even though
    // BUNDLE_DIR_MAPPING already lists `("tools/amplihack",
    // "tools/amplihack")`, there is nothing for `copy_dir_recursive` to
    // propagate into the canonical staged path. Asserting on the source
    // tree (not just the install output) keeps this regression detectable
    // even if the install pipeline grows alternative copy code paths.
    let hooks_dir = workspace_amplifier_bundle().join("tools/amplihack/hooks");
    assert!(
        hooks_dir.is_dir(),
        "expected amplifier-bundle/tools/amplihack/hooks/ to exist in the source tree \
         (issue #505) — got missing directory at {}",
        hooks_dir.display()
    );

    for script in REQUIRED_HOOK_SCRIPTS {
        let path = hooks_dir.join(script);
        assert!(
            path.is_file(),
            "amplifier-bundle/tools/amplihack/hooks/{script} must be shipped \
             so install can stage it at the canonical path consumers expect",
        );
        let metadata = fs::metadata(&path).unwrap();
        assert!(
            metadata.len() > 0,
            "amplifier-bundle/tools/amplihack/hooks/{script} must be non-empty \
             (recipe runner will silently no-op on empty hook scripts)"
        );
    }
}

#[test]
fn local_install_stages_python_hook_scripts_at_canonical_path() {
    // End-to-end contract: after `local_install`, the staged hooks/
    // directory must contain real, non-empty Python scripts at the exact
    // path `settings.json` (in install_flow.rs:168) and the recipe runner
    // reference. We use the bundle-only source-repo fixture and seed it
    // with hook scripts so the test does not depend on the in-repo
    // `amplifier-bundle/` already being populated — the install pipeline
    // is what we're exercising.
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let bin_dir = temp.path().join("stub_bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let hooks_stub = create_exe_stub(&bin_dir, "amplihack-hooks");

    let prev_hooks = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
    let prev_path = std::env::var_os("PATH");
    unsafe {
        std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_stub);
        let new_path = format!(
            "{}:{}",
            bin_dir.display(),
            prev_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default()
        );
        std::env::set_var("PATH", &new_path);
    }

    create_bundle_only_source_repo(temp.path());
    // Seed the source bundle with hook scripts; once the implementation
    // ships them in-repo this seed becomes redundant but harmless.
    let source_hooks = temp.path().join("amplifier-bundle/tools/amplihack/hooks");
    fs::create_dir_all(&source_hooks).unwrap();
    for script in REQUIRED_HOOK_SCRIPTS {
        let body = format!("#!/usr/bin/env python3\n# stub hook: {script}\nprint('ok')\n");
        fs::write(source_hooks.join(script), body).unwrap();
    }

    local_install(temp.path(), None).unwrap();

    if let Some(v) = prev_hooks {
        unsafe { std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v) };
    } else {
        unsafe { std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") };
    }
    if let Some(v) = prev_path {
        unsafe { std::env::set_var("PATH", v) };
    }
    crate::test_support::restore_home(previous);

    let staged_hooks = temp.path().join(".amplihack/.claude/tools/amplihack/hooks");
    assert!(
        staged_hooks.is_dir(),
        "issue #505: install must create {} (the canonical hooks dir)",
        staged_hooks.display()
    );

    for script in REQUIRED_HOOK_SCRIPTS {
        let path = staged_hooks.join(script);
        assert!(
            path.is_file(),
            "issue #505: install must stage {} as a regular file at {}",
            script,
            path.display()
        );
        let body = fs::read_to_string(&path).unwrap();
        assert!(
            !body.trim().is_empty(),
            "issue #505: staged {} must have non-empty content",
            path.display()
        );
    }
}
