//! Repo-sweep regression tests — Issues #599–#608.
//!
//! These tests guard against re-introduction of the 10 issues identified
//! in the repo-sweep epic. They are content-validation tests that scan
//! files for stale references, broken links, and missing patterns.
//!
//! # Issue mapping
//!
//! | Test prefix   | Issue | Summary                                    |
//! |---------------|-------|--------------------------------------------|
//! | TC-SWEEP-599  | #599  | Broken internal links in docs/             |
//! | TC-SWEEP-602  | #602  | Stale /ultrathink refs in SKILL.md files   |
//! | TC-SWEEP-603  | #603  | Nonexistent Python hook refs in SKILL.md   |
//! | TC-SWEEP-604  | #604  | Broken ~/.amplihack/ absolute paths in README |
//! | TC-SWEEP-605  | #605  | GitHubDistributor feature-gated            |
//! | TC-SWEEP-606  | #606  | litellm_callbacks test serialization       |
//! | TC-SWEEP-607  | #607  | docker_detector test serialization          |
//!
//! Issues #600, #601, #608 are GitHub-only (close duplicates / already-fixed)
//! and have no file-content regression surface.
//!
//! # Running
//!
//! ```bash
//! cargo test --test repo_sweep_issues_599_608 -- --nocapture
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

// ── Helpers ──────────────────────────────────────────────────────────────────

static WORKSPACE_ROOT: LazyLock<PathBuf> = LazyLock::new(|| {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // tests/ → workspace root
    path.pop();
    path
});

fn workspace_root() -> &'static Path {
    &WORKSPACE_ROOT
}

fn read_file(relative: &str) -> String {
    let path = workspace_root().join(relative);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("Failed to read {}: {e}", path.display()))
}

/// Simple recursive directory walker (no external dep).
/// Uses accumulator pattern to avoid intermediate `Vec` allocations per directory.
fn walkdir(dir: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    walkdir_into(dir, &mut result);
    result
}

fn walkdir_into(dir: &Path, result: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                if !name.starts_with('.') && name != "target" && name != "node_modules" {
                    walkdir_into(&path, result);
                }
            } else {
                result.push(path);
            }
        }
    }
}

/// Collect all `.md` files under a directory, recursively.
fn collect_md_files(dir: &Path) -> Vec<PathBuf> {
    walkdir(dir)
        .into_iter()
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("md"))
        .collect()
}

/// Collect SKILL.md files, excluding ultrathink-orchestrator (intentional legacy).
fn collect_skill_files() -> Vec<PathBuf> {
    walkdir(&workspace_root().join("amplifier-bundle/skills"))
        .into_iter()
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()) == Some("SKILL.md")
                && !p.to_string_lossy().contains("ultrathink-orchestrator")
        })
        .collect()
}

/// Scan markdown files for line-level violations, returning formatted messages.
fn scan_lines(files: &[PathBuf], predicate: impl Fn(&str, &str) -> bool) -> Vec<String> {
    let root = workspace_root();
    let mut violations = Vec::new();
    for path in files {
        let content = fs::read_to_string(path).unwrap_or_default();
        let rel = path.strip_prefix(root).unwrap_or(path);
        let rel_str = rel.to_string_lossy();
        for (i, line) in content.lines().enumerate() {
            if predicate(line, &rel_str) {
                violations.push(format!("  {}:{}: {}", rel.display(), i + 1, line.trim()));
            }
        }
    }
    violations
}

/// Extract the body of a named test function from source code.
fn extract_test_body<'a>(src: &'a str, test_name: &str) -> &'a str {
    let pos = src
        .find(&format!("fn {test_name}"))
        .unwrap_or_else(|| panic!("Test {test_name} must exist"));
    let after = &src[pos..];
    let end = after.find("\n    #[test]").unwrap_or(after.len());
    &after[..end]
}

// ═════════════════════════════════════════════════════════════════════════════
// TC-SWEEP-602: No stale /ultrathink command references in SKILL.md
// ═════════════════════════════════════════════════════════════════════════════

mod ultrathink_refs {
    use super::*;

    /// Returns true if a line has a stale `/ultrathink` command reference.
    fn is_stale_ultrathink(line: &str, rel_path: &str) -> bool {
        if !line.contains("/ultrathink") {
            return false;
        }
        !(line.contains("feat/ultrathink")
            || line.contains("ultrathink-orchestrator")
            || line.contains("ultrathink-recipe")
            || (rel_path.contains("dev-orchestrator") && line.contains("deprecated")))
    }

    /// TC-SWEEP-602-01: No SKILL.md file (except ultrathink-orchestrator and
    /// dev-orchestrator:478 deprecation alias) should reference `/ultrathink`.
    #[test]
    fn no_stale_ultrathink_command_refs_in_skills() {
        let files = collect_skill_files();
        let violations = scan_lines(&files, is_stale_ultrathink);
        assert!(
            violations.is_empty(),
            "Found stale /ultrathink command references (should be /dev):\n{}",
            violations.join("\n")
        );
    }

    /// TC-SWEEP-602-02: Same check for docs/claude/skills/ mirror directory.
    #[test]
    fn no_stale_ultrathink_in_docs_skills_mirror() {
        let docs_skills = workspace_root().join("docs/claude/skills");
        if !docs_skills.is_dir() {
            return;
        }
        let files: Vec<_> = collect_md_files(&docs_skills)
            .into_iter()
            .filter(|p| !p.to_string_lossy().contains("ultrathink-orchestrator"))
            .collect();
        let violations = scan_lines(&files, is_stale_ultrathink);
        assert!(
            violations.is_empty(),
            "Found stale /ultrathink refs in docs/claude/skills/:\n{}",
            violations.join("\n")
        );
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// TC-SWEEP-603: No references to nonexistent Python hook scripts
// ═════════════════════════════════════════════════════════════════════════════

mod python_hook_refs {
    use super::*;

    const STALE_PYTHON_HOOKS: &[&str] = &[
        "workflow_enforcement_hook.py",
        "launcher_detector.py",
        "stop_hook.py",
        "session_start_hook.py",
    ];

    fn has_stale_hook(line: &str) -> bool {
        STALE_PYTHON_HOOKS.iter().any(|h| line.contains(h))
    }

    /// TC-SWEEP-603-01: No SKILL.md should reference nonexistent Python hooks
    /// outside code blocks (legacy examples in code blocks are allowed).
    #[test]
    fn no_python_hook_refs_in_skills() {
        let root = workspace_root();
        let mut violations = Vec::new();

        for path in collect_skill_files() {
            let content = fs::read_to_string(&path).unwrap_or_default();
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let mut in_code_block = false;

            for (i, line) in content.lines().enumerate() {
                if line.trim_start().starts_with("```") {
                    in_code_block = !in_code_block;
                    continue;
                }
                if !in_code_block && has_stale_hook(line) {
                    violations.push(format!("  {}:{}: {}", rel.display(), i + 1, line.trim()));
                }
            }
        }

        assert!(
            violations.is_empty(),
            "Found references to nonexistent Python hooks (should reference amplihack-hooks):\n{}",
            violations.join("\n")
        );
    }

    /// TC-SWEEP-603-02: Same check in docs/claude/skills/ mirror.
    #[test]
    fn no_python_hook_refs_in_docs_mirror() {
        let docs_skills = workspace_root().join("docs/claude/skills");
        if !docs_skills.is_dir() {
            return;
        }
        let files = collect_md_files(&docs_skills);
        let violations = scan_lines(&files, |line, _| has_stale_hook(line));
        assert!(
            violations.is_empty(),
            "Found stale Python hook refs in docs/claude/skills/:\n{}",
            violations.join("\n")
        );
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// TC-SWEEP-604: No broken ~/.amplihack/ absolute paths in README.md
// ═════════════════════════════════════════════════════════════════════════════

mod readme_path_refs {
    use super::*;

    /// TC-SWEEP-604-01: The root README.md should not contain markdown links
    /// targeting `~/.amplihack/.claude/` paths (which are user-local and not
    /// in the repo). They should reference `amplifier-bundle/` instead.
    #[test]
    fn no_broken_tilde_amplihack_links_in_readme() {
        let readme = read_file("README.md");
        let mut violations = Vec::new();

        for (line_num, line) in readme.lines().enumerate() {
            // Match markdown links: [text](~/.amplihack/...)
            if line.contains("](~/.amplihack/") || line.contains("](~/.amplihack\\") {
                violations.push(format!("  README.md:{}: {}", line_num + 1, line.trim()));
            }
        }

        assert!(
            violations.is_empty(),
            "README.md contains broken ~/.amplihack/ links (should use amplifier-bundle/):\n{}",
            violations.join("\n")
        );
    }

    /// TC-SWEEP-604-02: Verify the COPILOT_CLI.md link exists and is correct.
    #[test]
    fn copilot_cli_link_target_exists() {
        let readme = read_file("README.md");
        if readme.contains("COPILOT_CLI.md") {
            // If README references COPILOT_CLI.md, it should be docs/COPILOT_CLI.md
            // (not a bare COPILOT_CLI.md which doesn't exist at root)
            let root = workspace_root();
            let bare = root.join("COPILOT_CLI.md");
            let docs = root.join("docs/COPILOT_CLI.md");

            if readme.contains("](COPILOT_CLI.md)") {
                assert!(
                    bare.exists(),
                    "README.md links to COPILOT_CLI.md but it doesn't exist at repo root. \
                     Should link to docs/COPILOT_CLI.md instead."
                );
            }

            // If it references docs/COPILOT_CLI.md, verify it exists
            if readme.contains("](docs/COPILOT_CLI.md)") {
                assert!(
                    docs.exists(),
                    "README.md links to docs/COPILOT_CLI.md but it doesn't exist."
                );
            }
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// TC-SWEEP-605: GitHubDistributor is a real implementation (no feature gate)
// ═════════════════════════════════════════════════════════════════════════════

mod github_distributor_gate {
    use super::*;

    /// TC-SWEEP-605-01: The `GitHubDistributor` struct must exist and NOT be
    /// behind a feature gate (feature gate was removed when stubs were replaced
    /// with real implementation).
    #[test]
    fn struct_exists_not_feature_gated() {
        let src = read_file("crates/amplihack-utils/src/bundle_generator.rs");

        let struct_pos = src
            .find("pub struct GitHubDistributor")
            .expect("GitHubDistributor struct should exist in bundle_generator.rs");

        // The cfg attribute should NOT appear before the struct
        let preceding = &src[..struct_pos];
        let last_cfg = preceding.rfind("#[cfg(feature = \"github-distributor\")]");
        assert!(
            last_cfg.is_none(),
            "GitHubDistributor must NOT be behind a feature gate (stubs replaced with real impl)"
        );
    }

    /// TC-SWEEP-605-02: The impl block must also NOT be feature-gated.
    #[test]
    fn impl_not_feature_gated() {
        let src = read_file("crates/amplihack-utils/src/bundle_generator.rs");

        let impl_pos = src
            .find("impl GitHubDistributor")
            .expect("GitHubDistributor impl should exist");

        let preceding = &src[..impl_pos];
        // Check there's no cfg(feature) on the line immediately before impl
        let prev_lines: Vec<&str> = preceding.lines().rev().take(3).collect();
        for line in &prev_lines {
            assert!(
                !line.contains("cfg(feature"),
                "GitHubDistributor impl must NOT be behind a feature gate"
            );
        }
    }

    /// TC-SWEEP-605-03: The github-distributor feature must NOT exist in Cargo.toml.
    #[test]
    fn feature_flag_removed() {
        let toml = read_file("crates/amplihack-utils/Cargo.toml");

        assert!(
            !toml.contains("github-distributor"),
            "github-distributor feature flag must be removed from Cargo.toml"
        );
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// TC-SWEEP-606: litellm_callbacks tests use SerialLock
// ═════════════════════════════════════════════════════════════════════════════

mod litellm_serialization {
    use super::*;

    const SRC: &str = "crates/amplihack-utils/src/litellm_callbacks.rs";

    /// TC-SWEEP-606-01: The test module must contain a SerialLock definition.
    #[test]
    fn has_serial_lock() {
        let src = read_file(SRC);
        assert!(
            src.contains("mod serial_lock"),
            "must define serial_lock submodule"
        );
        assert!(
            src.contains("OnceLock<Mutex<()>>") || src.contains("OnceLock < Mutex < () > >"),
            "SerialLock must use OnceLock<Mutex<()>> pattern"
        );
    }

    /// TC-SWEEP-606-02: The flaky test must acquire the serial lock.
    #[test]
    fn flaky_test_uses_serial_lock() {
        let src = read_file(SRC);
        let body = extract_test_body(&src, "unregister_removes_callback");
        assert!(
            body.contains("SerialLock::acquire()"),
            "unregister_removes_callback must acquire SerialLock"
        );
    }

    /// TC-SWEEP-606-03: All registry-mutating tests must use SerialLock.
    #[test]
    fn all_registry_tests_use_serial_lock() {
        let src = read_file(SRC);
        for name in [
            "register_returns_none_when_disabled",
            "register_returns_some_when_enabled",
            "unregister_removes_callback",
            "unregister_noop_when_none",
        ] {
            let body = extract_test_body(&src, name);
            assert!(
                body.contains("SerialLock::acquire()"),
                "Test {name} must acquire SerialLock"
            );
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// TC-SWEEP-607: docker_detector tests use SerialLock
// ═════════════════════════════════════════════════════════════════════════════

mod docker_serialization {
    use super::*;

    const SRC: &str = "crates/amplihack-utils/src/docker_detector.rs";

    /// TC-SWEEP-607-01: The test module must serialize env-mutating tests via
    /// the crate-wide serial lock (`crate::test_serial`). A crate-wide lock is
    /// required because tests in sibling modules share the same process-global
    /// env vars, so a module-private lock is insufficient.
    #[test]
    fn has_serial_lock() {
        let src = read_file(SRC);
        assert!(
            src.contains("crate::test_serial::acquire"),
            "must use the crate-wide test_serial lock"
        );
    }

    /// TC-SWEEP-607-02: The flaky test must acquire the lock.
    #[test]
    fn flaky_test_uses_serial_lock() {
        let src = read_file(SRC);
        let body = extract_test_body(&src, "is_in_docker_env_var");
        assert!(
            body.contains("serial_acquire()"),
            "is_in_docker_env_var must acquire the serial lock"
        );
    }

    /// TC-SWEEP-607-03: All env-var-mutating tests must acquire the lock.
    ///
    /// Note: the former `check_image_exists_false_when_no_docker` test mutated
    /// the process-global `PATH`, which raced with any concurrently spawning
    /// subprocess. It was replaced by a pure `which_docker_in` test that needs
    /// no global mutation, so it is intentionally absent from this list.
    #[test]
    fn all_env_mutating_tests_use_serial_lock() {
        let src = read_file(SRC);
        for name in [
            "is_in_docker_env_var",
            "is_in_docker_false_when_unset",
            "should_use_docker_false_by_default",
            "should_use_docker_true_for_all_truthy_env_values",
            "should_use_docker_false_for_falsy_env_values",
        ] {
            let body = extract_test_body(&src, name);
            assert!(
                body.contains("serial_acquire()"),
                "Test {name} must acquire the serial lock"
            );
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// TC-SWEEP-599: Internal link validation in docs/
// ═════════════════════════════════════════════════════════════════════════════

mod docs_internal_links {
    use super::*;

    /// TC-SWEEP-599-01: Markdown links in docs/ that reference other docs/ files
    /// should point to files that actually exist.
    #[test]
    fn docs_internal_links_resolve() {
        let docs_dir = workspace_root().join("docs");
        if !docs_dir.is_dir() {
            return;
        }

        let md_files = collect_md_files(&docs_dir);
        let mut broken = Vec::new();
        // Regex-like: match [text](relative/path.md) but not http/https URLs
        let link_pattern = regex::Regex::new(r"\]\(([^)]+\.md(?:#[^)]*)?)\)").unwrap();

        for file in &md_files {
            let content = fs::read_to_string(file).unwrap_or_default();
            let file_dir = file.parent().unwrap();

            for (line_num, line) in content.lines().enumerate() {
                for cap in link_pattern.captures_iter(line) {
                    let link = cap.get(1).unwrap().as_str();

                    // Skip URLs
                    if link.starts_with("http://") || link.starts_with("https://") {
                        continue;
                    }

                    // Strip fragment (#section)
                    let path_part = link.split('#').next().unwrap();
                    if path_part.is_empty() {
                        continue;
                    }

                    // Resolve relative to the file's directory
                    let target = file_dir.join(path_part);
                    if !target.exists() {
                        let rel = file.strip_prefix(workspace_root()).unwrap_or(file);
                        broken.push(format!(
                            "  {}:{}: -> {} (not found)",
                            rel.display(),
                            line_num + 1,
                            path_part
                        ));
                    }
                }
            }
        }

        // Known remaining: ~33 links to placeholder targets in documentation-writing
        // examples (e.g., ./other.md, ./auth-config.md) and a handful of generated-doc
        // references. These are tracked separately; this test guards against regression.
        let max_allowed_broken = 40;
        assert!(
            broken.len() <= max_allowed_broken,
            "Too many broken internal links in docs/ ({} found, max {max_allowed_broken}):\n{}",
            broken.len(),
            broken
                .iter()
                .take(20)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    /// TC-SWEEP-599-02: No docs/ markdown file should link to a deleted directory
    /// that was part of the Python-era layout.
    #[test]
    fn no_links_to_deleted_python_dirs() {
        let docs_dir = workspace_root().join("docs");
        if !docs_dir.is_dir() {
            return;
        }

        let deleted_dirs = [
            "amplihack/docker/",
            "amplihack/proxy/",
            "amplihack/config/",
            "amplihack/orchestration/",
        ];

        let md_files = collect_md_files(&docs_dir);
        let mut violations = Vec::new();

        for file in &md_files {
            let content = fs::read_to_string(file).unwrap_or_default();
            let rel = file.strip_prefix(workspace_root()).unwrap_or(file);

            for (line_num, line) in content.lines().enumerate() {
                for dir in &deleted_dirs {
                    if line.contains(&format!("]({dir}")) || line.contains(&format!("](../{dir}")) {
                        violations.push(format!(
                            "  {}:{}: links to deleted dir {}",
                            rel.display(),
                            line_num + 1,
                            dir
                        ));
                    }
                }
            }
        }

        assert!(
            violations.is_empty(),
            "Found links to deleted Python-era directories:\n{}",
            violations.join("\n")
        );
    }
}
