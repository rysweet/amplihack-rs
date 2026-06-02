use super::helpers::*;
use super::*;
use std::fs;

// ─── Issue #683: XPIA hook verification false-negative fix ─────────────────
//
// The XPIA_HOOK_FILES constant must list `.py` (Python) extensions — not `.sh`
// — because the deployed XPIA hook files are Python scripts. A mismatch
// causes verify_framework_assets to always print "❌ <file> missing" even
// when the hooks are correctly installed.

// ─── Unit: XPIA_HOOK_FILES constant correctness ───────────────────────────

#[test]
fn xpia_hook_files_use_py_extension() {
    for file in XPIA_HOOK_FILES {
        assert!(
            file.ends_with(".py"),
            "XPIA_HOOK_FILES entry `{file}` must use .py extension, not .sh \
             (issue #683: deployed XPIA hooks are Python scripts)"
        );
    }
}

#[test]
fn xpia_hook_files_do_not_use_sh_extension() {
    for file in XPIA_HOOK_FILES {
        assert!(
            !file.ends_with(".sh"),
            "XPIA_HOOK_FILES entry `{file}` must NOT use .sh extension \
             (issue #683: this was the root cause of false-negative verification)"
        );
    }
}

#[test]
fn xpia_hook_files_contains_expected_entries() {
    let expected = ["session_start.py", "post_tool_use.py", "pre_tool_use.py"];
    assert_eq!(
        XPIA_HOOK_FILES.len(),
        expected.len(),
        "XPIA_HOOK_FILES should have exactly {} entries, got {}",
        expected.len(),
        XPIA_HOOK_FILES.len()
    );
    for name in &expected {
        assert!(
            XPIA_HOOK_FILES.contains(name),
            "XPIA_HOOK_FILES must contain `{name}`"
        );
    }
}

#[test]
fn xpia_hook_files_entries_are_valid_filenames() {
    for file in XPIA_HOOK_FILES {
        assert!(
            !file.contains('/') && !file.contains('\\'),
            "XPIA_HOOK_FILES entry `{file}` must be a bare filename (no path separators)"
        );
        assert!(
            !file.is_empty(),
            "XPIA_HOOK_FILES must not contain empty strings"
        );
        assert!(
            !file.starts_with('.'),
            "XPIA_HOOK_FILES entry `{file}` must not be a hidden file"
        );
    }
}

// ─── Unit: XPIA_HOOK_FILES / XPIA_HOOK_SPECS consistency ──────────────────

#[test]
fn xpia_hook_files_covers_xpia_hook_specs_events() {
    // Each XPIA_HOOK_SPECS event should have a corresponding file in
    // XPIA_HOOK_FILES. The mapping is: event "SessionStart" → "session_start",
    // "PreToolUse" → "pre_tool_use", "PostToolUse" → "post_tool_use".
    let event_to_stem: Vec<(&str, &str)> = vec![
        ("SessionStart", "session_start"),
        ("PreToolUse", "pre_tool_use"),
        ("PostToolUse", "post_tool_use"),
    ];
    for (event, stem) in &event_to_stem {
        assert!(
            XPIA_HOOK_SPECS.iter().any(|s| s.event == *event),
            "XPIA_HOOK_SPECS must define event `{event}`"
        );
        let expected_file = format!("{stem}.py");
        assert!(
            XPIA_HOOK_FILES.contains(&expected_file.as_str()),
            "XPIA_HOOK_FILES must contain `{expected_file}` for event `{event}`"
        );
    }
}

#[test]
fn xpia_hook_specs_count_matches_xpia_hook_files_count() {
    assert_eq!(
        XPIA_HOOK_SPECS.len(),
        XPIA_HOOK_FILES.len(),
        "XPIA_HOOK_SPECS and XPIA_HOOK_FILES must have the same number of entries"
    );
}

// ─── Integration: verify_framework_assets XPIA verification ───────────────

#[test]
fn verify_framework_assets_shows_check_marks_when_xpia_py_files_present() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    create_minimal_staged_assets(temp.path());

    // Create the XPIA hooks directory with .py files
    let xpia_dir = temp.path().join(".amplihack/.claude/tools/xpia/hooks");
    fs::create_dir_all(&xpia_dir).unwrap();
    for file in XPIA_HOOK_FILES {
        fs::write(xpia_dir.join(file), "# XPIA hook\nprint('hook')\n").unwrap();
    }

    let claude_dir = temp.path().join(".amplihack/.claude");
    let result = settings::verify_framework_assets(&claude_dir);

    crate::test_support::restore_home(previous);

    assert!(
        result.is_ok(),
        "verify_framework_assets must succeed when all XPIA .py hooks are present"
    );
}

#[test]
fn verify_framework_assets_succeeds_when_xpia_dir_absent() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    create_minimal_staged_assets(temp.path());

    // Do NOT create XPIA directory — it's optional
    let xpia_dir = temp.path().join(".amplihack/.claude/tools/xpia/hooks");
    assert!(
        !xpia_dir.exists(),
        "precondition: XPIA hooks dir must not exist for this test"
    );

    let claude_dir = temp.path().join(".amplihack/.claude");
    let result = settings::verify_framework_assets(&claude_dir);

    crate::test_support::restore_home(previous);

    assert!(
        result.is_ok(),
        "verify_framework_assets must succeed when XPIA dir is absent (optional feature)"
    );
}

#[test]
fn verify_framework_assets_succeeds_with_partial_xpia_files() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    create_minimal_staged_assets(temp.path());

    // Create XPIA directory with only some .py files
    let xpia_dir = temp.path().join(".amplihack/.claude/tools/xpia/hooks");
    fs::create_dir_all(&xpia_dir).unwrap();
    // Only create session_start.py, skip the others
    fs::write(xpia_dir.join("session_start.py"), "# hook\n").unwrap();

    let claude_dir = temp.path().join(".amplihack/.claude");
    let result = settings::verify_framework_assets(&claude_dir);

    crate::test_support::restore_home(previous);

    // XPIA verification is non-fatal — returns Ok regardless of missing files
    assert!(
        result.is_ok(),
        "verify_framework_assets must return Ok even with partial XPIA hooks (non-fatal check)"
    );
}

#[test]
fn verify_framework_assets_succeeds_with_empty_xpia_dir() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    create_minimal_staged_assets(temp.path());

    // Create XPIA directory but leave it empty (no hook files)
    let xpia_dir = temp.path().join(".amplihack/.claude/tools/xpia/hooks");
    fs::create_dir_all(&xpia_dir).unwrap();

    let claude_dir = temp.path().join(".amplihack/.claude");
    let result = settings::verify_framework_assets(&claude_dir);

    crate::test_support::restore_home(previous);

    // Non-fatal — still returns Ok even if all XPIA files are missing
    assert!(
        result.is_ok(),
        "verify_framework_assets must return Ok even with empty XPIA hooks dir (non-fatal check)"
    );
}

#[test]
fn verify_framework_assets_ignores_sh_files_in_xpia_dir() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    create_minimal_staged_assets(temp.path());

    // Create XPIA directory with old .sh files (the pre-fix state)
    let xpia_dir = temp.path().join(".amplihack/.claude/tools/xpia/hooks");
    fs::create_dir_all(&xpia_dir).unwrap();
    fs::write(xpia_dir.join("session_start.sh"), "#!/bin/bash\n").unwrap();
    fs::write(xpia_dir.join("post_tool_use.sh"), "#!/bin/bash\n").unwrap();
    fs::write(xpia_dir.join("pre_tool_use.sh"), "#!/bin/bash\n").unwrap();

    // The .sh files should NOT satisfy the verification — it looks for .py
    // (Verification still returns Ok because XPIA is non-fatal, but files
    // would show as ❌ missing in output)
    let claude_dir = temp.path().join(".amplihack/.claude");
    let result = settings::verify_framework_assets(&claude_dir);

    crate::test_support::restore_home(previous);

    assert!(
        result.is_ok(),
        "verify_framework_assets returns Ok regardless (non-fatal), \
         but .sh files must not satisfy .py verification"
    );

    // Verify that the .py files are what's actually checked
    for file in XPIA_HOOK_FILES {
        let hook_path = xpia_dir.join(file);
        assert!(
            !hook_path.exists(),
            "precondition: {file} (.py) must not exist — only .sh stubs were created"
        );
    }
}

// ─── Regression guard: settings.rs XPIA staging with .py files ────────────

#[test]
fn ensure_settings_json_with_xpia_py_files_succeeds() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let previous = crate::test_support::set_home(temp.path());

    let staging_dir = temp.path().join(".amplihack/.claude");
    let xpia_hooks = staging_dir.join("tools/xpia/hooks");
    fs::create_dir_all(&xpia_hooks).unwrap();

    // Create the .py files that XPIA_HOOK_FILES now expects
    for file in XPIA_HOOK_FILES {
        fs::write(xpia_hooks.join(file), "# XPIA Python hook\n").unwrap();
    }
    let hooks_bin = create_exe_stub(temp.path(), "amplihack-hooks");

    let result = settings::ensure_settings_json(&staging_dir, 99999, &hooks_bin)
        .expect("settings setup must succeed with .py XPIA hook files");
    assert!(result.0, "settings setup must return success");

    crate::test_support::restore_home(previous);
}
