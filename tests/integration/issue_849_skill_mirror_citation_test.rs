//! Issue #849 regression contract: SKILL.md mirror parity + stale-citation guard.
//!
//! Commit #849 reconciled the bundle `default-workflow` SKILL mirror with the
//! authoritative docs copy and repointed the stale F-ERR-2 audit citations after
//! the Step 17a testing-evidence gate was extracted from `default-workflow.yaml`
//! into `workflow-pr-review.yaml`.
//!
//! These invariants regress silently (an edit to one SKILL copy, a rename of the
//! gate step, or a reintroduced `:961` line number would each go unnoticed), so
//! this test pins them:
//!   - the two SKILL.md copies stay byte-for-byte identical,
//!   - the extracted gate node survives in the mirror,
//!   - no doc reintroduces the stale `default-workflow.yaml:961` reference,
//!   - both audit files cite `workflow-pr-review.yaml` by step id with the
//!     point-in-time-snapshot note,
//!   - `workflow-pr-review.yaml` still defines that gate step and its output, so
//!     the audit citations cannot go stale without failing loudly,
//!   - the Step 13 validation reference doc linked from the mirror exists.

use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // bins/amplihack -> bins
    path.pop(); // bins -> workspace root
    path
}

fn read(relative: &str) -> String {
    let path = workspace_root().join(relative);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

const DOCS_SKILL: &str = "docs/claude/skills/default-workflow/SKILL.md";
const BUNDLE_SKILL: &str = "amplifier-bundle/skills/default-workflow/SKILL.md";
const AUDIT_FILES: &[&str] = &[
    "docs/audits/recipe-runner-quality-robustness-audit.md",
    "docs/reference/recipe-runner-audit.md",
];
const PR_REVIEW_RECIPE: &str = "amplifier-bundle/recipes/workflow-pr-review.yaml";
const STEP13_REFERENCE_DOC: &str = "docs/reference/default-workflow-step-13-validation.md";
const GATE_STEP_ID: &str = "step-17a-testing-evidence-gate";

/// The bundle mirror must remain byte-for-byte identical to the authoritative
/// docs copy — this is the core invariant #849 established.
#[test]
fn skill_mirror_is_byte_for_byte_identical() {
    let docs = read(DOCS_SKILL);
    let bundle = read(BUNDLE_SKILL);
    assert_eq!(
        docs, bundle,
        "{BUNDLE_SKILL} must be byte-for-byte identical to {DOCS_SKILL}; the bundle \
         copy is a mirror of the authoritative docs copy (issue #849)."
    );
}

/// The extracted Step 17a testing-evidence gate node must survive in the mirror.
#[test]
fn skill_retains_testing_evidence_gate_node() {
    for f in [DOCS_SKILL, BUNDLE_SKILL] {
        assert!(
            read(f).contains("Step 17a: Testing-Evidence Gate"),
            "{f} must retain the 'Step 17a: Testing-Evidence Gate' node (issue #849)."
        );
    }
}

/// No documentation file may reintroduce the stale line-number citation that
/// pointed F-ERR-2 at `default-workflow.yaml:961` before the gate was extracted.
#[test]
fn no_stale_961_citation_in_docs() {
    let mut offenders = Vec::new();
    let docs_root = workspace_root().join("docs");
    let files = markdown_files(&docs_root);
    assert!(
        !files.is_empty(),
        "no markdown files found under {}; the stale-citation scan would pass \
         vacuously (issue #849).",
        docs_root.display()
    );
    for path in files {
        let body =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        if body.contains("default-workflow.yaml:961") {
            offenders.push(path.display().to_string());
        }
    }
    assert!(
        offenders.is_empty(),
        "stale 'default-workflow.yaml:961' citation reintroduced in: {offenders:?} \
         (the Step 17a gate now lives in {PR_REVIEW_RECIPE}; cite it by step id)."
    );
}

/// Both audit files must cite the extracted gate by recipe file + step id and
/// carry the point-in-time-snapshot note so the reference survives future moves.
#[test]
fn audit_files_cite_extracted_gate_by_step_id() {
    for f in AUDIT_FILES {
        let body = read(f);
        assert!(
            body.contains("workflow-pr-review.yaml"),
            "{f} must cite `workflow-pr-review.yaml` for F-ERR-2 (issue #849)."
        );
        assert!(
            body.contains(GATE_STEP_ID),
            "{f} must locate F-ERR-2 by step id `{GATE_STEP_ID}` (issue #849)."
        );
        assert!(
            body.contains("point-in-time snapshot"),
            "{f} must keep the point-in-time-snapshot note so the citation is \
             located by step id rather than a fixed line number (issue #849)."
        );
    }
}

/// The cited recipe must actually define the gate step and emit its output, so
/// the audit citation cannot go stale without this test failing.
#[test]
fn pr_review_recipe_defines_cited_gate() {
    let recipe = read(PR_REVIEW_RECIPE);
    assert!(
        recipe.contains(GATE_STEP_ID),
        "{PR_REVIEW_RECIPE} must define the `{GATE_STEP_ID}` step referenced by \
         the F-ERR-2 audit citations (issue #849)."
    );
    assert!(
        recipe.contains("step_13_testing_evidence_check"),
        "{PR_REVIEW_RECIPE} must emit `step_13_testing_evidence_check`, the gate \
         output named by the F-ERR-2 audit citations (issue #849)."
    );
}

/// The Step 13 validation reference doc linked from the mirror must exist.
#[test]
fn step13_validation_reference_doc_exists() {
    let path = workspace_root().join(STEP13_REFERENCE_DOC);
    assert!(
        path.exists(),
        "{STEP13_REFERENCE_DOC} must exist; it is linked from the Step 13 Local \
         Validation Contract block in the SKILL mirror (issue #849)."
    );
}

/// Collect every `.md` file under `dir`, recursively.
fn markdown_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(markdown_files(&path));
        } else if path.extension().is_some_and(|ext| ext == "md") {
            out.push(path);
        }
    }
    out
}
