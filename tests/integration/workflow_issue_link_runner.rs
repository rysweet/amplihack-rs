//! Execute the production nested context mappings with the pinned recipe runner.
use super::super::workflow_issue_link_fixture::{Fixture, describe, recipe, root, step};
use std::fs;
use std::path::PathBuf;

fn pinned_runner() -> PathBuf {
    let pin = fs::read_to_string(root().join("claude-plugin/recipe-runner.rev")).unwrap();
    let runner = std::env::var_os("RECIPE_RUNNER_RS_PATH")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .map(|p| p.join("recipe-runner-rs"))
                .find(|p| p.is_file())
        })
        .expect(
            "install scripts/install-pinned-recipe-runner.sh; runner is a required prerequisite",
        );
    let receipt = runner
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(".crates2.json");
    let receipt: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(receipt).unwrap()).unwrap();
    assert!(
        receipt["installs"]
            .as_object()
            .unwrap()
            .keys()
            .any(|key| key.starts_with("recipe-runner-rs ")
                && key.contains("git+https://github.com/rysweet/amplihack-recipe-runner?")
                && key.contains(&format!("#{}", pin.trim()))),
        "runner must come from the repository pin {}",
        pin.trim()
    );
    runner
}

fn controlled(id: &str, output: &str, value: serde_json::Value) -> serde_yaml::Value {
    // printf's argument is JSON in a positional argument, never executable data.
    let json = value.to_string().replace('\'', "'\\''");
    serde_yaml::to_value(serde_json::json!({
        "id": id, "type": "bash", "output": output, "parse_json": true,
        "command": format!("printf '%s\\n' '{json}'")
    }))
    .unwrap()
}

fn run(mode: Option<&str>, stale: &str, expected: &str) {
    let f = Fixture::new();
    f.change("src/feature.rs");
    let recipes = f.path().join("recipes");
    fs::create_dir(&recipes).unwrap();
    for (name, next) in [
        ("auto-drive-to-merge", "autodrive-build"),
        ("autodrive-build", "step-02-default-workflow"),
        ("default-workflow", "workflow-publish"),
        ("workflow-publish", "step-16-create-draft-pr"),
    ] {
        let mut doc = recipe(name);
        // Keep real defaults, recovery context and complete production boundary
        // steps. Replace only unrelated prior work with controlled observations.
        let boundary = step(name, next);
        let mut steps = Vec::new();
        steps.push(serde_yaml::to_value(serde_json::json!({
            "id": "capture-link-mode", "type":"bash",
            "command": format!("printf '%s=%s\\n' '{name}' \"${{RECIPE_VAR_pr_issue_link:-unset}}\" >> \"$CASE_ROOT/boundaries\"")
        })).unwrap());
        if name == "autodrive-build" {
            steps.push(controlled(
                "fixture-preflight",
                "build_preflight",
                serde_json::json!({"should_run":"true"}),
            ));
            steps.push(controlled("fixture-directive", "no_merge_directive", serde_json::json!({"task_description":"Honour caller issue linkage", "requirements":"leave open"})));
        }
        if name == "workflow-publish" {
            steps.push(controlled(
                "fixture-issue",
                "issue_number",
                serde_json::json!("2550"),
            ));
            steps.push(controlled(
                "fixture-host",
                "remote_host_type",
                serde_json::json!("github"),
            ));
            steps.push(controlled(
                "fixture-worktree",
                "worktree_setup",
                f.worktree_context(),
            ));
            steps.push(controlled(
                "fixture-terminal",
                "terminal_state",
                serde_json::json!({"terminal_success":"false", "should_publish":"true"}),
            ));
            doc["context"]["goal_already_met"] = "false".into();
            steps.push(step(name, "step-15-commit-push"));
        }
        steps.push(boundary);
        doc["steps"] = serde_yaml::Value::Sequence(steps);
        doc["output"] = serde_yaml::to_value(
            serde_json::json!({"format":"text", "template":"fixture complete"}),
        )
        .unwrap();
        fs::write(
            recipes.join(format!("{name}.yaml")),
            serde_yaml::to_string(&doc).unwrap(),
        )
        .unwrap();
    }
    let mut cmd = f.child(pinned_runner());
    cmd.arg(recipes.join("auto-drive-to-merge.yaml"))
        .args(["--working-dir"])
        .arg(&f.repo)
        .args(["--no-auto-stage", "--output-format", "json"])
        .env("RECIPE_VAR_pr_issue_link", stale)
        .env("PR_ISSUE_LINK", stale)
        .env("pr_issue_link", stale);
    if let Some(mode) = mode {
        cmd.args(["-c", &format!("pr_issue_link={mode}")]);
    }
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    let wanted = mode.unwrap_or("auto");
    assert_eq!(
        f.read("boundaries").lines().collect::<Vec<_>>(),
        [
            format!("auto-drive-to-merge={wanted}"),
            format!("autodrive-build={wanted}"),
            format!("default-workflow={wanted}"),
            format!("workflow-publish={wanted}"),
        ],
        "all four production recipe contexts must retain the caller's value"
    );
    f.assert_published(&out, expected);
    let message = f.git(&["log", "-1", "--format=%B"]);
    assert!(message.lines().any(|line| line == expected), "{message}");
}

#[test]
fn caller_refs_traverses_all_four_recipe_boundaries_into_actual_commit_and_pr() {
    run(Some("refs"), "closes", "Refs #2550");
}

#[test]
fn omitted_mode_uses_auto_at_every_boundary_despite_stale_bare_aliases() {
    run(None, "refs", "Closes #2550");
}
