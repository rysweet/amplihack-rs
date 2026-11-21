//! Execute the real step with the pinned runner; helper stderr is insufficient.
use super::fixture::{BRANCH, PublishFixture, describe, root};
use std::fs;
use std::path::PathBuf;

/// `recipe-runner-rs` at the revision in `claude-plugin/recipe-runner.rev`,
/// from the shared helper (#2658): an override that passes the receipt check,
/// or a build of the pin cached in the test target directory. Never the host's
/// installed runner. A missing or mismatched runner fails the test.
fn pinned_runner() -> PathBuf {
    crate::pinned_runner::pinned_runner(&root())
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
        .arg("--log-dir")
        .arg(f.tmp.path().join("runner-logs"))
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

fn failed_publish_result(f: &PublishFixture, diagnostic: &str) -> serde_json::Value {
    // The runner retains failed-step stdout separately from its serialized error.
    let logs: Vec<_> = fs::read_dir(f.tmp.path().join("runner-logs"))
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("amplihack-failed-bash-stdout-")
        })
        .collect();
    assert_eq!(logs.len(), 1, "{logs:?}");
    let published: serde_json::Value =
        serde_json::from_slice(&fs::read(logs[0].path()).unwrap()).unwrap();
    assert_eq!(published["state"], "FAILED_PR_CREATE");
    assert_eq!(published["terminal_status"], "failure");
    assert_eq!(published["pr_url"], "");
    assert_eq!(published["pr_number"], "");
    let message = published["message"].as_str().unwrap();
    assert!(message.contains(diagnostic), "{message}");
    published
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

#[test]
fn followup_actual_runner_accepts_canonical_repository_casing() {
    let f = PublishFixture::new();
    f.git(&[
        "remote",
        "set-url",
        "origin",
        "https://github.com/Owner/Repo.git",
    ]);
    let result = run_step(&f, &[]);
    assert_eq!(result["success"], true, "{result}");
    assert_eq!(result["step_results"][0]["status"], "completed");
    let published = &result["context"]["pr_publish_result"];
    assert_eq!(published["state"], "FOLLOWUP_CREATED");
    assert_eq!(published["pr_number"], "99");
    assert_eq!(published["pr_url"], "https://github.com/owner/repo/pull/99");
}

#[test]
fn followup_actual_runner_keeps_rejected_create_identity_private() {
    let f = PublishFixture::new();
    let rejected = "https://github.com/foreign-owner/private-repo/pull/99";
    let result = run_step(&f, &[("CREATE_OUTPUT", rejected)]);
    let error = failed_step_error(&result);
    assert!(error.contains("invalid PR identity URL"), "{error}");
    assert!(!result.to_string().contains(rejected), "{result}");
}

#[test]
fn followup_actual_runner_keeps_final_rejection_after_long_retry_warning() {
    let f = PublishFixture::new();
    f.long_transient_create_failure();
    let secret = "sk-ant-api03-ABCDEFGHIJKLMNOPQRST";
    f.write(
        "create.stderr",
        &format!(
            "distinctive final creation rejection {secret} {} beyond-diagnostic-limit",
            "x".repeat(640)
        ),
    );
    let result = run_step(&f, &[("CREATE_STATUS", "23")]);
    let error = failed_step_error(&result);
    assert!(
        error.contains("distinctive final creation rejection"),
        "{error}"
    );
    assert!(error.contains("gh pr create"), "{error}");
    assert!(error.contains("exit 23"), "{error}");
    assert!(error.contains("REDACTED_TOKEN"), "{error}");
    assert!(!error.contains("beyond-diagnostic-limit"), "{error}");
    assert!(!result.to_string().contains(secret), "{result}");
    assert!(
        !result.to_string().contains("ghp_ABCDEFGHIJKLMNOPQRSTUVWX"),
        "{result}"
    );
}

#[test]
fn failed_retry_waits_preserve_status_in_publish_result_and_pinned_runner() {
    let secret = "ghp_ABCDEFGHIJKLMNOPQRSTUVWX";
    for (failure, operation) in [
        (
            "Post \"https://api.github.com/graphql\": dial tcp: i/o timeout",
            "transient backoff wait",
        ),
        (
            "HTTP 429: secondary rate limit exceeded",
            "rate-limit reset wait",
        ),
    ] {
        let f = PublishFixture::new();
        f.offer_collision_pr(BRANCH);
        f.create_fails_once(&format!("{failure} {secret}"));
        f.shim("sleep", "exit 76\n");
        let diagnostic = format!("{operation} failed (exit 76)");
        let result = run_step(&f, &[]);
        let error = failed_step_error(&result);
        assert!(
            error.contains("Command failed (exit 76)") && error.contains(&diagnostic),
            "{error}"
        );
        assert!(error.contains("REDACTED_TOKEN"), "{error}");
        assert!(!result.to_string().contains(secret), "{result}");
        let published = failed_publish_result(&f, &diagnostic);
        assert!(!published.to_string().contains(secret), "{published}");
        f.assert_create_attempts_and_scoped_lookups(1, 1);
        f.assert_private_cleanup();
    }
}

#[test]
fn actual_runner_keeps_body_temp_failure_visible_when_a_matching_pr_appears() {
    let f = PublishFixture::new();
    f.offer_collision_pr(BRANCH);
    f.fail_body_temp_creation();
    let result = run_step(&f, &[]);
    let error = failed_step_error(&result);
    let diagnostic = "body temp creation failed (exit 77)";
    assert!(
        error.contains("Command failed (exit 77)") && error.contains(diagnostic),
        "{error}"
    );
    failed_publish_result(&f, diagnostic);
    assert!(f.tmp.path().join("collision-ready").is_file());
    f.assert_create_attempts_and_scoped_lookups(0, 1);
    f.assert_private_cleanup();
}

#[test]
fn actual_runner_does_not_retry_branch_names_or_explicit_validation_errors() {
    for rejection in [
        "HTTP 422 Validation Failed: no commits between main and fix/network",
        "HTTP 422: invalid head fix/network (Retry-After: 5)",
        "pull request create failed: GraphQL: head SHA is blank for fix/network",
    ] {
        let f = PublishFixture::new();
        f.git(&["branch", "-m", "fix/network"]);
        f.write("create.stderr", rejection);
        let result = run_step(&f, &[("CREATE_STATUS", "23")]);
        let error = failed_step_error(&result);
        assert!(
            error.contains("Command failed (exit 23)") && error.contains(rejection),
            "{error}"
        );
        failed_publish_result(&f, rejection);
        f.assert_create_attempts_and_scoped_lookups(1, 2);
        f.assert_private_cleanup();
    }
}

#[test]
fn actual_runner_pins_both_writes_to_origin_with_a_selected_base_and_branch_alias() {
    let f = PublishFixture::new();
    f.git(&["update-ref", "refs/remotes/origin/dev", "origin/main"]);
    let alias = format!("{BRANCH}--wt-1");
    f.git(&["branch", "-m", &alias]);
    f.git(&["config", &format!("branch.{alias}.remote"), "origin"]);
    f.git(&[
        "config",
        &format!("branch.{alias}.merge"),
        &format!("refs/heads/{BRANCH}"),
    ]);
    let result = run_step(
        &f,
        &[
            ("GH_REPO", "elsewhere/wrong"),
            ("WORKFLOW_PR_LABELS", "review"),
            ("RECIPE_VAR_base_branch", "dev"),
            ("RECIPE_VAR_strict_delivery", "true"),
            ("RECIPE_VAR_repository", "plan/tasks"),
            ("RECIPE_VAR_pr_repository", "owner/repo"),
            ("RECIPE_VAR_closing_reference", "plan/tasks#2306"),
        ],
    );
    assert_eq!(result["success"], true, "{result}");
    assert_eq!(result["context"]["pr_publish_result"]["base_branch"], "dev");
    let log = f.read("gh.log");
    assert!(
        log.lines().any(|line| line.starts_with(&format!(
            "pr create --draft --repo github.com/owner/repo --base=dev --head={BRANCH} "
        ))),
        "{log}"
    );
    assert!(
        log.lines()
            .any(|line| line == "pr edit 99 --repo github.com/owner/repo --add-label review"),
        "{log}"
    );
    assert!(
        log.contains("issue view 2306 --repo plan/tasks --json title"),
        "{log}"
    );
    f.assert_create_attempts_and_scoped_lookups(1, 1);
    f.assert_private_cleanup();
}
