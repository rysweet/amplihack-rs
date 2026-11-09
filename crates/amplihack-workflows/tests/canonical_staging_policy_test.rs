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
    let failures = policy_failures(&RecipeSet::source(), &cases);
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
    // The source-level removal control for all 40 authorised steps is
    // removing_any_authorised_opt_out_is_named_by_the_policy_check.
}

// ---------------------------------------------------------------------------
// Decision 17.146-1 (S1): the source-level removal control.
//
// `RecipeSet` and `policy_failures` live in `support/canonical_staging.rs`:
//
// * `RecipeSet::source()` reads `amplifier-bundle/recipes/<asset>.yaml`;
//   `RecipeSet::from_dir(dir)` reads `<dir>/<asset>.yaml`. No environment
//   variable selects the directory, so the control is safe in a
//   multi-threaded test process.
// * `RecipeSet::step(asset, step_id) -> Option<serde_yaml::Value>`.
// * `policy_failures(set, cases) -> Vec<String>`: one
//   `"<asset> / <step_id>: omitted or enabled auto_stage"` line per
//   authorised case whose step does not set `auto_stage: false`, in
//   inventory order. `every_authorised_agent_explicitly_defers_staging`
//   calls it with `RecipeSet::source()` and `inventory()`.
//
// This reads "native removal control" as a control that needs only the
// recipe files and a temporary directory. The 25 runner-backed native
// removal runs in canonical_staging_runner_test.rs stay #[ignore]d and
// outstanding; this control does not replace them.
// ---------------------------------------------------------------------------

/// The recipe text with the `auto_stage: false` line of `step_id` removed,
/// and the 1-based number of the removed line. Panics unless the step
/// exists and holds exactly one such line inside its own block.
fn remove_opt_out(text: &str, step_id: &str) -> (String, usize) {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let is_step_start = |l: &str| l.trim_start().starts_with("- id:");
    let names = |l: &str| {
        let value = l.trim_start().trim_start_matches("- id:").trim();
        value.trim_matches('"').trim_matches('\'') == step_id
    };
    let start = lines
        .iter()
        .position(|l| is_step_start(l) && names(l))
        .unwrap_or_else(|| panic!("step {step_id} not found"));
    let indent = lines[start].len() - lines[start].trim_start().len();
    let end = lines[start + 1..]
        .iter()
        .position(|l| {
            let here = l.len() - l.trim_start().len();
            !l.trim().is_empty() && here <= indent
        })
        .map_or(lines.len(), |n| start + 1 + n);
    let hits: Vec<usize> = (start + 1..end)
        .filter(|&n| lines[n].trim() == "auto_stage: false")
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "{step_id}: expected one opt-out line in its block, found {hits:?}"
    );
    let removed = hits[0];
    let mut out = String::with_capacity(text.len());
    for (n, line) in lines.iter().enumerate() {
        if n != removed {
            out.push_str(line);
        }
    }
    (out, removed + 1)
}

fn steps_by_id(document: &serde_yaml::Value) -> Vec<(String, serde_yaml::Value)> {
    document["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .map(|s| (s["id"].as_str().unwrap().to_owned(), s.clone()))
        .collect()
}

/// For each authorised case: copy the recipes into a TempDir, remove that
/// step's opt-out as text, and run `check`. Returns the cases whose removal
/// `check` did not name exactly (an empty list means every removal was
/// detected).
fn undetected_removals(check: &dyn Fn(&RecipeSet, &[Case]) -> Vec<String>) -> Vec<String> {
    let cases = inventory();
    let assets: BTreeSet<String> = cases.iter().map(|c| c.asset.clone()).collect();
    let mut undetected = Vec::new();
    for case in cases.iter().filter(|c| c.authorised_edit) {
        let dir = tempfile::tempdir().unwrap();
        for asset in &assets {
            let from = root().join(format!("amplifier-bundle/recipes/{asset}.yaml"));
            std::fs::copy(&from, dir.path().join(format!("{asset}.yaml"))).unwrap();
        }
        let path = dir.path().join(format!("{}.yaml", case.asset));
        let original = std::fs::read_to_string(&path).unwrap();
        let (edited, line) = remove_opt_out(&original, &case.step_id);
        assert_eq!(
            original.lines().count(),
            edited.lines().count() + 1,
            "{} / {}: the edit removed exactly one line (line {line})",
            case.asset,
            case.step_id
        );
        std::fs::write(&path, &edited).unwrap();
        // The edit changed nothing else: every other step parses the same,
        // and the target differs only by its auto_stage key.
        let before = steps_by_id(&serde_yaml::from_str(&original).unwrap());
        let after = steps_by_id(&serde_yaml::from_str(&edited).unwrap());
        assert_eq!(before.len(), after.len());
        for ((id, a), (id2, b)) in before.iter().zip(&after) {
            assert_eq!(id, id2);
            if *id == case.step_id {
                let mut expected = a.clone();
                expected
                    .as_mapping_mut()
                    .unwrap()
                    .remove(serde_yaml::Value::String("auto_stage".into()))
                    .expect("the step had an auto_stage key");
                assert_eq!(&expected, b, "{id}: only auto_stage changed");
            } else {
                assert_eq!(a, b, "{}: step {id} changed", case.asset);
            }
        }
        let set = RecipeSet::from_dir(dir.path());
        assert!(
            set.step(&case.asset, &case.step_id)
                .is_some_and(|s| s.get("auto_stage").is_none()),
            "{} / {}: the edited recipe still holds the opt-out",
            case.asset,
            case.step_id
        );
        let want = vec![format!(
            "{} / {}: omitted or enabled auto_stage",
            case.asset, case.step_id
        )];
        if check(&set, &cases) != want {
            undetected.push(format!("{} / {}", case.asset, case.step_id));
        }
    }
    undetected
}

#[test]
fn removing_any_authorised_opt_out_is_named_by_the_policy_check() {
    let cases = inventory();
    // 1. Negative control: the unmodified recipes pass.
    assert_eq!(
        policy_failures(&RecipeSet::source(), &cases),
        Vec::<String>::new()
    );
    let copy = tempfile::tempdir().unwrap();
    for asset in cases.iter().map(|c| &c.asset).collect::<BTreeSet<_>>() {
        std::fs::copy(
            root().join(format!("amplifier-bundle/recipes/{asset}.yaml")),
            copy.path().join(format!("{asset}.yaml")),
        )
        .unwrap();
    }
    assert_eq!(
        policy_failures(&RecipeSet::from_dir(copy.path()), &cases),
        Vec::<String>::new(),
        "an unmodified copy passes"
    );
    // 2. Each of the 40 authorised removals is named, and only it.
    assert_eq!(cases.iter().filter(|c| c.authorised_edit).count(), 40);
    let missed = undetected_removals(&|set, cases| policy_failures(set, cases));
    assert!(
        missed.is_empty(),
        "removals the policy check missed: {missed:#?}"
    );
}

#[test]
fn the_adjudicator_is_excluded_by_the_inventory_not_by_its_recipe() {
    let mut cases = inventory();
    let source = RecipeSet::source();
    let adjudicator = source
        .step(
            "workflow-terminal-adjudication",
            "terminal-state-adjudicator",
        )
        .expect("adjudicator step");
    // 3. Mutant: the adjudicator has no opt-out in its recipe, yet the check
    // passes, because the inventory lists it as a scope exclusion.
    assert!(adjudicator.get("auto_stage").is_none());
    assert!(policy_failures(&source, &cases).is_empty());
    let excluded: Vec<_> = cases.iter().filter(|c| !c.authorised_edit).collect();
    assert_eq!(excluded.len(), 1);
    assert_eq!(excluded[0].step_id, "terminal-state-adjudicator");
    // Marking it authorised makes the check name it: the exclusion is the
    // only thing keeping it out.
    for case in cases.iter_mut().filter(|c| !c.authorised_edit) {
        case.authorised_edit = true;
    }
    assert_eq!(
        policy_failures(&source, &cases),
        vec!["workflow-terminal-adjudication / terminal-state-adjudicator: omitted or enabled auto_stage".to_owned()]
    );
}

#[test]
fn the_removal_control_notices_a_check_that_ignores_one_step() {
    // 4. Sensitivity: a weakened check that skips one authorised step leaves
    // exactly that removal undetected.
    let skipped = inventory().into_iter().find(|c| c.authorised_edit).unwrap();
    let skip_asset = skipped.asset.clone();
    let skip_step = skipped.step_id.clone();
    let weakened = move |set: &RecipeSet, cases: &[Case]| {
        let kept: Vec<Case> = cases
            .iter()
            .filter(|c| !(c.asset == skip_asset && c.step_id == skip_step))
            .cloned()
            .collect();
        policy_failures(set, &kept)
    };
    assert_eq!(
        undetected_removals(&weakened),
        vec![format!("{} / {}", skipped.asset, skipped.step_id)]
    );
}

#[test]
fn remove_opt_out_edits_only_the_target_block() {
    let text = "steps:\n  - id: \"a\"\n    auto_stage: false\n  - id: b\n    agent: x\n    auto_stage: false\n";
    let (out, line) = remove_opt_out(text, "b");
    assert_eq!(line, 6);
    assert_eq!(
        out,
        "steps:\n  - id: \"a\"\n    auto_stage: false\n  - id: b\n    agent: x\n"
    );
    let (out, line) = remove_opt_out(text, "a");
    assert_eq!(line, 3);
    assert!(out.contains("  - id: b\n    agent: x\n    auto_stage: false\n"));
}

#[test]
#[should_panic(expected = "expected one opt-out line")]
fn remove_opt_out_refuses_a_step_without_an_opt_out() {
    remove_opt_out(
        "steps:\n  - id: a\n    agent: x\n  - id: b\n    auto_stage: false\n",
        "a",
    );
}
