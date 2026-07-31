//! Contract tests for the NPE hunting workflow skill.

use std::fs;
use std::path::PathBuf;

const SKILL: &str = "amplifier-bundle/skills/npe-hunting-workflow/SKILL.md";
const REFERENCE: &str = "amplifier-bundle/skills/npe-hunting-workflow/reference.md";
const EXAMPLES: &str = "amplifier-bundle/skills/npe-hunting-workflow/examples.md";
const BUNDLE: &str = "amplifier-bundle/bundle.md";
const CATALOG: &str = "docs/skills/SKILL_CATALOG.md";
const GUIDE: &str = "amplifier-bundle/agents/core/guide.md";

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn read(relative: &str) -> String {
    let path = workspace_root().join(relative);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn skill_is_registered_with_activation_terms() {
    let skill = read(SKILL);
    assert!(skill.starts_with("---\n"));
    assert!(skill.contains("name: npe-hunting-workflow"));
    assert!(skill.contains("NullPointerException (NPE)"));
    assert!(skill.contains("null-dereference"));
    assert!(read(BUNDLE).contains("npe-hunting-workflow:"));
    assert!(read(CATALOG).contains("`npe-hunting-workflow`"));
    assert!(read(GUIDE).contains("Guide users through the 75 available skills"));
    assert!(read(GUIDE).contains("**Technical Skills** (20)"));
}

#[test]
fn workflow_starts_from_evidence_and_tracks_candidates() {
    let skill = read(SKILL);
    for required in [
        "Start from a real stack trace",
        "Anchor the seed failure",
        "Trace the nullable lifecycle",
        "candidate ledger",
        "false-positive condition",
        "Do not widen the search until the immediate seed path is source-confirmed",
        "never synthesize a trace",
        "Independently re-derive these facts from production source",
    ] {
        assert!(
            skill.contains(required),
            "{SKILL} must contain `{required}`"
        );
    }
}

#[test]
fn skeptical_review_is_a_blocking_gate_for_every_candidate() {
    let skill = read(SKILL);
    assert!(skill.contains("Invoke `crusty-old-engineer`"));
    assert!(skill.contains("Require one verdict per candidate"));
    assert!(skill.contains("No candidate may enter remediation directly from static analysis"));
    for verdict in [
        "VALIDATED",
        "RETAIN_FOR_REPRODUCTION",
        "REJECT_FALSE_POSITIVE",
    ] {
        assert!(skill.contains(verdict));
    }
}

#[test]
fn formal_gate_checks_current_minimal_and_proposed_designs() {
    let skill = read(SKILL);
    for required in [
        "Invoke `tla-plus-expert`",
        "a counterexample for the current design",
        "the tempting minimal fix",
        "a passing model for the proposed fix invariant",
        "atomicity, liveness, and memory-model limits",
        "Skip formal modeling for a direct sequential missing guard",
        "Model replacement as a new resource identity",
    ] {
        assert!(
            skill.contains(required),
            "{SKILL} must contain `{required}`"
        );
    }
}

#[test]
fn characterization_and_validation_precede_fixes() {
    let skill = read(SKILL);
    assert!(skill.contains("Characterize before fixing"));
    assert!(skill.contains("skeptical review retained it"));
    assert!(skill.contains("a characterization test captures current behavior"));
    assert!(skill.contains("Fix only validated bugs through `default-workflow`"));
}

#[test]
fn parallel_fixes_require_disjoint_ownership() {
    let skill = read(SKILL);
    assert!(skill.contains("Create an ownership matrix"));
    assert!(skill.contains("parallel `default-workflow` workstreams only for disjoint groups"));
    assert!(skill.contains("Run overlapping groups sequentially"));
    assert!(read(REFERENCE).contains("Two workstreams are disjoint only if"));
}

#[test]
fn references_capture_false_positive_and_proof_limits() {
    let reference = read(REFERENCE);
    for required in [
        "a lazy getter recreates the object instead of returning null",
        "the value is stale but non-null",
        "the null-producing teardown operation has no production caller",
        "Do not call a bounded model check an unbounded proof",
        "Assertions do not count as production null guards",
    ] {
        assert!(
            reference.contains(required),
            "{REFERENCE} must contain `{required}`"
        );
    }
}

#[test]
fn examples_cover_activation_falsification_and_remediation() {
    let examples = read(EXAMPLES);
    for section in [
        "Example 1: Activate from an observed stack trace",
        "Example 2: Reject plausible static matches",
        "Example 3: Model lifecycle ordering and fix in parallel",
    ] {
        assert!(
            examples.contains(section),
            "{EXAMPLES} must contain `{section}`"
        );
    }
}

#[test]
fn skill_artifacts_are_ascii_and_progressively_disclosed() {
    let skill = read(SKILL);
    assert!(skill.lines().count() < 500);
    assert!(skill.contains("[reference.md](reference.md)"));
    assert!(skill.contains("[examples.md](examples.md)"));

    for artifact in [SKILL, REFERENCE, EXAMPLES] {
        let body = read(artifact);
        assert!(
            body.is_ascii(),
            "{artifact} must remain ASCII-clean for the invisible-character gate"
        );
    }
}
