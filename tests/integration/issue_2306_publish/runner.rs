//! Execute the real step with the pinned runner; helper stderr is insufficient.
use super::fixture::{PublishFixture, describe, root};
use std::fs;
use std::path::PathBuf;

/// Require Cargo installation provenance, not just a matching version string.
/// Missing/mismatched runner is a hard prerequisite failure, never a skipped test.
fn pinned_runner() -> PathBuf {
    let pin = fs::read_to_string(root().join("claude-plugin/recipe-runner.rev")).unwrap();
    let candidate = std::env::var_os("RECIPE_RUNNER_RS_PATH")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .map(|p| p.join("recipe-runner-rs"))
                .find(|p| p.is_file())
        })
        .unwrap_or_else(|| {
            let cargo_home = std::env::var_os("CARGO_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap()).join(".cargo"));
            cargo_home.join("bin/recipe-runner-rs")
        });
    assert!(
        candidate.is_file(),
        "install the pinned runner with scripts/install-pinned-recipe-runner.sh"
    );
    let receipt = candidate
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(".crates2.json");
    let receipt: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&receipt)
            .unwrap_or_else(|e| panic!("runner provenance {}: {e}", receipt.display())),
    )
    .unwrap();
    assert!(
        receipt["installs"]
            .as_object()
            .unwrap()
            .keys()
            .any(|key| key.starts_with("recipe-runner-rs ")
                && key.contains("git+https://github.com/rysweet/amplihack-recipe-runner?")
                && key.contains(&format!("#{}", pin.trim()))),
        "runner must be installed from pin {}",
        pin.trim()
    );
    candidate
}

fn run_step(f: &PublishFixture, extra: &[(&str, &str)]) -> serde_json::Value {
    let recipe: serde_yaml::Value = serde_yaml::from_str(
        &fs::read_to_string(root().join("amplifier-bundle/recipes/workflow-publish.yaml")).unwrap(),
    )
    .unwrap();
    let step = recipe["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|s| s["id"].as_str() == Some("step-16-create-draft-pr"))
        .unwrap();
    // Copy the actual step, including condition, output and parse_json. Only
    // supply prior-step context; do not duplicate or approximate the command.
    let minimal = serde_json::json!({
        "name": "issue-2306-publish-boundary", "version": "1.0.0",
        "context": {
            "goal_already_met": "false",
            "terminal_state": {"terminal_success": "false", "should_publish": "true"},
            "commit_result": {"pushed": "true"},
            "worktree_setup": {"worktree_path": f.repo.canonicalize().unwrap()},
            "task_description": "Add task title"
        },
        "steps": [step]
    });
    let path = f.tmp.path().join("boundary.yaml");
    fs::write(&path, serde_yaml::to_string(&minimal).unwrap()).unwrap();
    let runner = pinned_runner();
    let mut command = f.command(runner.to_str().unwrap());
    command
        .arg(&path)
        .arg("--working-dir")
        .arg(&f.repo)
        .args(["--no-auto-stage", "--output-format", "json"])
        .envs(extra.iter().copied());
    let output = command.output().unwrap();
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "runner must serialize RecipeResult: {e}\n{}",
            describe(&output)
        )
    });
    assert_eq!(result["step_results"].as_array().unwrap().len(), 1);
    assert_eq!(
        result["step_results"][0]["step_id"],
        "step-16-create-draft-pr"
    );
    assert_eq!(
        output.status.success(),
        result["success"].as_bool().unwrap()
    );
    result
}

fn failed_step_error(result: &serde_json::Value) -> &str {
    assert_eq!(result["success"], false);
    let step = &result["step_results"][0];
    assert_eq!(
        step["status"], "failed",
        "actual step must run and fail, not be skipped"
    );
    let error = step["error"].as_str().unwrap();
    assert!(
        !error.trim().is_empty(),
        "serialized StepResult.error must never be empty"
    );
    error
}

#[test]
fn actual_runner_step_error_includes_create_operation_status_and_distinctive_gh_stderr() {
    let f = PublishFixture::new();
    f.write("create.stderr", "distinctive publish rejection");
    let result = run_step(&f, &[("CREATE_STATUS", "23")]);
    let error = failed_step_error(&result);
    for needle in ["gh pr create", "23", "distinctive publish rejection"] {
        assert!(
            error.contains(needle),
            "serialized step error lost {needle:?}: {error:?}"
        );
    }
}

#[test]
fn actual_runner_step_error_explains_empty_gh_stderr() {
    let f = PublishFixture::new();
    let result = run_step(&f, &[("CREATE_STATUS", "23")]);
    let error = failed_step_error(&result);
    assert!(
        error.contains("gh pr create") && error.contains("23"),
        "{error:?}"
    );
    assert!(
        error.contains("stderr") && (error.contains("empty") || error.contains("no error")),
        "{error:?}"
    );
}

#[test]
fn actual_runner_serialized_errors_redact_credentials() {
    let f = PublishFixture::new();
    let secrets = [
        "sk-ant-api03-ABCDEFGHIJKLMNOPQRST",
        "AKIAABCDEFGHIJKLMNOP",
        "ASIAABCDEFGHIJKLMNOP",
        "ghp_ABCDEFGHIJKLMNOPQRSTUVWX",
        "https://user:pass@github.com/private",
    ];
    f.write(
        "create.stderr",
        &format!("distinctive publish rejection {}", secrets.join(" ")),
    );
    let result = run_step(&f, &[("CREATE_STATUS", "23")]);
    let error = failed_step_error(&result);
    assert!(
        error.contains("distinctive publish rejection"),
        "sanitized provider context must survive: {error}"
    );
    for secret in secrets {
        assert!(
            !result.to_string().contains(secret),
            "secret leaked in runner result"
        );
    }
}

#[test]
fn actual_runner_reports_recovery_as_success_with_parsed_publish_result() {
    let f = PublishFixture::new();
    f.inject_scope_failure();
    let result = run_step(&f, &[]);
    assert_eq!(
        result["success"], true,
        "recovered draft must complete the recipe: {result}"
    );
    assert_eq!(result["step_results"][0]["status"], "completed");
    assert_eq!(
        result["context"]["pr_publish_result"]["terminal_status"],
        "success"
    );
    assert_eq!(
        result["context"]["pr_publish_result"]["state"],
        "FOLLOWUP_CREATED"
    );
}

#[test]
fn actual_runner_success_parses_the_existing_publish_result_contract() {
    let f = PublishFixture::new();
    let result = run_step(&f, &[]);
    assert_eq!(result["success"], true, "{result}");
    assert_eq!(result["step_results"][0]["status"], "completed");
    assert_eq!(
        result["context"]["pr_publish_result"]["state"],
        "FOLLOWUP_CREATED"
    );
    assert_eq!(
        result["context"]["pr_publish_result"]["terminal_status"],
        "success"
    );
}

#[test]
fn actual_runner_reports_pre_helper_failure_with_operation_and_status() {
    let f = PublishFixture::new();
    f.shim(
        "git",
        r#"
if [ "${1:-}" = 'branch' ] && [ "${2:-}" = '--show-current' ]; then exit 79; fi
exec "$REAL_TOOL" "$@"
"#,
    );
    let result = run_step(&f, &[]);
    let error = failed_step_error(&result);
    assert!(
        error.contains("git branch") && error.contains("79"),
        "pre-helper error needs operation/status: {error:?}"
    );
    assert!(!f.read("gh.log").contains("pr create"));
}

#[test]
fn persistent_awk_failure_still_serializes_success_after_exactly_one_creation() {
    let f = PublishFixture::new();
    f.shim("awk", "exit 72\n");
    let result = run_step(&f, &[]);
    assert_eq!(result["success"], true, "{result}");
    assert_eq!(result["step_results"][0]["status"], "completed");
    let published = &result["context"]["pr_publish_result"];
    assert_eq!(published["pr_number"], "99");
    assert_eq!(published["pr_url"], "https://github.com/owner/repo/pull/99");
    assert_eq!(published["terminal_status"], "success");
    assert_eq!(
        f.read("gh.log")
            .lines()
            .filter(|s| s.starts_with("pr create "))
            .count(),
        1
    );
}
