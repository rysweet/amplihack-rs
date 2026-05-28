//! Regression test: no stale Python recipe-runner patterns in SKILL.md files.
//!
//! Scans all SKILL.md files under `amplifier-bundle/skills/` and
//! `docs/claude/skills/` for patterns that indicate stale Python-based
//! recipe-runner instructions that should have been replaced with native
//! Rust CLI equivalents:
//!
//! - `run_recipe_by_name` — legacy Python recipe-runner API
//! - `from amplihack.recipes import` — legacy Python recipe module import
//! - `python3 -c.*recipe` — inline Python invoking recipe-runner
//!
//! Also scans for gratuitous `python3 -c` JSON/YAML one-liners in shell
//! snippets (excluding dynamic-debugger, which legitimately uses debugpy).
//!
//! # Running
//!
//! ```bash
//! cargo test --test no_stale_python_patterns -- --nocapture
//! ```

use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};

// ── Helpers ─────────────────────────────────────────────────────────────────

fn workspace_root() -> PathBuf {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.pop(); // bins/amplihack → bins/
    root.pop(); // bins/ → workspace root
    root
}

/// Recursively find every `SKILL.md` under `dir`.
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

/// Returns true if a path is inside the dynamic-debugger skill directory
/// (debugpy is a legitimate Python tool — do not flag it).
fn is_dynamic_debugger(path: &Path) -> bool {
    path.components()
        .any(|c| c.as_os_str() == "dynamic-debugger")
}

// ── Tests ───────────────────────────────────────────────────────────────────

/// TC-NOPY-01: No stale Python recipe-runner patterns in any SKILL.md.
#[test]
fn tc_nopy_01_no_stale_python_recipe_patterns() {
    let patterns: Vec<(&str, Regex)> = vec![
        (
            "run_recipe_by_name",
            Regex::new(r"run_recipe_by_name").unwrap(),
        ),
        (
            "from amplihack.recipes import",
            Regex::new(r"from\s+amplihack\.recipes\s+import").unwrap(),
        ),
        (
            "python3 -c.*recipe",
            Regex::new(r"python3\s+-c.*recipe").unwrap(),
        ),
    ];

    let scan_dirs = [
        workspace_root().join("amplifier-bundle/skills"),
        workspace_root().join("docs/claude/skills"),
    ];

    let mut violations: Vec<String> = Vec::new();

    for dir in &scan_dirs {
        for skill_file in find_skill_files(dir) {
            let content = fs::read_to_string(&skill_file).expect("read SKILL.md");
            let rel_path = skill_file
                .strip_prefix(&workspace_root())
                .unwrap_or(&skill_file);

            for (label, regex) in &patterns {
                for (line_num, line) in content.lines().enumerate() {
                    if regex.is_match(line) {
                        violations.push(format!(
                            "  {}:{}: matches '{}'\n    {}",
                            rel_path.display(),
                            line_num + 1,
                            label,
                            line.trim()
                        ));
                    }
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "Found stale Python recipe-runner patterns in SKILL.md files:\n{}",
        violations.join("\n")
    );
}

/// TC-NOPY-02: No gratuitous `python3 -c` JSON/YAML one-liners in skills.
///
/// Scans SKILL.md, SECURITY.md, shell scripts, and YAML templates under
/// the skills directories. Excludes dynamic-debugger (debugpy is legitimate).
#[test]
fn tc_nopy_02_no_python3_json_yaml_oneliners() {
    let python3_re = Regex::new(r"python3\s+-c").unwrap();

    let scan_dirs = [
        workspace_root().join("amplifier-bundle/skills"),
        workspace_root().join("docs/claude/skills"),
    ];

    let scannable_extensions = ["md", "sh", "yml", "yaml"];

    let mut violations: Vec<String> = Vec::new();

    for dir in &scan_dirs {
        if !dir.is_dir() {
            continue;
        }
        for entry in walkdir(dir) {
            if is_dynamic_debugger(&entry) {
                continue;
            }

            let ext = entry.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !scannable_extensions.contains(&ext) {
                continue;
            }

            let content = match fs::read_to_string(&entry) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let rel_path = entry.strip_prefix(&workspace_root()).unwrap_or(&entry);

            for (line_num, line) in content.lines().enumerate() {
                if python3_re.is_match(line) {
                    violations.push(format!(
                        "  {}:{}: python3 -c found\n    {}",
                        rel_path.display(),
                        line_num + 1,
                        line.trim()
                    ));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "Found python3 -c JSON/YAML one-liners in skills (use jq/yq instead):\n{}",
        violations.join("\n")
    );
}

/// Recursively walk a directory and return all file paths.
fn walkdir(dir: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    if !dir.is_dir() {
        return result;
    }
    for entry in fs::read_dir(dir).expect("read dir") {
        let entry = entry.expect("read dir entry");
        let path = entry.path();
        if path.is_dir() {
            result.extend(walkdir(&path));
        } else {
            result.push(path);
        }
    }
    result
}
