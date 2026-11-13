//! Actual publication script, JSON output and downstream runner conditions.
//! These controls do not replace the original nine cases or 25 native controls.
use crate::issue_1554_fixture::{
    ALLOWED, Repo, describe, executable, sandbox, step, success, write,
};
use recipe_runner_rs::adapters::Adapter;
use recipe_runner_rs::runner::RecipeRunner;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::process::Command;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn publication(r: &Repo) -> Command {
    let mut command = r.step_command("workflow-publish", "step-15-commit-push");
    command
        .env(
            "RECIPE_VAR_final_requirements",
            json!({
                "version": 1, "task": "1554-fixture", "permitted_source_paths": ALLOWED
            })
            .to_string(),
        )
        .env(
            "RECIPE_VAR_design_spec",
            json!({
                "version": 1, "task": "1554-fixture", "files_to_change": ALLOWED
            })
            .to_string(),
        );
    command
}

fn structured(out: &std::process::Output) -> Value {
    let value: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|error| {
        panic!(
            "publication must emit exactly one JSON result: {error}\n{}",
            describe(out)
        )
    });
    assert_eq!(value["version"], 1);
    for key in ["committed", "reconciled", "pushed"] {
        assert!(value[key].is_boolean(), "missing Boolean {key}: {value}");
    }
    assert!(
        value
            .get("commit_id")
            .is_some_and(|v| v.is_null() || v.is_string())
    );
    assert!(value["push_reason"].is_string());
    assert!(value["updates"].is_array());
    assert!(value["errors"].is_array());
    value
}

#[test]
fn real_publication_emits_one_result_and_routes_hook_output_to_stderr() {
    sandbox(
        module_path!(),
        "real_publication_emits_one_result_and_routes_hook_output_to_stderr",
        || {
            let r = Repo::new();
            write(r.work.join("src/new.rs"), "authorised source\n");
            r.hook(
                "pre-commit",
                "#!/bin/sh\nprintf 'normal inventory hook ran\\n'\n",
            );
            let out = publication(&r).output().unwrap();
            success(&out);
            let value = structured(&out);
            assert_eq!(value["committed"], true);
            assert_eq!(value["reconciled"], true);
            assert_eq!(value["pushed"], true);
            assert_eq!(value["push_reason"], "success");
            assert_eq!(value["commit_id"], r.oid(&r.work, "HEAD"));
            assert!(!value["updates"].as_array().unwrap().is_empty());
            assert!(value["errors"].as_array().unwrap().is_empty());
            assert!(
                String::from_utf8_lossy(&out.stderr).contains("normal inventory hook ran"),
                "normal hook output was lost"
            );
            assert_eq!(
                r.oid(&r.remote, "refs/heads/feat/1554"),
                r.oid(&r.work, "HEAD")
            );
        },
    );
}

#[test]
fn already_current_publication_is_structured_success_without_a_commit() {
    sandbox(
        module_path!(),
        "already_current_publication_is_structured_success_without_a_commit",
        || {
            let r = Repo::new();
            let before = r.oid(&r.work, "HEAD");
            let out = publication(&r).output().unwrap();
            success(&out);
            let value = structured(&out);
            assert_eq!(value["committed"], false);
            assert!(value["commit_id"].is_null());
            assert_eq!(value["pushed"], true);
            assert_eq!(value["push_reason"], "already_current");
            assert_eq!(r.oid(&r.work, "HEAD"), before);
        },
    );
}

#[test]
fn policy_refusal_emits_structured_errors_and_preserves_status_78() {
    sandbox(
        module_path!(),
        "policy_refusal_emits_structured_errors_and_preserves_status_78",
        || {
            let r = Repo::new();
            let before = r.oid(&r.remote, "refs/heads/feat/1554");
            r.commit_file(&r.work, "unrelated.txt", "unauthorised outgoing source\n");
            let out = publication(&r).output().unwrap();
            assert_eq!(out.status.code(), Some(78), "{}", describe(&out));
            let value = structured(&out);
            assert_eq!(value["pushed"], false);
            assert_eq!(value["push_reason"], "policy_rejected");
            assert!(!value["errors"].as_array().unwrap().is_empty());
            assert_eq!(r.oid(&r.remote, "refs/heads/feat/1554"), before);
        },
    );
}

#[test]
fn partial_publication_reports_completed_and_failed_destination_updates() {
    sandbox(
        module_path!(),
        "partial_publication_reports_completed_and_failed_destination_updates",
        || {
            let r = Repo::new();
            let second = r.root.join("second.git");
            r.git(
                &r.root,
                &[
                    "clone",
                    "--quiet",
                    "--bare",
                    r.remote.to_str().unwrap(),
                    second.to_str().unwrap(),
                ],
            );
            let before = r.oid(&second, "refs/heads/feat/1554");
            executable(
                second.join("hooks/pre-receive"),
                "#!/bin/sh\necho 'fixture destination refuses update' >&2\nexit 1\n",
            );
            for destination in [&r.remote, &second] {
                r.git(
                    &r.work,
                    &[
                        "config",
                        "--add",
                        "remote.origin.pushurl",
                        destination.to_str().unwrap(),
                    ],
                );
            }
            let tip = r.commit_file(&r.work, "src/new.rs", "authorised task source\n");
            let out = publication(&r).output().unwrap();
            assert!(
                !out.status.success(),
                "a failed destination must retain a failing process outcome"
            );
            assert_eq!(r.oid(&r.remote, "refs/heads/feat/1554"), tip);
            assert_eq!(r.oid(&second, "refs/heads/feat/1554"), before);
            let value = structured(&out);
            assert_eq!(value["pushed"], false);
            assert_eq!(value["push_reason"], "partial");
            let updates = value["updates"].as_array().unwrap();
            assert!(
                updates.iter().any(|u| u["outcome"] == "pushed"),
                "completed update omitted: {value}"
            );
            assert!(
                updates.iter().any(|u| u["outcome"] == "failed"),
                "failed update omitted: {value}"
            );
            assert!(!value["errors"].as_array().unwrap().is_empty());
        },
    );
}

struct PublicationAdapter<'a> {
    repo: &'a Repo,
    pr_runs: Arc<AtomicUsize>,
}

impl Adapter for PublicationAdapter<'_> {
    fn execute_agent_step(
        &self,
        _prompt: &str,
        _agent: Option<&str>,
        _system: Option<&str>,
        _mode: Option<&str>,
        _cwd: &str,
        _model: Option<&str>,
        _timeout: Option<u64>,
    ) -> anyhow::Result<String> {
        anyhow::bail!("this contract executes no agent")
    }

    fn execute_bash_step(
        &self,
        command: &str,
        _cwd: &str,
        timeout: Option<u64>,
        extra_env: &HashMap<String, String>,
    ) -> anyhow::Result<String> {
        assert_eq!(
            timeout, None,
            "publication controls use completion, not timers"
        );
        if command == "fixture-pr-admitted" {
            self.pr_runs.fetch_add(1, Ordering::SeqCst);
            return Ok(json!({"pr_would_be_created": true}).to_string());
        }
        assert_eq!(
            command,
            step("workflow-publish", "step-15-commit-push")["command"]
                .as_str()
                .unwrap()
        );
        let out = publication(self.repo).envs(extra_env).output()?;
        anyhow::ensure!(out.status.success(), "{}", describe(&out));
        Ok(String::from_utf8(out.stdout)?)
    }

    fn is_available(&self) -> bool {
        true
    }
    fn name(&self) -> &str {
        "issue-1554-real-publication"
    }
}

#[test]
fn real_runner_admits_pr_creation_after_parsing_an_already_current_result() {
    sandbox(
        module_path!(),
        "real_runner_admits_pr_creation_after_parsing_an_already_current_result",
        || {
            let r = Repo::new();
            let publish = step("workflow-publish", "step-15-commit-push");
            assert_eq!(publish["parse_json"].as_bool(), Some(true));
            let mut pr = step("workflow-publish", "step-16-create-draft-pr");
            assert!(
                pr["condition"]
                    .as_str()
                    .unwrap()
                    .contains("commit_result.pushed == 'true'")
            );
            // Replace the external PR side effect, retaining the actual condition.
            pr["command"] = serde_yaml::Value::String("fixture-pr-admitted".to_owned());
            let yaml = serde_yaml::to_string(&json!({
                "name": "issue-1554-publication-result", "description": "actual JSON consumer",
                "version": "1.0.0", "context": {}, "steps": [publish, pr]
            }))
            .unwrap();
            let parsed = recipe_runner_rs::parse_recipe(&yaml).unwrap();
            let mut context = HashMap::new();
            context.insert("goal_already_met".to_owned(), json!("false"));
            context.insert(
                "terminal_state".to_owned(),
                json!({"terminal_success": "false", "should_publish": "true"}),
            );
            context.insert("repo_path".to_owned(), json!(r.main));
            context.insert(
                "worktree_setup".to_owned(),
                json!({"worktree_path": r.work, "branch_name": "feat/1554"}),
            );
            context.insert(
                "final_requirements".to_owned(),
                json!({"version": 1, "task": "1554-fixture", "permitted_source_paths": ALLOWED}),
            );
            context.insert(
                "design_spec".to_owned(),
                json!({"version": 1, "task": "1554-fixture", "files_to_change": ALLOWED}),
            );
            let pr_runs = Arc::new(AtomicUsize::new(0));
            let result = RecipeRunner::new(PublicationAdapter {
                repo: &r,
                pr_runs: pr_runs.clone(),
            })
            .with_working_dir(r.work.to_str().unwrap())
            .with_auto_stage(false)
            .with_checkpoints(false)
            .execute(&parsed, Some(context));
            assert!(result.success, "actual recipe execution failed: {result:?}");
            assert_eq!(
                result.context["commit_result"]["pushed"], true,
                "runner did not parse the Boolean push result"
            );
            assert_eq!(
                pr_runs.load(Ordering::SeqCst),
                1,
                "actual downstream PR condition did not accept an already-current push"
            );
        },
    );
}

#[test]
fn post_commit_head_change_reports_the_created_commit_without_replacing_the_index() {
    sandbox(
        module_path!(),
        "post_commit_head_change_reports_the_created_commit_without_replacing_the_index",
        || {
            let r = Repo::new();
            r.protect_user_blob();
            let index = std::path::PathBuf::from(
                String::from_utf8(r.git(
                    &r.work,
                    &["rev-parse", "--path-format=absolute", "--git-path", "index"],
                ))
                .unwrap()
                .trim(),
            );
            let original_index = std::fs::read(&index).unwrap();
            let original_head = r.oid(&r.work, "HEAD");
            write(r.work.join("src/new.rs"), "authorised source\n");
            let created = r.root.join("created-commit");
            r.hook("post-commit", "#!/bin/sh\ngit rev-parse HEAD > \"$CREATED_COMMIT\" || exit $?\n# Select normal repository hooks for this independent ref mutation.\nunset GIT_CONFIG_PARAMETERS GIT_CONFIG_COUNT\ngit update-ref HEAD \"$ORIGINAL_HEAD\"\n");
            let out = publication(&r)
                .env("CREATED_COMMIT", &created)
                .env("ORIGINAL_HEAD", &original_head)
                .output()
                .unwrap();
            assert!(
                !out.status.success(),
                "concurrent HEAD movement must fail: {}",
                describe(&out)
            );
            let value = structured(&out);
            let actual_commit = std::fs::read_to_string(created).unwrap();
            assert_ne!(actual_commit.trim(), original_head);
            assert_eq!(value["committed"], true);
            assert_eq!(value["commit_id"], actual_commit.trim());
            assert_eq!(value["reconciled"], false);
            assert_eq!(value["pushed"], false);
            assert_eq!(r.oid(&r.work, "HEAD"), original_head);
            assert_eq!(std::fs::read(index).unwrap(), original_index);
        },
    );
}
