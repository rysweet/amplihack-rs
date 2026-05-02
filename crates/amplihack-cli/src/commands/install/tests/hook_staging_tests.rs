//! TDD spec for issue #522: the 7 Python hook shims at
//! `amplifier-bundle/tools/amplihack/hooks/` have been ported to native
//! Rust subcommands of the `amplihack-hooks` binary and the `.py` files
//! have been DELETED. These tests assert the inverted contract:
//!
//!   1. The bundle source tree no longer contains any of the 7 ported
//!      `.py` shims (issue #522 deletion criterion).
//!   2. After `local_install`, no `.py` files exist under the staged
//!      `~/.amplihack/.claude/tools/amplihack/hooks/` either.
//!   3. The staged `settings.json` wires every former-Python event to
//!      `amplihack-hooks <subcmd>` (or its alias) — never to a `.py`
//!      script path.
//!
//! Status: FAILING until the .py files are deleted and the install
//! pipeline + dispatcher are updated. This file supersedes the prior
//! issue #505 staging assertions, which became obsolete once the shims
//! were removed in favor of the native Rust dispatcher.

use super::helpers::*;
use super::*;
use std::fs;
use std::path::{Path, PathBuf};

/// The 7 Python hook shims listed in issue #522's exact-scope
/// requirement. Every one of these MUST be absent after the port.
const DELETED_HOOK_SCRIPTS: &[&str] = &[
    "_shim.py",
    "post_tool_use.py",
    "precommit_prefs.py",
    "session_end.py",
    "session_stop.py",
    "stop.py",
    "user_prompt_submit.py",
];

/// Subcommands the staged `settings.json` must invoke via the native
/// `amplihack-hooks` binary (replacing former `.py` references). These
/// correspond to the deleted shims plus any aliases (session-end is a
/// clap alias for stop per design spec A3, so settings.json may wire
/// SessionEnd to either `stop` or `session-end`).
const REQUIRED_NATIVE_SUBCMDS: &[&str] = &["stop", "post-tool-use", "user-prompt-submit"];

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
fn bundle_source_tree_no_longer_ships_ported_python_shims() {
    // Issue #522 deletion criterion: every one of the 7 ported shims must
    // be removed from the bundle source tree. The hooks/ directory itself
    // may still exist (it now holds README.md), but no `.py` file may
    // remain inside it.
    let hooks_dir = workspace_amplifier_bundle().join("tools/amplihack/hooks");
    if !hooks_dir.exists() {
        // Acceptable end state: the entire directory was removed because
        // it contained only the deleted shims.
        return;
    }

    for script in DELETED_HOOK_SCRIPTS {
        let path = hooks_dir.join(script);
        assert!(
            !path.exists(),
            "issue #522: amplifier-bundle/tools/amplihack/hooks/{script} must be DELETED \
             (ported to native amplihack-hooks subcommand) — found stale file at {}",
            path.display()
        );
    }

    // Belt-and-suspenders: enumerate every .py file in the dir so an
    // accidental rename (e.g., precommit_prefs_v2.py) is also caught.
    let stragglers: Vec<String> = fs::read_dir(&hooks_dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| {
                    e.path()
                        .extension()
                        .and_then(|s| s.to_str())
                        .map(|s| s.eq_ignore_ascii_case("py"))
                        .unwrap_or(false)
                })
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        stragglers.is_empty(),
        "issue #522: no .py files may remain under amplifier-bundle/tools/amplihack/hooks/; \
         found: {stragglers:?}"
    );
}

#[test]
fn local_install_does_not_stage_ported_python_shims() {
    // Inverted contract for issue #522: after `local_install`, none of the
    // 7 ported `.py` shims may appear under the staged hooks directory.
    // The hooks dir may still exist (e.g., for README.md), but every .py
    // referenced by the deletion list must be absent.
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
    // Do NOT seed any .py files — the source bundle must be free of the
    // ported shims (post-#522 state).

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
    if staged_hooks.exists() {
        for script in DELETED_HOOK_SCRIPTS {
            let path = staged_hooks.join(script);
            assert!(
                !path.exists(),
                "issue #522: install must not stage {} at {} — the .py shim was ported \
                 to a native amplihack-hooks subcommand",
                script,
                path.display()
            );
        }
    }
}

#[test]
fn local_install_wires_settings_json_to_native_binary_subcommands() {
    // Inverted contract for issue #522: every former-Python event in the
    // staged settings.json must invoke `amplihack-hooks <subcmd>`, never
    // a `.py` script path.
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

    let settings_path = temp.path().join(".claude/settings.json");
    assert!(
        settings_path.is_file(),
        "issue #522: install must produce a settings.json at {}",
        settings_path.display()
    );

    let settings_body = fs::read_to_string(&settings_path).unwrap();

    // Hard-fail on any `.py` reference for the ported scripts.
    for script in DELETED_HOOK_SCRIPTS {
        assert!(
            !settings_body.contains(script),
            "issue #522: settings.json must not reference {script} — that shim was \
             deleted in favor of a native amplihack-hooks subcommand. \
             Found in:\n{settings_body}"
        );
    }

    // Each native subcmd that replaced a `.py` shim must be wired in.
    // The install pipeline emits the binary as a quoted absolute path
    // (e.g., `"/path/to/amplihack-hooks" stop`), so we accept either the
    // bare-name form or the `<path>" <subcmd>` form.
    for subcmd in REQUIRED_NATIVE_SUBCMDS {
        let bare_needle = format!("amplihack-hooks {subcmd}");
        let quoted_needle = format!("amplihack-hooks\\\" {subcmd}");
        let escaped_quoted_needle = format!("amplihack-hooks\" {subcmd}");
        assert!(
            settings_body.contains(&bare_needle)
                || settings_body.contains(&quoted_needle)
                || settings_body.contains(&escaped_quoted_needle),
            "issue #522: settings.json must wire the native `amplihack-hooks {subcmd}` invocation. \
             Got:\n{settings_body}"
        );
    }
}
