/// CI regression test — Issue #661: no stale Python recipe-runner patterns.
///
/// Scans all SKILL.md files under `amplifier-bundle/skills/` and
/// `docs/claude/skills/` for patterns that indicate leftover Python
/// recipe-runner usage that should have been replaced with Rust CLI
/// equivalents.
///
/// Checked patterns:
///   - `run_recipe_by_name`      — legacy Python recipe invocation
///   - `from amplihack.recipes import` — legacy Python import
///   - `python3 -c` + `recipe`   — inline Python recipe calls
///
/// Excludes dynamic-debugger (debugpy is a legitimate Python tool).
///
/// # Running
///
/// ```bash
/// cargo test --test no_stale_python_patterns -- --nocapture
/// ```
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

// ── Helpers ──────────────────────────────────────────────────────────────────

fn workspace_root() -> PathBuf {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.pop(); // bins/amplihack → bins/
    root.pop(); // bins/ → workspace root
    root
}

/// Recursively find every `SKILL.md` (case-insensitive) under `dir`.
fn find_skill_files(dir: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    if !dir.is_dir() {
        return result;
    }
    for entry in fs::read_dir(dir).expect("read dir") {
        let entry = entry.expect("read dir entry");
        let path = entry.path();
        if path.is_dir() {
            result.extend(find_skill_files(&path));
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case("SKILL.md"))
        {
            result.push(path);
        }
    }
    result
}

/// Returns true if the path is under a `dynamic-debugger` directory
/// (debugpy is a legitimate Python tool — exempt from this scan).
fn is_dynamic_debugger(path: &Path) -> bool {
    path.components().any(|c| {
        c.as_os_str()
            .to_str()
            .is_some_and(|s| s == "dynamic-debugger")
    })
}

// ── Tests ────────────────────────────────────────────────────────────────────

/// TC-STALE-01: No `run_recipe_by_name` in any SKILL.md.
#[test]
fn no_run_recipe_by_name_in_skill_files() {
    let root = workspace_root();
    let dirs = [
        root.join("amplifier-bundle/skills"),
        root.join("docs/claude/skills"),
    ];

    let mut violations = Vec::new();
    for dir in &dirs {
        for path in find_skill_files(dir) {
            if is_dynamic_debugger(&path) {
                continue;
            }
            let content = fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            for (i, line) in content.lines().enumerate() {
                if line.contains("run_recipe_by_name") {
                    violations.push(format!(
                        "  {}:{}: {}",
                        path.strip_prefix(&root).unwrap_or(&path).display(),
                        i + 1,
                        line.trim()
                    ));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "Found stale `run_recipe_by_name` references in SKILL.md files:\n{}",
        violations.join("\n")
    );
}

/// TC-STALE-02: No `from amplihack.recipes import` in any SKILL.md.
#[test]
fn no_python_recipe_import_in_skill_files() {
    let root = workspace_root();
    let dirs = [
        root.join("amplifier-bundle/skills"),
        root.join("docs/claude/skills"),
    ];

    let mut violations = Vec::new();
    for dir in &dirs {
        for path in find_skill_files(dir) {
            if is_dynamic_debugger(&path) {
                continue;
            }
            let content = fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            for (i, line) in content.lines().enumerate() {
                if line.contains("from amplihack.recipes import")
                    || line.contains("from amplihack.recipes import")
                {
                    violations.push(format!(
                        "  {}:{}: {}",
                        path.strip_prefix(&root).unwrap_or(&path).display(),
                        i + 1,
                        line.trim()
                    ));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "Found stale `from amplihack.recipes import` references in SKILL.md files:\n{}",
        violations.join("\n")
    );
}

/// TC-STALE-03: No `python3 -c` combined with `recipe` in any SKILL.md.
#[test]
fn no_python3_c_recipe_in_skill_files() {
    let root = workspace_root();
    let dirs = [
        root.join("amplifier-bundle/skills"),
        root.join("docs/claude/skills"),
    ];

    let re = Regex::new(r"python3\s+-c.*recipe").expect("compile regex");

    let mut violations = Vec::new();
    for dir in &dirs {
        for path in find_skill_files(dir) {
            if is_dynamic_debugger(&path) {
                continue;
            }
            let content = fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            for (i, line) in content.lines().enumerate() {
                if re.is_match(line) {
                    violations.push(format!(
                        "  {}:{}: {}",
                        path.strip_prefix(&root).unwrap_or(&path).display(),
                        i + 1,
                        line.trim()
                    ));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "Found stale `python3 -c ... recipe` calls in SKILL.md files:\n{}",
        violations.join("\n")
    );
}
