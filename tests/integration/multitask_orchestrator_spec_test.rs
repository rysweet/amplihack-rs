//! Formal specification tests for the multitask orchestrator refactoring (QA-022).
//!
//! Verifies structural invariants, Python-dependency absence, and behavioral
//! contracts by inspecting source files directly.

use std::fs;
use std::path::Path;

fn multitask_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/amplihack-cli/src/commands/multitask")
}

// ========================================================================
// SPEC 1: BRICK LIMIT — every file ≤ 400 LOC
// ========================================================================

#[test]
fn every_multitask_module_under_brick_limit() {
    let mut violations = Vec::new();
    for entry in fs::read_dir(multitask_dir()).expect("multitask dir") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let lines = fs::read_to_string(&path).unwrap().lines().count();
        if lines > 400 {
            violations.push(format!(
                "{}: {} lines",
                path.file_name().unwrap().to_string_lossy(),
                lines
            ));
        }
    }
    assert!(
        violations.is_empty(),
        "Brick limit violations:\n  {}",
        violations.join("\n  ")
    );
}

// ========================================================================
// SPEC 2: MODULE COUNT — at least 4 .rs files after extraction
// ========================================================================

#[test]
fn multitask_module_has_extracted_submodules() {
    let count = fs::read_dir(multitask_dir())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("rs"))
        .count();
    assert!(
        count >= 4,
        "Expected ≥4 .rs files (mod+models+orchestrator+extracted), found {count}"
    );
}

#[test]
fn multitask_launcher_has_extracted_command_builder_and_log_output_bricks() {
    let dir = multitask_dir();
    for module in ["command_builder.rs", "log_output.rs"] {
        let path = dir.join(module);
        assert!(
            path.exists(),
            "#797 regression: multitask launcher responsibilities must be split into {module}"
        );
        let lines = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
            .lines()
            .count();
        assert!(
            lines <= 400,
            "#797 regression: extracted module {module} must stay within the 400-line brick limit, found {lines}"
        );
    }

    let launcher = fs::read_to_string(dir.join("launcher.rs")).expect("read launcher.rs");
    assert!(
        launcher.contains("command_builder") || launcher.contains("build_launcher_command"),
        "#797 regression: launcher.rs must delegate process argument construction to a focused command-builder brick"
    );
    assert!(
        launcher.contains("log_output") || launcher.contains("tail_log_output"),
        "#797 regression: launcher.rs must delegate log naming/output handling to a focused log-output brick"
    );
}

// ========================================================================
// SPEC 3: LAUNCHER USES RUST CLI — no Python
// ========================================================================

#[test]
fn launcher_template_invokes_amplihack_recipe_run() {
    let mut found = false;
    for entry in fs::read_dir(multitask_dir()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let content = fs::read_to_string(&path).unwrap();
        if content.contains("launcher.sh") && content.contains("amplihack recipe run") {
            found = true;
        }
    }
    assert!(
        found,
        "No launcher.sh template with 'amplihack recipe run' found"
    );
}

#[test]
fn run_sh_template_uses_bash_not_python() {
    for entry in fs::read_dir(multitask_dir()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let content = fs::read_to_string(&path).unwrap();
        if content.contains("exec bash launcher.sh") {
            return;
        }
    }
    panic!("No 'exec bash launcher.sh' found in multitask module");
}

// ========================================================================
// SPEC 4: ZERO PYTHON DEPENDENCIES in generated output
// ========================================================================

#[test]
fn multitask_module_zero_python_in_non_comment_code() {
    let forbidden = [
        ("#!/usr/bin/env python", "Python shebang"),
        ("from amplihack.", "Python import"),
        ("import amplihack", "Python import"),
        ("pip install amplihack", "pip install"),
        ("launcher.py", "Python launcher"),
        ("run_recipe_by_name", "Python recipe API"),
    ];
    for entry in fs::read_dir(multitask_dir()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let content = fs::read_to_string(&path).unwrap();
        let fname = path.file_name().unwrap().to_string_lossy().to_string();
        for (i, line) in content.lines().enumerate() {
            let t = line.trim();
            if t.starts_with("//") || t.starts_with("///") {
                continue;
            }
            for (pat, desc) in &forbidden {
                if t.contains(pat) {
                    panic!("{fname}:{}: forbidden {desc} in code: {t}", i + 1);
                }
            }
        }
    }
}

// ========================================================================
// SPEC 5: NO BARE MUTEX UNWRAP in production code
// ========================================================================

#[test]
fn no_bare_mutex_unwrap() {
    for entry in fs::read_dir(multitask_dir()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let content = fs::read_to_string(&path).unwrap();
        let fname = path.file_name().unwrap().to_string_lossy().to_string();
        let mut in_test = false;
        for (i, line) in content.lines().enumerate() {
            let t = line.trim();
            if t == "#[cfg(test)]" {
                in_test = true;
            }
            if in_test {
                continue;
            }
            if t.starts_with("//") {
                continue;
            }
            if t.contains(".lock().unwrap()") {
                panic!(
                    "{fname}:{}: bare .lock().unwrap() — use .unwrap_or_else(|e| e.into_inner())",
                    i + 1
                );
            }
        }
    }
}

// ========================================================================
// SPEC 6: RUN.SH EXPORTS SESSION TREE VARIABLES
// ========================================================================

#[test]
fn run_sh_exports_session_tree_vars() {
    for entry in fs::read_dir(multitask_dir()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let content = fs::read_to_string(&path).unwrap();
        if content.contains("exec bash launcher.sh") {
            assert!(
                content.contains("AMPLIHACK_TREE_ID"),
                "run.sh must export AMPLIHACK_TREE_ID"
            );
            assert!(
                content.contains("AMPLIHACK_SESSION_DEPTH"),
                "run.sh must export AMPLIHACK_SESSION_DEPTH"
            );
            assert!(
                content.contains("AMPLIHACK_MAX_DEPTH"),
                "run.sh must export AMPLIHACK_MAX_DEPTH"
            );
            return;
        }
    }
    panic!("No run.sh template found");
}

// ========================================================================
// SPEC 7: LAUNCHER.SH HAS set -euo pipefail
// ========================================================================

#[test]
fn launcher_has_strict_error_handling() {
    for entry in fs::read_dir(multitask_dir()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let content = fs::read_to_string(&path).unwrap();
        if content.contains("amplihack recipe run") && content.contains("launcher.sh") {
            assert!(
                content.contains("set -euo pipefail"),
                "launcher.sh template must include 'set -euo pipefail'"
            );
            return;
        }
    }
}
