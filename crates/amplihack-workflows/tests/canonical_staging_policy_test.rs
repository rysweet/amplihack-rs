// File: crates/amplihack-workflows/tests/canonical_staging_policy_test.rs
//! Refs #1554 and Refs #1914. These policy tests deliberately fail on the baseline.
#![cfg(unix)]
#[path = "support/canonical_staging.rs"]
mod support;
use std::collections::BTreeSet;
use support::*;

// These are additional controls. The original inventory and its 40 required
// controls remain unchanged, including the excluded nested adjudicator.
const SCOPE_REVIEW_CONTROLS: &[(&str, &str)] = &[
    ("workflow-publish", "step-15-commit-push-scope-review"),
    (
        "workflow-pr-review",
        "step-18c-push-feedback-changes-scope-review",
    ),
    ("workflow-finalize", "step-20b-push-cleanup-scope-review"),
];

#[test]
fn inventory_accounts_for_all_direct_and_transitive_agents() {
    fn walk(name: &str, seen: &mut BTreeSet<String>, found: &mut BTreeSet<(String, String)>) {
        if !seen.insert(name.into()) {
            return;
        }
        for s in recipe(name)["steps"].as_sequence().unwrap() {
            if s.get("agent").is_some() || s["type"].as_str() == Some("agent") {
                assert!(
                    found.insert((name.into(), s["id"].as_str().unwrap().into())),
                    "duplicate step"
                );
            }
            if let Some(child) = s["recipe"].as_str() {
                walk(child, seen, found);
            }
        }
    }
    let mut found = BTreeSet::new();
    let mut seen = BTreeSet::new();
    walk("default-workflow", &mut seen, &mut found);
    walk("autodrive-crusty-round", &mut seen, &mut found);
    let rows = inventory();
    let mut expected: BTreeSet<_> = rows
        .iter()
        .map(|c| (c.asset.clone(), c.step_id.clone()))
        .collect();
    assert_eq!(expected.len(), rows.len(), "duplicate inventory row");
    for &(asset, id) in SCOPE_REVIEW_CONTROLS {
        assert!(expected.insert((asset.into(), id.into())));
    }
    assert_eq!(
        found, expected,
        "new or omitted agents require classification and ownership review"
    );
}

#[test]
fn nested_adjudicator_is_a_visible_scope_exclusion() {
    let nested: Vec<_> = inventory()
        .into_iter()
        .filter(|c| !c.authorised_edit)
        .collect();
    assert_eq!(nested.len(), 1);
    assert_eq!(nested[0].asset, "workflow-terminal-adjudication");
    assert_eq!(nested[0].step_id, "terminal-state-adjudicator");
    // This records a limitation; it does NOT assert that the nested agent is safe.
}

#[test]
fn documentation_review_evidence_is_not_a_source_commit_owner() {
    for case in inventory().into_iter().filter(|c| c.mutation) {
        assert!(!case.checkpoint_owner.contains("checkpoint-doc-review"));
        assert!(!case.checkpoint_owner.starts_with("none:"));
    }
}

#[test]
fn every_authorised_agent_explicitly_defers_staging() {
    let cases = inventory();
    let required: Vec<_> = cases.iter().filter(|c| c.authorised_edit).collect();
    assert_eq!(cases.len(), 41);
    assert_eq!(required.len(), 40);
    let failures: Vec<_> = required
        .iter()
        .filter(|c| step(c)["auto_stage"].as_bool() != Some(false))
        .map(|c| format!("{} / {}: omitted or enabled auto_stage", c.asset, c.step_id))
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn checkpoint_delta_adds_four_separately_counted_opt_out_controls() {
    let mut controls = SCOPE_REVIEW_CONTROLS.to_vec();
    controls.push(("hook-rejection-repair", "fix-hook-rejection"));
    assert_eq!(controls.len(), 4);
    for (asset, id) in controls {
        let document = recipe(asset);
        let selected = document["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|step| step["id"].as_str() == Some(id))
            .unwrap();
        assert_eq!(selected["auto_stage"].as_bool(), Some(false));
        let mut removed = selected.clone();
        removed
            .as_mapping_mut()
            .unwrap()
            .remove(serde_yaml::Value::String("auto_stage".into()));
        assert_ne!(removed["auto_stage"].as_bool(), Some(false));
        if asset != "hook-rejection-repair" {
            assert_eq!(
                selected["working_dir"].as_str(),
                Some("{{worktree_setup.worktree_path}}")
            );
            assert_eq!(selected["parse_json"].as_bool(), Some(false));
        }
    }
    // These are source controls, not native runner acceptance. Issue 201
    // still leaves 25 original native removal controls unverified.
}
