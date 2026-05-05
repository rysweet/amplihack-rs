//! TDD tests for issue #561 / PR #554: docs landing and navigation audit cleanup.
//!
//! Verifies the three audit findings are resolved:
//!
//!   1. `docs/index.md` contains no uppercase `DISCOVERIES.md` links (case fix).
//!   2. `docs/index.md` contains no `sdk_eval_loop.py` references (Python file removed).
//!   3. `mkdocs.yml` nav includes Diataxis entries for `concepts/hive-mind-design.md`
//!      and `concepts/hive-mind-eval.md` — docs already linked from `docs/index.md`.
//!   4. `mkdocs.yml` nav includes any new testing docs that exist on disk.
//!
//! These tests define the implementation contract.  Tests 3 and 4 FAIL before the
//! mkdocs.yml nav patch is applied and pass once it is.
//!
//! Test proportionality: docs/nav-only change → file-content assertions, no binary.

use std::fs;
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // tests/
    path.pop(); // workspace root
    path
}

fn read_file(relative: &str) -> String {
    let path = workspace_root().join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {e}", path.display()))
}

// ═══════════════════════════════════════════════════════════════════════════
// 1. docs/index.md: no uppercase DISCOVERIES.md links
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn index_md_has_no_uppercase_discoveries_link() {
    let index = read_file("docs/index.md");
    assert!(
        !index.contains("DISCOVERIES.md"),
        "docs/index.md must not contain uppercase 'DISCOVERIES.md'; \
         use lowercase 'discoveries.md' for case-sensitive GitHub Pages serving. \
         Found at least one occurrence — fix the link target."
    );
}

#[test]
fn index_md_links_discoveries_lowercase() {
    let index = read_file("docs/index.md");
    assert!(
        index.contains("discoveries.md"),
        "docs/index.md must link to 'discoveries.md' (lowercase) \
         in the Contributing section."
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. docs/index.md: no sdk_eval_loop.py references
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn index_md_has_no_sdk_eval_loop_reference() {
    let index = read_file("docs/index.md");
    assert!(
        !index.contains("sdk_eval_loop"),
        "docs/index.md must not reference 'sdk_eval_loop' (stale Python file). \
         Replace with Rust-native wording or remove the reference entirely."
    );
}

#[test]
fn index_md_has_no_python_file_links() {
    let index = read_file("docs/index.md");
    // The landing page must not point users at Python (.py) implementation files.
    let python_file_link = index
        .lines()
        .any(|line| line.contains(".py)") || line.contains(".py`"));
    assert!(
        !python_file_link,
        "docs/index.md must not link directly to Python (.py) files; \
         the landing page should stand alone for amplihack-rs (Rust-native)."
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. mkdocs.yml: Hive Mind Diataxis concepts nav alignment
//
// docs/index.md already links both hive-mind-design.md and hive-mind-eval.md
// under its Concepts section.  mkdocs.yml must surface them in the Concepts
// nav so the GitHub Pages sidebar matches the landing page.
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn mkdocs_nav_includes_hive_mind_design() {
    let mkdocs = read_file("mkdocs.yml");
    assert!(
        mkdocs.contains("concepts/hive-mind-design.md"),
        "mkdocs.yml nav must include 'concepts/hive-mind-design.md'. \
         The file exists at docs/concepts/hive-mind-design.md and is linked \
         from docs/index.md, but it is absent from the nav. \
         Add it under the Concepts section."
    );
}

#[test]
fn mkdocs_nav_includes_hive_mind_eval() {
    let mkdocs = read_file("mkdocs.yml");
    assert!(
        mkdocs.contains("concepts/hive-mind-eval.md"),
        "mkdocs.yml nav must include 'concepts/hive-mind-eval.md'. \
         The file exists at docs/concepts/hive-mind-eval.md and is linked \
         from docs/index.md, but it is absent from the nav. \
         Add it under the Concepts section."
    );
}

#[test]
fn hive_mind_design_doc_exists() {
    let path = workspace_root().join("docs/concepts/hive-mind-design.md");
    assert!(
        path.is_file(),
        "docs/concepts/hive-mind-design.md must exist on disk before adding it to nav."
    );
}

#[test]
fn hive_mind_eval_doc_exists() {
    let path = workspace_root().join("docs/concepts/hive-mind-eval.md");
    assert!(
        path.is_file(),
        "docs/concepts/hive-mind-eval.md must exist on disk before adding it to nav."
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 4. mkdocs.yml: new testing docs nav (conditional — only if files exist)
//
// If docs/testing/MIGRATION_CONTRACT_TESTS.md or
// docs/testing/SUBPROCESS_ENV_ISOLATION_TESTS.md are present on disk they
// must appear in the mkdocs.yml nav.  Tests skip gracefully when the files
// do not yet exist so the suite does not block on unrelated work.
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn mkdocs_nav_includes_migration_contract_tests_if_file_exists() {
    let file_path = workspace_root().join("docs/testing/MIGRATION_CONTRACT_TESTS.md");
    if !file_path.exists() {
        // File not created yet — test is a no-op until it lands.
        return;
    }
    let mkdocs = read_file("mkdocs.yml");
    assert!(
        mkdocs.contains("testing/MIGRATION_CONTRACT_TESTS.md"),
        "docs/testing/MIGRATION_CONTRACT_TESTS.md exists but is not listed in \
         mkdocs.yml nav. Add it under the Testing section so it is discoverable \
         in the GitHub Pages sidebar."
    );
}

#[test]
fn mkdocs_nav_includes_subprocess_env_isolation_tests_if_file_exists() {
    let file_path = workspace_root().join("docs/testing/SUBPROCESS_ENV_ISOLATION_TESTS.md");
    if !file_path.exists() {
        // File not created yet — test is a no-op until it lands.
        return;
    }
    let mkdocs = read_file("mkdocs.yml");
    assert!(
        mkdocs.contains("testing/SUBPROCESS_ENV_ISOLATION_TESTS.md"),
        "docs/testing/SUBPROCESS_ENV_ISOLATION_TESTS.md exists but is not listed \
         in mkdocs.yml nav. Add it under the Testing section so it is discoverable \
         in the GitHub Pages sidebar."
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 5. Regression guard: no accidental deletion of existing Tutorials hive nav
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn mkdocs_nav_retains_hive_mind_getting_started_tutorial() {
    let mkdocs = read_file("mkdocs.yml");
    assert!(
        mkdocs.contains("tutorials/hive-mind-getting-started.md"),
        "mkdocs.yml must retain the existing \
         'tutorials/hive-mind-getting-started.md' nav entry. \
         Do not delete valid existing pages while adding new ones."
    );
}

#[test]
fn mkdocs_nav_retains_hive_mind_tutorial() {
    let mkdocs = read_file("mkdocs.yml");
    assert!(
        mkdocs.contains("tutorials/hive-mind-tutorial.md"),
        "mkdocs.yml must retain the existing \
         'tutorials/hive-mind-tutorial.md' nav entry. \
         Do not delete valid existing pages while adding new ones."
    );
}
