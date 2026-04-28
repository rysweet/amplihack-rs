//! Documentation and invariant tests for issue #434.
//!
//! Issue #434 was closed as not-planned after investigation showed the
//! gherkin-expert skill already ships in the amplifier-bundle. These tests
//! verify:
//!   1. The investigation doc exists with correct structure
//!   2. The doc is linked from docs/index.md
//!   3. The gherkin-expert skill files remain present (regression guard)
//!   4. The known_skills.rs allowlist still contains "gherkin-expert"
//!
//! Test proportionality: investigation-only change -> verification tests only.

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
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("Failed to read {}: {e}", path.display()))
}

// ═══════════════════════════════════════════════════════════════════════════
// Investigation document existence and structure
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn investigation_doc_exists() {
    let path =
        workspace_root().join("docs/investigations/434-gherkin-v2-experiment-disposition.md");
    assert!(
        path.exists(),
        "docs/investigations/434-gherkin-v2-experiment-disposition.md must exist (issue #434)"
    );
}

#[test]
fn investigation_doc_has_title() {
    let doc = read_file("docs/investigations/434-gherkin-v2-experiment-disposition.md");
    assert!(
        doc.contains(
            "# Investigation: Disposition of Upstream Gherkin v2 Experiment Findings (#434)"
        ),
        "Investigation doc must have the correct title heading"
    );
}

#[test]
fn investigation_doc_has_disposition() {
    let doc = read_file("docs/investigations/434-gherkin-v2-experiment-disposition.md");
    assert!(
        doc.contains("Closed as not-planned"),
        "Investigation doc must state the disposition as 'Closed as not-planned'"
    );
}

#[test]
fn investigation_doc_has_summary_section() {
    let doc = read_file("docs/investigations/434-gherkin-v2-experiment-disposition.md");
    assert!(
        doc.contains("## Summary"),
        "Investigation doc must include a Summary section"
    );
}

#[test]
fn investigation_doc_has_decision_section() {
    let doc = read_file("docs/investigations/434-gherkin-v2-experiment-disposition.md");
    assert!(
        doc.contains("## 4. Decision"),
        "Investigation doc must include a Decision section"
    );
}

#[test]
fn investigation_doc_has_evidence_table() {
    let doc = read_file("docs/investigations/434-gherkin-v2-experiment-disposition.md");
    // The evidence table lists the 5 key files proving gherkin-expert exists
    let evidence_files = [
        "amplifier-bundle/skills/gherkin-expert/SKILL.md",
        "amplifier-bundle/agents/specialized/gherkin-expert.md",
        "known_skills.rs",
    ];
    for file in &evidence_files {
        assert!(
            doc.contains(file),
            "Investigation doc must reference evidence file: {file}"
        );
    }
}

#[test]
fn investigation_doc_has_risks_section() {
    let doc = read_file("docs/investigations/434-gherkin-v2-experiment-disposition.md");
    assert!(
        doc.contains("## 5. Risks Considered"),
        "Investigation doc must include a Risks section"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Integration: linked from docs/index.md
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn index_links_to_investigation_doc() {
    let index = read_file("docs/index.md");
    assert!(
        index.contains("434-gherkin-v2-experiment-disposition.md"),
        "docs/index.md must link to the #434 investigation document"
    );
}

#[test]
fn index_link_is_in_investigations_section() {
    let index = read_file("docs/index.md");
    let investigations_pos = index.find("### Investigations");
    let link_pos = index.find("434-gherkin-v2-experiment-disposition.md");
    assert!(
        investigations_pos.is_some() && link_pos.is_some(),
        "Both Investigations section and #434 link must exist in index.md"
    );
    assert!(
        investigations_pos.unwrap() < link_pos.unwrap(),
        "#434 investigation link should appear after the Investigations section heading"
    );
}

#[test]
fn index_link_has_descriptive_summary() {
    let index = read_file("docs/index.md");
    // The index entry should mention both issue number and key conclusion
    assert!(
        index.contains("#434") || index.contains("434"),
        "Index entry must reference issue #434"
    );
    assert!(
        index.contains("gherkin-expert"),
        "Index entry must mention the gherkin-expert skill"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Regression guard: gherkin-expert skill must remain present
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn gherkin_skill_definition_exists() {
    let path = workspace_root().join("amplifier-bundle/skills/gherkin-expert/SKILL.md");
    assert!(
        path.exists(),
        "amplifier-bundle/skills/gherkin-expert/SKILL.md must exist — \
         removing it would break the gherkin-expert capability (see #434)"
    );
}

#[test]
fn gherkin_agent_definition_exists() {
    let path = workspace_root().join("amplifier-bundle/agents/specialized/gherkin-expert.md");
    assert!(
        path.exists(),
        "amplifier-bundle/agents/specialized/gherkin-expert.md must exist — \
         removing it would break the gherkin-expert capability (see #434)"
    );
}

#[test]
fn known_skills_contains_gherkin_expert() {
    let src = read_file("crates/amplihack-hooks/src/known_skills.rs");
    assert!(
        src.contains("\"gherkin-expert\""),
        "known_skills.rs must contain \"gherkin-expert\" in the allowlist — \
         removing it would break skill discovery (see #434 investigation)"
    );
}
