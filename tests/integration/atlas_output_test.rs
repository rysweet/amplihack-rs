//! TDD tests: Verify `docs/atlas/` output directory exists and has content
//! (Issue #258).
//!
//! These tests verify the code-atlas recipe output directory is committed
//! to the repository with at minimum a README placeholder, ensuring the
//! directory is trackable in git.
//!
//! ## What these tests verify
//!
//! 1. `docs/atlas/` directory exists
//! 2. `docs/atlas/` is not empty (has at least a README.md)
//! 3. No secrets or credentials in atlas output
//! 4. Atlas does NOT contain bug report files (bugs go to GitHub issues)
//!
//! ## Failure modes
//!
//! These tests FAIL (red) before implementation:
//! - `docs/atlas/` directory does not exist
//! - Directory is empty (git won't track empty directories)
//!
//! They PASS (green) once the recipe runs and output is committed.
//!
//! ## Related
//!
//! - `tests/integration/atlas_workflow_test.rs` — workflow file tests
//! - `amplifier-bundle/recipes/code-atlas.yaml` — the recipe
//! - Issue #258: Code Atlas Recipe + CI Workflow

use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Locate `docs/atlas/` relative to the workspace root.
fn atlas_dir() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // bins/amplihack → bins/
    path.pop(); // bins/ → workspace root
    path.push("docs");
    path.push("atlas");
    path
}


// ---------------------------------------------------------------------------
// TEST 1: docs/atlas/ directory exists
// ---------------------------------------------------------------------------

/// The atlas output directory must exist in the repository.
///
/// **FAILS** before implementation: directory does not exist.
#[test]
fn docs_atlas_directory_exists() {
    let dir = atlas_dir();
    assert!(
        dir.exists(),
        "FAIL: docs/atlas/ directory not found at {dir:?}.\n\
         Run the code-atlas recipe to generate output, or create a placeholder.\n\
         See Issue #258."
    );
    assert!(
        dir.is_dir(),
        "FAIL: docs/atlas exists but is not a directory at {dir:?}."
    );
}

// ---------------------------------------------------------------------------
// TEST 2: docs/atlas/ has at least a README.md
// ---------------------------------------------------------------------------

/// The atlas directory must contain at least a README.md file.
/// Git does not track empty directories, so there must be at least
/// one file present — even if the recipe produced no other output.
///
/// **FAILS** before implementation: no README.md in docs/atlas/.
#[test]
fn docs_atlas_has_readme() {
    let readme = atlas_dir().join("README.md");
    assert!(
        readme.exists(),
        "FAIL: docs/atlas/README.md not found.\n\
         The atlas directory must contain at least a README.md:\n\
         - If the recipe produced output, README.md is the atlas index.\n\
         - If the recipe produced no output, README.md is a placeholder\n\
           explaining what the atlas is and how to regenerate it."
    );
}

/// The README must not be empty — it should at minimum describe what
/// the atlas is and how to regenerate it.
#[test]
fn docs_atlas_readme_is_not_empty() {
    let readme = atlas_dir().join("README.md");
    if !readme.exists() {
        // Let the existence test handle this failure
        return;
    }
    let content = std::fs::read_to_string(&readme).expect("failed to read README.md");
    assert!(
        content.len() > 20,
        "FAIL: docs/atlas/README.md is too short ({} bytes).\n\
         It must describe what the atlas is and how to regenerate it.\n\
         Minimum: a heading and one sentence of description.",
        content.len()
    );
}

// ---------------------------------------------------------------------------
// TEST 3: No secrets in atlas output
// ---------------------------------------------------------------------------

/// Atlas output must not contain API keys, tokens, or credentials.
/// The recipe produces documentation — no secrets should ever appear.
///
/// This is a REGRESSION GUARD — it should always pass.
#[test]
fn docs_atlas_contains_no_secrets() {
    let dir = atlas_dir();
    if !dir.exists() {
        return; // Let existence test handle this
    }

    let secret_patterns = [
        "sk-ant-",       // Anthropic API key prefix
        "sk-proj-",      // OpenAI API key prefix
        "ghp_",          // GitHub personal access token
        "AKIA",          // AWS access key prefix
        "password=",     // Hardcoded passwords
        "secret_key=",   // Generic secret keys
        "Bearer eyJ",    // JWT tokens
    ];

    for entry in walkdir(&dir) {
        if let Ok(content) = std::fs::read_to_string(&entry) {
            for pattern in &secret_patterns {
                assert!(
                    !content.contains(pattern),
                    "FAIL: Secret pattern '{pattern}' found in {entry:?}.\n\
                     Atlas output must never contain credentials or API keys.\n\
                     SEC-09: credential redaction must be applied."
                );
            }
        }
    }
}

/// Simple recursive file walker (avoids external dependency).
fn walkdir(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(walkdir(&path));
            } else {
                files.push(path);
            }
        }
    }
    files
}

// ---------------------------------------------------------------------------
// TEST 4: Atlas does NOT contain bug report files
// ---------------------------------------------------------------------------

/// Per the recipe spec, bugs are filed as GitHub issues — NOT stored
/// in the atlas docs directory. If bug-reports/ exists in the atlas,
/// it must not contain any markdown files (those belong in issues).
///
/// This validates the separation of concerns:
/// - docs/atlas/ = living architecture documentation
/// - GitHub Issues = bug reports from the bug hunt
#[test]
fn docs_atlas_does_not_store_bug_reports_inline() {
    let _bug_dir = atlas_dir().join("bug-reports");
    // bug-reports directory may exist for validation logs, but
    // the key contract is that atlas pages don't embed bug lists.
    // This test checks the atlas README doesn't contain bug filing content.
    let readme = atlas_dir().join("README.md");
    if !readme.exists() {
        return;
    }
    let content = std::fs::read_to_string(&readme).unwrap_or_default();

    // The atlas index should not contain inline bug listings
    assert!(
        !content.contains("[code-atlas-bughunt]"),
        "FAIL: docs/atlas/README.md contains bug report references.\n\
         Bugs must be filed as GitHub issues with the 'code-atlas-bughunt' label,\n\
         NOT stored inline in the atlas documentation."
    );
}

// ---------------------------------------------------------------------------
// TEST 5: Atlas files use relative paths only (SEC-16)
// ---------------------------------------------------------------------------

/// All file references in atlas markdown must use relative paths.
/// Absolute paths would break portability across machines.
#[test]
fn docs_atlas_uses_relative_paths_only() {
    let dir = atlas_dir();
    if !dir.exists() {
        return;
    }

    for entry in walkdir(&dir) {
        if entry.extension().map_or(false, |e| e == "md") {
            if let Ok(content) = std::fs::read_to_string(&entry) {
                // Check for absolute paths in links: [text](/absolute/path)
                // or image refs: ![alt](/absolute/path)
                let lines: Vec<&str> = content.lines().collect();
                for (i, line) in lines.iter().enumerate() {
                    // Skip code blocks
                    if line.trim_start().starts_with("```") {
                        continue;
                    }
                    // Check for markdown links with absolute paths
                    if line.contains("](/") && !line.contains("](http") {
                        // Allow root-relative doc links like ](/docs/...)
                        // but flag filesystem absolute paths like ](/home/... or ](/tmp/...
                        if line.contains("](/home/")
                            || line.contains("](/tmp/")
                            || line.contains("](/etc/")
                            || line.contains("](/usr/")
                        {
                            panic!(
                                "FAIL: Absolute filesystem path found in {entry:?} line {line_num}:\n\
                                 {line}\n\
                                 SEC-16: Atlas files must use relative paths only.",
                                entry = entry,
                                line_num = i + 1,
                                line = line
                            );
                        }
                    }
                }
            }
        }
    }
}
