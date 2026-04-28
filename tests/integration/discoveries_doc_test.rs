//! Documentation structure tests for issue #435.
//!
//! These tests verify that `docs/discoveries.md` exists, follows the
//! prescribed format, and is linked from `docs/index.md`. Pure
//! file-content assertions — no binary execution required.
//!
//! Test proportionality: docs-only change → verification tests only.

use std::fs;
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // tests/
    path.pop(); // workspace root
    path
}

fn read_doc(relative: &str) -> String {
    let path = workspace_root().join(relative);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("Failed to read {}: {e}", path.display()))
}

// ═══════════════════════════════════════════════════════════════════════════
// Existence and structure
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn discoveries_file_exists() {
    let path = workspace_root().join("docs/discoveries.md");
    assert!(path.exists(), "docs/discoveries.md must exist (issue #435)");
}

#[test]
fn discoveries_has_title_heading() {
    let doc = read_doc("docs/discoveries.md");
    assert!(
        doc.starts_with("# Discoveries"),
        "docs/discoveries.md must start with '# Discoveries' heading"
    );
}

#[test]
fn discoveries_has_archive_policy() {
    let doc = read_doc("docs/discoveries.md");
    let lower = doc.to_lowercase();
    assert!(
        lower.contains("archive policy") || lower.contains("discoveries-archive"),
        "docs/discoveries.md must document an archive policy for old entries"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Entry format guide
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn discoveries_has_entry_format_section() {
    let doc = read_doc("docs/discoveries.md");
    assert!(
        doc.contains("## Entry Format") || doc.contains("## Format"),
        "docs/discoveries.md must include an entry format guide"
    );
}

#[test]
fn discoveries_format_includes_required_fields() {
    let doc = read_doc("docs/discoveries.md");
    let required_fields = ["Problem", "Root Cause", "Solution", "Key Learnings"];
    for field in &required_fields {
        assert!(
            doc.contains(field),
            "Entry format must document the '{field}' field"
        );
    }
}

#[test]
fn discoveries_format_includes_category_tag() {
    let doc = read_doc("docs/discoveries.md");
    assert!(
        doc.contains("Category"),
        "Entry format must include a Category tag"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Seed entry — at least one real entry must exist
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn discoveries_has_at_least_one_dated_entry() {
    let doc = read_doc("docs/discoveries.md");
    // Entries use the pattern: ## Title (YYYY-MM-DD)
    let has_dated_entry = doc.lines().any(|line| {
        line.starts_with("## ")
                && line.contains('(')
                && line.contains(')')
                // Must contain a date-like pattern: 4 digits, dash, 2 digits
                && line.chars().filter(|c| c.is_ascii_digit()).count() >= 8
    });
    assert!(
        has_dated_entry,
        "docs/discoveries.md must contain at least one dated entry \
         in '## Title (YYYY-MM-DD)' format"
    );
}

#[test]
fn discoveries_has_table_of_contents() {
    let doc = read_doc("docs/discoveries.md");
    assert!(
        doc.contains("## Table of Contents") || doc.contains("## Contents"),
        "docs/discoveries.md must include a Table of Contents section"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Integration: linked from index.md
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn index_links_to_discoveries() {
    let index = read_doc("docs/index.md");
    assert!(
        index.contains("discoveries.md"),
        "docs/index.md must link to discoveries.md"
    );
}

#[test]
fn index_discoveries_link_is_in_contributing_section() {
    let index = read_doc("docs/index.md");
    let contributing_pos = index.find("Contributing");
    let discoveries_pos = index.find("discoveries.md");
    assert!(
        contributing_pos.is_some() && discoveries_pos.is_some(),
        "Both Contributing section and discoveries.md link must exist in index.md"
    );
    assert!(
        contributing_pos.unwrap() < discoveries_pos.unwrap(),
        "discoveries.md link should appear after the Contributing section heading"
    );
}
