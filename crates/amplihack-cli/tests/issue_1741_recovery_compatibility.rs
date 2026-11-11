//! Ordered invalidation and resource contracts. Historical state is generated
//! by execution, never fabricated to agree with an implementation's schema.
#![cfg(unix)]

use crate::issue_1741_fixture::{Fixture, describe, quote};
use serde_json::json;
use std::fs;

fn four_steps(f: &Fixture) -> serde_json::Value {
    f.document(
        "smart-orchestrator",
        vec![
            f.step("classify"),
            f.step("design"),
            f.step("verify"),
            f.step("publish"),
        ],
    )
}

fn assert_reran_suffix(events: &[String], step: &str, expected: &[&str]) {
    let start = events
        .iter()
        .position(|s| s == step)
        .expect("invalidated step must execute");
    assert_eq!(
        &events[start..],
        expected,
        "dependent results must rerun even when their own inputs appear unchanged"
    );
}

#[test]
fn changed_effective_input_invalidates_the_dependent_ordered_suffix() {
    let f = Fixture::new();
    let mut recipe = four_steps(&f);
    recipe["context"]["input"] = json!("old");
    recipe["steps"][1]["command"] = json!(format!(
        "printf '%s' \"{{{{input}}}}\" > /dev/null; {} design",
        quote(&f.root.join("step-tool"))
    ));
    f.write_recipe(&f.recipe, &recipe);
    f.stop_at("publish");
    f.mode("publish", "success");
    let before = f.events().len();

    let second = f.run(&["-c", "input=new"]);
    assert!(
        second.output.status.success(),
        "{}",
        describe(&second.output)
    );
    assert_reran_suffix(
        &f.events()[before..],
        "design",
        &["design", "verify", "publish"],
    );
    let recovery = second.recovery();
    assert!(
        recovery["invalidated_steps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["reason"] == "input_changed"),
        "{recovery:#}"
    );
}

#[test]
fn definition_change_without_a_version_bump_invalidates_later_results() {
    let f = Fixture::new();
    let mut recipe = four_steps(&f);
    f.write_recipe(&f.recipe, &recipe);
    f.stop_at("publish");
    recipe["steps"][1]["command"] = json!(format!(
        "printf 'changed behavior' > /dev/null; {} design",
        quote(&f.root.join("step-tool"))
    ));
    f.write_recipe(&f.recipe, &recipe);
    f.mode("publish", "success");
    let before = f.events().len();

    let second = f.run(&[]);
    assert!(
        second.output.status.success(),
        "{}",
        describe(&second.output)
    );
    assert_reran_suffix(
        &f.events()[before..],
        "design",
        &["design", "verify", "publish"],
    );
    assert!(
        second.recovery()["invalidated_steps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["reason"] == "definition_changed")
    );
}

#[test]
fn declared_file_change_reruns_its_step_and_suffix_but_preserves_the_prefix() {
    let f = Fixture::new();
    let dependency = f.root.join("design-input.txt");
    fs::write(&dependency, "old").unwrap();
    let mut recipe = four_steps(&f);
    recipe["steps"][1]["recovery"]["resources"] = json!([
        {"kind": "file", "path": dependency}
    ]);
    recipe["steps"][1]["command"] = json!(format!(
        "cat {} > /dev/null; {} design",
        quote(&dependency),
        quote(&f.root.join("step-tool"))
    ));
    f.write_recipe(&f.recipe, &recipe);
    let first = f.stop_at("publish");
    fs::write(&dependency, "new").unwrap();
    f.mode("publish", "success");
    let before = f.events().len();

    let second = f.run(&[]);
    assert!(
        second.output.status.success(),
        "{}",
        describe(&second.output)
    );
    assert_eq!(&f.events()[before..], ["design", "verify", "publish"]);
    second.assert_resumed(&first, "design");
}

#[test]
fn missing_declared_file_prevents_reuse_of_the_affected_step() {
    let f = Fixture::new();
    let dependency = f.root.join("dependency.txt");
    fs::write(&dependency, "required").unwrap();
    let mut recipe = f.workflow();
    recipe["steps"][0]["recovery"]["resources"] = json!([
        {"kind": "file", "path": dependency}
    ]);
    // The tool can remake this dependency; deleting it must still force
    // execution rather than accepting an unverifiable historical completion.
    recipe["steps"][0]["command"] = json!(format!(
        "printf 'required' > {}; {} classify",
        quote(&dependency),
        quote(&f.root.join("step-tool"))
    ));
    f.write_recipe(&f.recipe, &recipe);
    f.stop_at("design");
    fs::remove_file(&dependency).unwrap();
    f.mode("design", "success");

    let second = f.run(&[]);
    assert!(
        second.output.status.success(),
        "{}",
        describe(&second.output)
    );
    assert_eq!(
        f.events(),
        ["classify", "design", "classify", "design", "implement"]
    );
    assert!(
        second.recovery()["invalidated_steps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["reason"] == "resource_unavailable")
    );
}

#[test]
fn external_revision_change_invalidates_repository_dependent_work() {
    let f = Fixture::new();
    let mut recipe = f.workflow();
    recipe["steps"][0]["recovery"]["resources"] = json!([
        {"kind": "repository", "path": f.repo}
    ]);
    f.write_recipe(&f.recipe, &recipe);
    f.stop_at("design");
    fs::write(f.repo.join("input.txt"), "external commit\n").unwrap();
    f.git(&["add", "input.txt"]);
    f.git(&["commit", "-m", "external change"]);
    let head = f.git(&["rev-parse", "HEAD"]);
    f.mode("design", "success");

    let second = f.run(&[]);
    assert!(
        second.output.status.success(),
        "{}",
        describe(&second.output)
    );
    assert_eq!(
        f.events(),
        ["classify", "design", "classify", "design", "implement"]
    );
    assert_eq!(
        f.git(&["rev-parse", "HEAD"]),
        head,
        "recovery must never reset Git"
    );
    assert!(
        second.recovery()["invalidated_steps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["reason"] == "repository_changed")
    );
}

#[test]
fn branch_and_commit_created_by_a_completed_step_remain_compatible() {
    let f = Fixture::new();
    let mut recipe = f.workflow();
    recipe["steps"][0]["command"] = json!(format!(
        "git switch -c workflow-1741 && printf 'workflow progress\\n' > input.txt && git add input.txt && git commit -m progress && {} classify",
        quote(&f.root.join("step-tool"))
    ));
    // Git prints non-JSON progress; preserve it as a string for this step.
    recipe["steps"][0]["parse_json"] = json!(false);
    recipe["steps"][0]["parse_json_required"] = json!(false);
    recipe["steps"][0]["recovery"]["resources"] = json!([
        {"kind": "repository", "path": f.repo}
    ]);
    f.write_recipe(&f.recipe, &recipe);
    let first = f.stop_at("design");
    let head = f.git(&["rev-parse", "HEAD"]);
    f.mode("design", "success");

    let second = f.run(&[]);
    assert!(
        second.output.status.success(),
        "{}",
        describe(&second.output)
    );
    second.assert_resumed(&first, "design");
    assert_eq!(f.events(), ["classify", "design", "design", "implement"]);
    assert_eq!(f.git(&["rev-parse", "HEAD"]), head);
}

#[test]
fn artifact_copy_and_explicit_context_binding_survive_the_original_file() {
    let f = Fixture::new();
    let artifact = f.root.join("design-artifact.txt");
    fs::write(&artifact, "durable design bytes").unwrap();
    let mut recipe = f.workflow();
    recipe["context"]["artifact_path"] = json!(artifact);
    recipe["recovery"]["persist"]["context"]
        .as_array_mut()
        .unwrap()
        .push(json!("artifact_path"));
    recipe["steps"][0]["recovery"]["resources"] = json!([
        {"kind": "artifact", "path": "{{artifact_path}}", "context_key": "artifact_path"}
    ]);
    f.write_recipe(&f.recipe, &recipe);
    let first = f.stop_at("design");
    assert!(
        files_in_recovery(&first)
            .iter()
            .any(|p| fs::read(p).unwrap() == b"durable design bytes"),
        "artifact must be copied before completion is acknowledged"
    );
    fs::remove_file(&artifact).unwrap();
    f.mode("design", "success");
    f.mode("implement", "limit");

    let second = f.run(&[]);
    assert!(
        !second.output.status.success(),
        "{}",
        describe(&second.output)
    );
    second.assert_resumed(&first, "design");
    let result = second.runner_result();
    let rebound = std::path::Path::new(result["context"]["artifact_path"].as_str().unwrap());
    assert!(
        rebound.starts_with(second.dir().join("recovery")),
        "the new attempt owns its inherited artifact"
    );
    assert_eq!(fs::read(rebound).unwrap(), b"durable design bytes");
    assert_eq!(f.events(), ["classify", "design", "design", "implement"]);
    fs::remove_dir_all(first.dir()).unwrap();
    f.mode("implement", "success");

    let third = f.run(&[]);
    assert!(third.output.status.success(), "{}", describe(&third.output));
    third.assert_resumed(&second, "implement");
    let result = third.runner_result();
    let rebound = std::path::Path::new(result["context"]["artifact_path"].as_str().unwrap());
    assert!(rebound.starts_with(third.dir().join("recovery")));
    assert_eq!(fs::read(rebound).unwrap(), b"durable design bytes");
    assert_eq!(
        f.events(),
        ["classify", "design", "design", "implement", "implement"]
    );
}

fn files_in_recovery(attempt: &crate::issue_1741_fixture::Attempt) -> Vec<std::path::PathBuf> {
    crate::issue_1741_fixture::files(&attempt.dir().join("recovery"))
}

#[test]
fn missing_persistence_permission_forces_execution_without_saving_a_secret() {
    let f = Fixture::new();
    let secret = "ISSUE_1741_PRIVATE_VALUE_DO_NOT_PERSIST";
    let mut recipe = f.workflow();
    recipe["context"]["secret"] = json!(secret);
    recipe["steps"][0]["command"] = json!(format!(
        "printf '%s' \"{{{{secret}}}}\" > /dev/null; {} classify",
        quote(&f.root.join("step-tool"))
    ));
    f.write_recipe(&f.recipe, &recipe);
    let first = f.stop_at("design");
    for path in files_in_recovery(&first) {
        assert!(
            !fs::read(path)
                .unwrap()
                .windows(secret.len())
                .any(|b| b == secret.as_bytes()),
            "prohibited inputs must not be persisted"
        );
    }
    f.mode("design", "success");

    let second = f.run(&[]);
    assert!(
        second.output.status.success(),
        "{}",
        describe(&second.output)
    );
    assert_eq!(
        f.events(),
        ["classify", "design", "classify", "design", "implement"]
    );
    assert!(!String::from_utf8_lossy(&second.output.stderr).contains(secret));
    assert!(!second.recovery().to_string().contains(secret));
}

#[test]
fn changed_child_definition_invalidates_its_parent_but_preserves_an_independent_branch() {
    let f = Fixture::new();
    let a = f.root.join("branch-a.yaml");
    let b = f.root.join("branch-b.yaml");
    let mut child_a = f.document("branch-a", vec![f.step("a1"), f.step("a2")]);
    f.write_recipe(&a, &child_a);
    f.write_recipe(
        &b,
        &f.document("branch-b", vec![f.step("b1"), f.step("b2")]),
    );
    let call = |id, recipe| {
        json!({
            "id": id, "type": "recipe", "recipe": recipe, "output": id,
            "parallel_group": "branches", "auto_stage": false,
            "recovery": {"replay": "reuse"}
        })
    };
    f.write_recipe(
        &f.recipe,
        &f.document(
            "smart-orchestrator",
            vec![
                call("branch-a-call", "branch-a"),
                call("branch-b-call", "branch-b"),
                f.step("implement"),
            ],
        ),
    );
    let first = f.stop_at("implement");
    let mut initial = f.events();
    initial.sort();
    assert_eq!(initial, ["a1", "a2", "b1", "b2", "implement"]);
    child_a["steps"][0]["command"] = json!(format!(
        "printf 'new child behavior' > /dev/null; {} a1",
        quote(&f.root.join("step-tool"))
    ));
    f.write_recipe(&a, &child_a);
    f.mode("implement", "success");
    let before = f.events().len();

    let second = f.run(&[]);
    assert!(
        second.output.status.success(),
        "{}",
        describe(&second.output)
    );
    assert_eq!(
        &f.events()[before..],
        ["a1", "a2", "implement"],
        "unchanged branch B must remain reusable"
    );
    let recovery = second.assert_resumed(&first, "a1");
    assert!(
        recovery["restored_steps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["step_id"] == "branch-b-call" || s["step_id"] == "b1"),
        "{recovery:#}"
    );
}
