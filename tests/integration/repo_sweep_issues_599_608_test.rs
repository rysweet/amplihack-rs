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

// ── Helpers ──────────────────────────────────────────────────────────────────

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // tests/ → workspace root
    path.pop(); // (already at root)
    path
}

fn read_file(relative: &str) -> String {
    let path = workspace_root().join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {e}", path.display()))
}

/// Collect all `.md` files under a directory, recursively.
fn collect_md_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if !dir.is_dir() {
        return files;
    }
    for entry in walkdir(dir) {
        if entry.extension().and_then(|e| e.to_str()) == Some("md") {
            files.push(entry);
        }
    }
    files
}

/// Simple recursive directory walker (no external dep).
fn walkdir(dir: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // Skip hidden dirs and target/
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                if !name.starts_with('.') && name != "target" && name != "node_modules" {
                    result.extend(walkdir(&path));
                }
            } else {
                result.push(path);
            }
        }
    }
    result
}

/// Collect all SKILL.md files under amplifier-bundle/skills/ (excluding
/// ultrathink-orchestrator which is an intentional legacy reference).
fn collect_skill_files() -> Vec<PathBuf> {
    let skills_dir = workspace_root().join("amplifier-bundle/skills");
    let mut files = Vec::new();
    for path in walkdir(&skills_dir) {
        if path.file_name().and_then(|n| n.to_str()) == Some("SKILL.md") {
            // Exclude ultrathink-orchestrator — intentional legacy reference
            let path_str = path.to_string_lossy();
            if !path_str.contains("ultrathink-orchestrator") {
                files.push(path);
            }
        }
    }
    files
}

// ═════════════════════════════════════════════════════════════════════════════
// TC-SWEEP-602: No stale /ultrathink command references in SKILL.md
// ═════════════════════════════════════════════════════════════════════════════

mod ultrathink_refs {
    use super::*;

    /// TC-SWEEP-602-01: No SKILL.md file (except ultrathink-orchestrator and
    /// dev-orchestrator:478 deprecation alias) should reference `/ultrathink`
    /// as a command.
    #[test]
    fn no_stale_ultrathink_command_refs_in_skills() {
        let mut violations = Vec::new();

        for path in collect_skill_files() {
            let content = fs::read_to_string(&path).unwrap_or_default();
            let rel = path.strip_prefix(&workspace_root()).unwrap_or(&path);

            for (line_num, line) in content.lines().enumerate() {
                // Match `/ultrathink` as a command (not in a branch name or URL path)
                if line.contains("/ultrathink")
                    && !line.contains("feat/ultrathink")
                    && !line.contains("ultrathink-orchestrator")
                    && !line.contains("ultrathink-recipe")
                {
                    // Allow dev-orchestrator's documented deprecation alias
                    let is_deprecation_alias = rel
                        .to_string_lossy()
                        .contains("dev-orchestrator")
                        && line.contains("deprecated");

                    if !is_deprecation_alias {
                        violations.push(format!(
                            "  {}:{}: {}",
                            rel.display(),
                            line_num + 1,
                            line.trim()
                        ));
                    }
                }
            }
        }

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
            // Mirror may not exist in all builds
            return;
        }

        let mut violations = Vec::new();
        for path in walkdir(&docs_skills) {
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let content = fs::read_to_string(&path).unwrap_or_default();
            let rel = path.strip_prefix(&workspace_root()).unwrap_or(&path);

            // Exclude ultrathink-orchestrator — intentional legacy reference
            if rel.to_string_lossy().contains("ultrathink-orchestrator") {
                continue;
            }

            for (line_num, line) in content.lines().enumerate() {
                if line.contains("/ultrathink")
                    && !line.contains("feat/ultrathink")
                    && !line.contains("ultrathink-orchestrator")
                    && !line.contains("ultrathink-recipe")
                {
                    let is_deprecation_alias = rel
                        .to_string_lossy()
                        .contains("dev-orchestrator")
                        && line.contains("deprecated");

                    if !is_deprecation_alias {
                        violations.push(format!(
                            "  {}:{}: {}",
                            rel.display(),
                            line_num + 1,
                            line.trim()
                        ));
                    }
                }
            }
        }

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

    /// Python hook scripts that were replaced by the Rust `amplihack-hooks` binary.
    const STALE_PYTHON_HOOKS: &[&str] = &[
        "workflow_enforcement_hook.py",
        "launcher_detector.py",
        "stop_hook.py",
        "session_start_hook.py",
    ];

    /// TC-SWEEP-603-01: No SKILL.md file should reference nonexistent Python hooks
    /// in prose (code blocks with "Legacy Python example" comments are allowed
    /// since they're retained for conceptual illustration).
    #[test]
    fn no_python_hook_refs_in_skills() {
        let mut violations = Vec::new();

        for path in collect_skill_files() {
            let content = fs::read_to_string(&path).unwrap_or_default();
            let rel = path.strip_prefix(&workspace_root()).unwrap_or(&path);

            let mut in_code_block = false;
            for (line_num, line) in content.lines().enumerate() {
                if line.trim_start().starts_with("```") {
                    in_code_block = !in_code_block;
                    continue;
                }
                // Allow references inside code blocks (legacy examples)
                if in_code_block {
                    continue;
                }
                for hook in STALE_PYTHON_HOOKS {
                    if line.contains(hook) {
                        violations.push(format!(
                            "  {}:{}: references {}: {}",
                            rel.display(),
                            line_num + 1,
                            hook,
                            line.trim()
                        ));
                    }
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

        let mut violations = Vec::new();
        for path in walkdir(&docs_skills) {
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let content = fs::read_to_string(&path).unwrap_or_default();
            let rel = path.strip_prefix(&workspace_root()).unwrap_or(&path);

            for (line_num, line) in content.lines().enumerate() {
                for hook in STALE_PYTHON_HOOKS {
                    if line.contains(hook) {
                        violations.push(format!(
                            "  {}:{}: references {}: {}",
                            rel.display(),
                            line_num + 1,
                            hook,
                            line.trim()
                        ));
                    }
                }
            }
        }

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
// TC-SWEEP-605: GitHubDistributor is feature-gated
// ═════════════════════════════════════════════════════════════════════════════

mod github_distributor_gate {
    use super::*;

    /// TC-SWEEP-605-01: The `GitHubDistributor` struct must be behind
    /// `#[cfg(feature = "github-distributor")]`.
    #[test]
    fn struct_is_feature_gated() {
        let src = read_file("crates/amplihack-utils/src/bundle_generator.rs");

        // Find the struct definition
        let struct_pos = src
            .find("pub struct GitHubDistributor")
            .expect("GitHubDistributor struct should exist in bundle_generator.rs");

        // The cfg attribute should appear before the struct
        let preceding = &src[..struct_pos];
        let last_cfg = preceding.rfind("#[cfg(feature = \"github-distributor\")]");
        assert!(
            last_cfg.is_some(),
            "GitHubDistributor must be gated behind #[cfg(feature = \"github-distributor\")]"
        );

        // Verify no more than a doc comment + blank lines between cfg and struct
        let between = &preceding[last_cfg.unwrap()..];
        let non_attr_lines: Vec<&str> = between
            .lines()
            .skip(1) // skip the cfg line itself
            .filter(|l| !l.trim().is_empty() && !l.trim().starts_with("///"))
            .collect();
        assert!(
            non_attr_lines.is_empty(),
            "cfg attribute should be immediately before GitHubDistributor (found: {non_attr_lines:?})"
        );
    }

    /// TC-SWEEP-605-02: The impl block must also be feature-gated.
    #[test]
    fn impl_is_feature_gated() {
        let src = read_file("crates/amplihack-utils/src/bundle_generator.rs");

        let impl_pos = src
            .find("impl GitHubDistributor")
            .expect("GitHubDistributor impl should exist");

        let preceding = &src[..impl_pos];
        let last_cfg = preceding.rfind("#[cfg(feature = \"github-distributor\")]");
        assert!(
            last_cfg.is_some(),
            "GitHubDistributor impl must be gated behind #[cfg(feature = \"github-distributor\")]"
        );
    }

    /// TC-SWEEP-605-03: The feature must be declared in Cargo.toml but NOT in default.
    #[test]
    fn feature_declared_not_default() {
        let toml = read_file("crates/amplihack-utils/Cargo.toml");

        assert!(
            toml.contains("github-distributor = []"),
            "Cargo.toml must declare github-distributor feature"
        );

        // Check default does not include it
        for line in toml.lines() {
            if line.starts_with("default") && line.contains("github-distributor") {
                panic!("github-distributor must NOT be in default features");
            }
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// TC-SWEEP-606: litellm_callbacks tests use SerialLock
// ═════════════════════════════════════════════════════════════════════════════

mod litellm_serialization {
    use super::*;

    /// TC-SWEEP-606-01: The test module must contain a SerialLock definition.
    #[test]
    fn has_serial_lock() {
        let src = read_file("crates/amplihack-utils/src/litellm_callbacks.rs");
        assert!(
            src.contains("mod serial_lock"),
            "litellm_callbacks test module must define a serial_lock submodule"
        );
        assert!(
            src.contains("OnceLock<Mutex<()>>") || src.contains("OnceLock < Mutex < () > >"),
            "SerialLock must use OnceLock<Mutex<()>> pattern"
        );
    }

    /// TC-SWEEP-606-02: The `unregister_removes_callback` test (the flaky one)
    /// must acquire the serial lock.
    #[test]
    fn flaky_test_uses_serial_lock() {
        let src = read_file("crates/amplihack-utils/src/litellm_callbacks.rs");

        // Find the test function
        let test_start = src
            .find("fn unregister_removes_callback")
            .expect("unregister_removes_callback test must exist");

        // Find the next test or end of module
        let after_test = &src[test_start..];
        let test_end = after_test
            .find("\n    #[test]")
            .unwrap_or(after_test.len());
        let test_body = &after_test[..test_end];

        assert!(
            test_body.contains("SerialLock::acquire()"),
            "unregister_removes_callback must acquire SerialLock to prevent race condition"
        );
    }

    /// TC-SWEEP-606-03: All tests that touch the global registry must use SerialLock.
    #[test]
    fn all_registry_tests_use_serial_lock() {
        let src = read_file("crates/amplihack-utils/src/litellm_callbacks.rs");

        // Tests that register/unregister callbacks touch global state
        let registry_tests = [
            "register_returns_none_when_disabled",
            "register_returns_some_when_enabled",
            "unregister_removes_callback",
            "unregister_noop_when_none",
        ];

        for test_name in &registry_tests {
            let test_pos = src.find(&format!("fn {test_name}"))
                .unwrap_or_else(|| panic!("Test {test_name} must exist"));

            let after = &src[test_pos..];
            let end = after.find("\n    #[test]").unwrap_or(after.len());
            let body = &after[..end];

            assert!(
                body.contains("SerialLock::acquire()"),
                "Test {test_name} touches global registry and must acquire SerialLock"
            );
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// TC-SWEEP-607: docker_detector tests use SerialLock
// ═════════════════════════════════════════════════════════════════════════════

mod docker_serialization {
    use super::*;

    /// TC-SWEEP-607-01: The test module must contain a SerialLock definition.
    #[test]
    fn has_serial_lock() {
        let src = read_file("crates/amplihack-utils/src/docker_detector.rs");
        assert!(
            src.contains("mod serial_lock"),
            "docker_detector test module must define a serial_lock submodule"
        );
    }

    /// TC-SWEEP-607-02: The flaky `is_in_docker_env_var` test must acquire lock.
    #[test]
    fn flaky_test_uses_serial_lock() {
        let src = read_file("crates/amplihack-utils/src/docker_detector.rs");

        let test_start = src
            .find("fn is_in_docker_env_var")
            .expect("is_in_docker_env_var test must exist");

        let after = &src[test_start..];
        let end = after.find("\n    #[test]").unwrap_or(after.len());
        let body = &after[..end];

        assert!(
            body.contains("SerialLock::acquire()"),
            "is_in_docker_env_var must acquire SerialLock to prevent env var race"
        );
    }

    /// TC-SWEEP-607-03: All env-var-mutating tests must acquire the lock.
    #[test]
    fn all_env_mutating_tests_use_serial_lock() {
        let src = read_file("crates/amplihack-utils/src/docker_detector.rs");

        // Tests that set/remove env vars
        let env_tests = [
            "is_in_docker_env_var",
            "is_in_docker_false_when_unset",
            "should_use_docker_false_by_default",
            "check_image_exists_false_when_no_docker",
            "should_use_docker_true_for_all_truthy_env_values",
            "should_use_docker_false_for_falsy_env_values",
        ];

        for test_name in &env_tests {
            let test_pos = src.find(&format!("fn {test_name}"))
                .unwrap_or_else(|| panic!("Test {test_name} must exist"));

            let after = &src[test_pos..];
            let end = after.find("\n    #[test]").unwrap_or(after.len());
            let body = &after[..end];

            assert!(
                body.contains("SerialLock::acquire()"),
                "Test {test_name} mutates env vars and must acquire SerialLock"
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
        let link_pattern = regex::Regex::new(
            r"\]\(([^)]+\.md(?:#[^)]*)?)\)"
        ).unwrap();

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
                        let rel = file.strip_prefix(&workspace_root()).unwrap_or(file);
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

        // Known remaining: ~92 links to non-local targets (index.md → README.md,
        // absolute paths from generated docs, ~/.amplihack/ refs in workflow docs).
        // These are tracked separately; this test guards against regression.
        let max_allowed_broken = 95;
        assert!(
            broken.len() <= max_allowed_broken,
            "Too many broken internal links in docs/ ({} found, max {max_allowed_broken}):\n{}",
            broken.len(),
            broken.iter().take(20).cloned().collect::<Vec<_>>().join("\n")
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
            let rel = file.strip_prefix(&workspace_root()).unwrap_or(file);

            for (line_num, line) in content.lines().enumerate() {
                for dir in &deleted_dirs {
                    if line.contains(&format!("]({dir}"))
                        || line.contains(&format!("](../{dir}"))
                    {
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
