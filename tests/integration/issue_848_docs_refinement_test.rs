//! Issue #848 — documentation-refinement contract tests.
//!
//! Follow-up to the step-17a `compliance` → `testing-evidence` gate rename
//! (PR #848). The rename itself is merged and correct; these tests lock in the
//! two out-of-scope documentation-hygiene fixes identified by the architect
//! review so they cannot regress:
//!
//!   1. **SKILL.md mirror drift** — `docs/claude/skills/default-workflow/SKILL.md`
//!      and `amplifier-bundle/skills/default-workflow/SKILL.md` must stay
//!      byte-for-byte identical, and cross-doc references must use repo-relative
//!      inline-code paths (never depth-fragile `../` markdown links).
//!   2. **Stale audit citation** — F-ERR-2 in both audit mirrors must cite the
//!      extracted sub-recipe `amplifier-bundle/recipes/workflow-pr-review.yaml`
//!      (step `step-17a-testing-evidence-gate`) with a point-in-time-snapshot
//!      note, never the dangling `default-workflow.yaml:961` pointer.
//!
//! These are TDD-RED contract tests: at HEAD (pre-refinement) the mirrors
//! differ, the stale `:961` citation is present, the convention note is absent,
//! and the fragile `../` markdown link exists — so every test below fails.
//! After the docs-only refinement lands they all pass, and they guard against
//! re-introduction of any of the four defects.
//!
//! The tests also ground the documentation against the *merged implementation*
//! (`workflow-pr-review.yaml`): if the gate step id, output var, or echo
//! strings are ever renamed again, the citation-accuracy test fails, forcing
//! the docs and code to move in lockstep.
//!
//! # Running
//!
//! ```bash
//! cargo test -p amplihack --test issue_848_docs_refinement -- --nocapture
//! ```

use std::fs;
use std::path::{Path, PathBuf};

// ── Paths ────────────────────────────────────────────────────────────────────

/// The two byte-identical mirror copies of the default-workflow skill doc.
const SKILL_BUNDLE: &str = "amplifier-bundle/skills/default-workflow/SKILL.md";
const SKILL_DOCS: &str = "docs/claude/skills/default-workflow/SKILL.md";

/// The two mirror copies of the recipe-runner audit that carry finding F-ERR-2.
const AUDIT_FILES: &[&str] = &[
    "docs/reference/recipe-runner-audit.md",
    "docs/audits/recipe-runner-quality-robustness-audit.md",
];

/// The extracted sub-recipe that now owns the step-17a testing-evidence gate.
const PR_REVIEW_RECIPE: &str = "amplifier-bundle/recipes/workflow-pr-review.yaml";

// ── Helpers ──────────────────────────────────────────────────────────────────

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // bins/amplihack -> bins
    path.pop(); // bins -> workspace root
    path
}

fn read_abs(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn read_rel(relative: &str) -> String {
    read_abs(&workspace_root().join(relative))
}

/// Recursively collect every `.md` file under `dir` (skips hidden/`target`).
fn collect_md_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if !name.starts_with('.') && name != "target" && name != "node_modules" {
                collect_md_files(&path, out);
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            out.push(path);
        }
    }
}

fn all_docs_md_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_md_files(&workspace_root().join("docs"), &mut out);
    out
}

/// Scan every docs/ markdown file for any of `needles`, returning a
/// `rel/path:line: [needle] content` report line for each offending match.
fn scan_docs_for(needles: &[&str]) -> Vec<String> {
    let root = workspace_root();
    let mut offenders = Vec::new();
    for path in all_docs_md_files() {
        let content = read_abs(&path);
        // Whole-file substring search is a single early-exiting `memmem` pass.
        // The overwhelmingly common no-match file skips the per-line scan
        // entirely, and matching files only line-scan the needles that are
        // actually present — output is identical to a full per-line sweep.
        let hits: Vec<&str> = needles
            .iter()
            .copied()
            .filter(|n| content.contains(n))
            .collect();
        if hits.is_empty() {
            continue;
        }
        let rel = path.strip_prefix(&root).unwrap_or(&path).display();
        for (i, line) in content.lines().enumerate() {
            for needle in &hits {
                if line.contains(needle) {
                    offenders.push(format!("{rel}:{}: [{needle}] {}", i + 1, line.trim()));
                }
            }
        }
    }
    offenders
}

/// Extract the F-ERR-2 finding section (heading up to the next `####`/`###`).
fn ferr2_section(content: &str) -> &str {
    let start = content
        .find("F-ERR-2")
        .expect("audit must contain the F-ERR-2 finding");
    // Back up to the start of the heading line so the whole finding is captured.
    let heading_start = content[..start].rfind('\n').map_or(0, |nl| nl + 1);
    let tail = &content[heading_start..];
    // Section ends at the next finding heading (first line beginning with '#'
    // after the F-ERR-2 heading line itself).
    let after_heading = tail.find('\n').map_or(tail.len(), |nl| nl + 1);
    let body = &tail[after_heading..];
    let end = body
        .find("\n#")
        .map_or(tail.len(), |rel| after_heading + rel);
    &tail[..end]
}

// ── (1) SKILL.md mirror reconciliation ───────────────────────────────────────

#[test]
fn tc_848_1_skill_md_mirrors_are_byte_identical() {
    let bundle = read_rel(SKILL_BUNDLE);
    let docs = read_rel(SKILL_DOCS);

    assert_eq!(
        bundle, docs,
        "The bundled and docs mirror copies of the default-workflow SKILL.md \
         must be byte-for-byte identical; drift between them is the defect \
         issue #848 reconciles.\n  bundle: {SKILL_BUNDLE}\n  docs:   {SKILL_DOCS}"
    );
}

#[test]
fn tc_848_2_both_skill_mirrors_preserve_the_renamed_gate_node() {
    // The mermaid node must carry the *new* name after the rename. This proves
    // reconciliation did not clobber the gate line and that the rename is
    // represented identically in both mirrors.
    const GATE_NODE: &str = "S17a[Step 17a: Testing-Evidence Gate]";
    for path in [SKILL_BUNDLE, SKILL_DOCS] {
        let content = read_rel(path);
        assert!(
            content.contains(GATE_NODE),
            "{path} must contain the renamed gate node `{GATE_NODE}`."
        );
    }
}

#[test]
fn tc_848_3_skill_mirrors_document_the_mirror_path_convention() {
    // Recommendation #1: document the byte-for-byte mirror + the repo-relative
    // inline-code path rule so the depth-mismatch broken-link class cannot recur.
    for path in [SKILL_BUNDLE, SKILL_DOCS] {
        let content = read_rel(path);
        assert!(
            content.contains("mirrored byte-for-byte"),
            "{path} must document that the SKILL.md copies are mirrored \
             byte-for-byte."
        );
        assert!(
            content.contains("repo-relative inline-code path"),
            "{path} must document the repo-relative inline-code path convention \
             for cross-doc references."
        );
    }
}

#[test]
fn tc_848_4_skill_mirrors_use_no_fragile_relative_markdown_links() {
    // The broken-link class that regressed: a `](../…)` markdown link resolves
    // from one mirror depth but not the other. Cross-doc references must be
    // repo-relative inline-code paths instead.
    for path in [SKILL_BUNDLE, SKILL_DOCS] {
        let content = read_rel(path);
        let offenders: Vec<(usize, &str)> = content
            .lines()
            .enumerate()
            .filter(|(_, line)| line.contains("](.."))
            .map(|(i, line)| (i + 1, line.trim()))
            .collect();
        assert!(
            offenders.is_empty(),
            "{path} must not use depth-fragile relative `../` markdown links; \
             use a repo-relative inline-code path instead. Offending lines:\n{}",
            offenders
                .iter()
                .map(|(n, l)| format!("  {n}: {l}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

// ── (2) Stale audit citation correction ──────────────────────────────────────

#[test]
fn tc_848_5_no_stale_default_workflow_yaml_961_citation_in_docs() {
    // The `default-workflow.yaml:961` pointer no longer resolves — the file is
    // now a ~167-line orchestrator. It must not appear anywhere under docs/.
    const STALE: &str = "default-workflow.yaml:961";
    let offenders = scan_docs_for(&[STALE]);
    assert!(
        offenders.is_empty(),
        "The dangling `{STALE}` citation must not appear in any docs/ file; \
         cite `workflow-pr-review.yaml` step `step-17a-testing-evidence-gate` \
         instead. Offenders:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn tc_848_6_audit_mirrors_cite_the_extracted_gate_recipe() {
    // Both audit mirrors must point F-ERR-2 at the correct file + step, with a
    // point-in-time-snapshot note guarding against future line drift.
    for path in AUDIT_FILES {
        let section = {
            let content = read_rel(path);
            ferr2_section(&content).to_owned()
        };

        assert!(
            section.contains("workflow-pr-review.yaml"),
            "{path} F-ERR-2 must cite the extracted sub-recipe \
             `workflow-pr-review.yaml`."
        );
        assert!(
            section.contains("step-17a-testing-evidence-gate"),
            "{path} F-ERR-2 must name the gate step id \
             `step-17a-testing-evidence-gate`."
        );
        assert!(
            section.contains("point-in-time snapshot"),
            "{path} F-ERR-2 must note the line numbers are a point-in-time \
             snapshot that may drift."
        );
        assert!(
            !section.contains("default-workflow.yaml:961"),
            "{path} F-ERR-2 must not retain the stale `default-workflow.yaml:961` \
             pointer."
        );
    }
}

#[test]
fn tc_848_7_audit_mirrors_preserve_ferr2_substance() {
    // The citation fix is out-of-scope-hygiene only: the finding's substance
    // (the Step 13 "CANNOT BE SKIPPED" enforcement gap and the `required: true`
    // remediation) must be preserved, not remediated or removed.
    for path in AUDIT_FILES {
        let section = {
            let content = read_rel(path);
            ferr2_section(&content).to_owned()
        };
        assert!(
            section.contains("CANNOT BE SKIPPED"),
            "{path} F-ERR-2 must preserve the Step 13 enforcement-gap description."
        );
        assert!(
            section.contains("required: true"),
            "{path} F-ERR-2 must preserve the `required: true` remediation."
        );
    }
}

// ── (3) Documentation grounded in the merged implementation ───────────────────

#[test]
fn tc_848_8_cited_recipe_contains_the_documented_gate_contract() {
    // Ground the audit citation against the real merged implementation. If the
    // gate step id, output var, or echo strings are renamed again, this fails
    // and forces the docs to be updated in lockstep.
    let recipe = read_rel(PR_REVIEW_RECIPE);

    for token in [
        "id: \"step-17a-testing-evidence-gate\"",
        "output: \"step_13_testing_evidence_check\"",
        "=== Step 17a: Step 13 Testing-Evidence Gate ===",
        "=== Testing-Evidence Gate PASSED ===",
    ] {
        assert!(
            recipe.contains(token),
            "{PR_REVIEW_RECIPE} must contain `{token}` — the audit citation and \
             SKILL.md gate name depend on this exact contract."
        );
    }
}

#[test]
fn tc_848_9_no_stale_pre_rename_gate_identifiers_remain_in_docs() {
    // The rename must be fully represented: none of the pre-rename step-17a
    // gate identifiers may survive anywhere under docs/. These identifiers are
    // gate-specific and can never collide with the legitimate Philosophy /
    // Pattern Compliance checks at steps 18/19.
    const STALE_IDENTIFIERS: &[&str] = &[
        "step-17a-compliance-verification",
        "step_13_compliance_check",
        "Step 17a: Compliance Verification",
        "Step 17a: Step 13 Compliance",
        "Step 17a (compliance gate)",
    ];
    let offenders = scan_docs_for(STALE_IDENTIFIERS);
    assert!(
        offenders.is_empty(),
        "Pre-rename step-17a gate identifiers must not remain in docs/ after \
         the compliance -> testing-evidence rename. Offenders:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn tc_848_10_legitimate_step_19_compliance_checks_untouched() {
    // Guard against over-eager de-"compliance" edits: the legitimate Philosophy
    // and Patterns Compliance checks at step 19 are NOT the renamed gate and
    // must survive unchanged in both SKILL.md mirrors. The rename is scoped to
    // the step-17a testing-evidence gate only.
    for path in [SKILL_BUNDLE, SKILL_DOCS] {
        let content = read_rel(path);
        assert!(
            content.contains("Step 19a: Philosophy Check"),
            "{path} must retain the legitimate step-19a Philosophy Check; the \
             rename must not touch the step-18/19 compliance checks."
        );
        assert!(
            content.contains("Step 19b: Patterns Check"),
            "{path} must retain the legitimate step-19b Patterns Check."
        );
    }
}
