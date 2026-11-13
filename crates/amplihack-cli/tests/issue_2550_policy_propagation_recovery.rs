//! Real runner tests retaining production defaults, nested recipe mappings and
//! recovery policy. Agent work is replaced by deterministic Rust checkpoints;
//! publication still executes the actual production commit and publish steps.
#![cfg(unix)]

use crate::support::issue_2550::{Fixture, describe, recipe, step, step_command, succeeds};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn runner() -> PathBuf {
    std::env::var_os("RECIPE_RUNNER_RS_PATH")
        .or_else(|| std::env::var_os("RECIPE_RUNNER_QA_BINARY"))
        .map(PathBuf::from)
        .or_else(|| {
            std::env::split_paths(&std::env::var_os("PATH")?)
                .map(|p| p.join("recipe-runner-rs"))
                .find(|p| p.is_file())
        })
        .expect("the pinned real recipe-runner-rs is required; no stub or silent skip")
        .canonicalize()
        .unwrap()
}

fn document(name: &str, steps: Vec<serde_yaml::Value>) -> serde_yaml::Value {
    let mut doc = recipe(name);
    doc["steps"] = serde_yaml::Value::Sequence(steps);
    // Keep production defaults/mappings/recovery. Presentation and prerequisite
    // declarations refer to the omitted agent work, so they are not executed.
    doc.as_mapping_mut()
        .unwrap()
        .remove(serde_yaml::Value::String("output".into()));
    doc.as_mapping_mut()
        .unwrap()
        .remove(serde_yaml::Value::String("inputs".into()));
    doc
}

fn bash_step(id: &str, command: &str) -> serde_yaml::Value {
    serde_yaml::to_value(json!({"id":id,"type":"bash","auto_stage":false,"command":command}))
        .unwrap()
}

fn write_doc(dir: &Path, name: &str, doc: &serde_yaml::Value) {
    fs::write(
        dir.join(format!("{name}.yaml")),
        serde_yaml::to_string(doc).unwrap(),
    )
    .unwrap();
}

fn mappings(f: &Fixture) -> PathBuf {
    let dir = f.bundle.join("amplifier-bundle/recipes");
    fs::create_dir_all(&dir).unwrap();
    for (name, id) in [
        ("auto-drive-to-merge", "autodrive-build"),
        ("autodrive-build", "step-02-default-workflow"),
        ("default-workflow", "workflow-publish"),
    ] {
        let mut call = step(name, id);
        // Only the build preflight agent is omitted; its deterministic result
        // is supplied as context. Forwarding itself is never reconstructed.
        call.as_mapping_mut()
            .unwrap()
            .remove(serde_yaml::Value::String("condition".into()));
        let mut doc = document(name, vec![call]);
        if name == "autodrive-build" {
            doc["context"]["no_merge_directive"] = serde_yaml::to_value(json!({
                "task_description":"Fix reference selection", "requirements":"Keep the issue open"
            }))
            .unwrap();
        }
        write_doc(&dir, name, &doc);
    }
    write_doc(
        &dir,
        "workflow-publish",
        &document(
            "workflow-publish",
            vec![bash_step("acceptance-publish", "boundary publish")],
        ),
    );
    f.write(
        "commit-command",
        step_command("workflow-publish", "step-15-commit-push"),
    );
    f.write(
        "publish-command",
        step_command("workflow-publish", "step-16-create-draft-pr"),
    );
    dir
}

fn real_run(f: &Fixture, path: &Path) -> Command {
    let mut c = f.cli();
    c.args(["recipe", "run"])
        .arg(path)
        .args(["--no-detach", "--format", "json", "-w"])
        .arg(&f.worktree)
        .arg("-c")
        .arg(format!("repo_path={}", f.main.display()))
        .arg("-c")
        .arg(format!(
            "worktree_setup={}",
            serde_json::json!({
                "worktree_path": f.worktree, "branch_name": crate::support::issue_2550::BRANCH,
                "base_ref": "origin/main"
            })
        ))
        .args([
            "-c",
            "issue_number=42",
            "-c",
            "remote_host_type=github",
            "-c",
            "task_description=Fix reference selection",
        ])
        .env("RECIPE_RUNNER_RS_PATH", runner())
        .env("AMPLIHACK_AGENT_BINARY", "claude")
        .env(
            "ISSUE_2550_REAL_AMPLIHACK",
            crate::support::amplihack_bin::amplihack_bin(),
        )
        // Both bare aliases and a stale prefixed value must be replaced by
        // the context exported at each boundary.
        .env("PR_ISSUE_LINK", "closes")
        .env("pr_issue_link", "closes")
        .env("RECIPE_VAR_pr_issue_link", "closes");
    c
}

#[test]
fn nested_production_mappings_carry_refs_to_the_commit_and_persisted_pr_body() {
    let f = Fixture::new();
    f.change("src/feature.rs");
    let dir = mappings(&f);
    let out = real_run(&f, &dir.join("auto-drive-to-merge.yaml"))
        .args(["-c", "pr_issue_link=refs"])
        .output()
        .unwrap();
    succeeds(&out);
    assert!(
        f.read("events").lines().any(|s| s == "boundary:refs"),
        "{}\n{}",
        f.read("events"),
        describe(&out)
    );
    assert!(
        f.git(&["log", "-1", "--format=%B"])
            .lines()
            .any(|s| s == "Refs #42")
    );
    assert!(
        f.read("body").lines().any(|s| s == "Refs #42"),
        "{}",
        f.read("body")
    );
}

#[test]
fn omitted_policy_overrides_conflicting_inherited_environment_at_every_entry_boundary() {
    for name in [
        "auto-drive-to-merge",
        "autodrive-build",
        "default-workflow",
        "workflow-publish",
    ] {
        let f = Fixture::new();
        f.change("src/feature.rs");
        let dir = mappings(&f);
        let out = real_run(&f, &dir.join(format!("{name}.yaml")))
            .env("PR_ISSUE_LINK", "refs")
            .env("pr_issue_link", "refs")
            .env("RECIPE_VAR_pr_issue_link", "refs")
            .output()
            .unwrap();
        succeeds(&out);
        assert!(
            f.read("events").lines().any(|s| s == "boundary:auto"),
            "{name}: {}",
            f.read("events")
        );
        assert!(
            f.read("body").lines().any(|s| s == "Closes #42"),
            "{name}: {}",
            f.read("body")
        );
    }
}

#[test]
fn invalid_normal_and_publication_entry_fail_before_the_first_work_step() {
    for name in ["default-workflow", "workflow-publish"] {
        let f = Fixture::new();
        let dir = f.root.join("entry");
        fs::create_dir(&dir).unwrap();
        // Execute the genuine entry bash steps, stopping at the first agent
        // or nested phase. The sentinel makes late validation observable.
        let original = recipe(name);
        let mut prefix: Vec<_> = original["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .take_while(|s| s["type"].as_str() == Some("bash"))
            .cloned()
            .collect();
        prefix.push(bash_step("entry-sentinel", "boundary inspect"));
        write_doc(&dir, name, &document(name, prefix));
        for invalid in ["", "invalid"] {
            let out = real_run(&f, &dir.join(format!("{name}.yaml")))
                .args(["-c", &format!("pr_issue_link={invalid}")])
                .output()
                .unwrap();
            assert!(!out.status.success(), "{name}: {}", describe(&out));
            assert!(
                String::from_utf8_lossy(&out.stderr).contains("pr_issue_link"),
                "{}",
                describe(&out)
            );
            assert!(
                !f.read("events").contains("boundary:"),
                "{name} performed work before validation"
            );
        }
    }
}

#[test]
fn real_publication_recipe_never_reaches_pr_creation_after_a_rejected_hook() {
    for hook in ["pre-commit", "pre-push"] {
        let f = Fixture::new();
        f.change("src/feature.rs");
        f.write(&format!("reject-{hook}"), "yes");
        let upstream = f.git(&["rev-parse", "@{u}"]);
        let dir = f.root.join("publication");
        fs::create_dir(&dir).unwrap();
        let doc = document(
            "workflow-publish",
            vec![
                step("workflow-publish", "step-15-commit-push"),
                step("workflow-publish", "step-16-create-draft-pr"),
            ],
        );
        write_doc(&dir, "workflow-publish", &doc);
        let out = real_run(&f, &dir.join("workflow-publish.yaml"))
            .args([
                "-c",
                "pr_issue_link=refs",
                "-c",
                "goal_already_met=false",
                "-c",
                "terminal_state={\"terminal_success\":\"false\",\"should_publish\":\"true\"}",
            ])
            .output()
            .unwrap();
        assert!(!out.status.success(), "{hook}: {}", describe(&out));
        assert!(
            f.read("events").lines().any(|s| s == hook),
            "{hook} never ran: {}",
            describe(&out)
        );
        assert_eq!(f.git(&["rev-parse", "@{u}"]), upstream);
        assert!(
            !f.read("events").contains("pr create"),
            "rejected hook reached PR creation"
        );
    }
}

fn recovery_recipe(f: &Fixture) -> PathBuf {
    let dir = f.root.join("recovery-recipes");
    fs::create_dir(&dir).unwrap();
    let mut checkpoint = bash_step(
        "workflow-prep",
        "boundary inspect \"$RECIPE_VAR_pr_issue_link\"",
    );
    checkpoint["output"] = serde_yaml::Value::String("workflow_init".into());
    checkpoint["parse_json"] = serde_yaml::Value::Bool(true);
    checkpoint["recovery"] = serde_yaml::to_value(json!({"replay":"reuse"})).unwrap();
    let stop = bash_step("workflow-design", "checkpoint");
    let publish = bash_step("workflow-publish", "boundary publish");
    write_doc(
        &dir,
        "default-workflow",
        &document("default-workflow", vec![checkpoint, stop, publish]),
    );
    f.write(
        "commit-command",
        step_command("workflow-publish", "step-15-commit-push"),
    );
    f.write(
        "publish-command",
        step_command("workflow-publish", "step-16-create-draft-pr"),
    );
    dir.join("default-workflow.yaml")
}

fn recovery_trees(root: &Path) -> Vec<PathBuf> {
    let mut found = vec![];
    if !root.is_dir() {
        return found;
    }
    for e in fs::read_dir(root).unwrap() {
        let e = e.unwrap();
        if e.file_type().unwrap().is_dir() {
            found.extend(recovery_trees(&e.path()));
        } else if e.file_name() == "tree.json" {
            found.push(e.path());
        }
    }
    found
}

fn stopped_run(f: &Fixture, recipe: &Path, policy: &str) -> PathBuf {
    let out = real_run(f, recipe)
        .args(["-c", &format!("pr_issue_link={policy}")])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "checkpoint must stop before publication"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("usage_limit"),
        "checkpoint must be reached: {}",
        describe(&out)
    );
    let trees = recovery_trees(&f.root.join("runs"));
    assert_eq!(trees.len(), 1, "durable recovery tree missing: {trees:?}");
    trees[0].clone()
}

#[test]
fn recovery_persists_the_policy_and_reuses_the_checkpoint_under_conflicting_environment() {
    let f = Fixture::new();
    f.change("src/feature.rs");
    let recipe = recovery_recipe(&f);
    let tree = stopped_run(&f, &recipe, "refs");
    let saved: Value = serde_json::from_slice(&fs::read(&tree).unwrap()).unwrap();
    assert!(
        saved["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["probe"]["inputs"]["pr_issue_link"] == "refs"),
        "policy was not persisted: {saved:#}"
    );
    f.write("continue", "yes");
    // Recovery must carry the saved policy without requiring the caller to
    // remember and repeat it on the relaunch command line.
    let out = real_run(&f, &recipe).output().unwrap();
    succeeds(&out);
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["recovery"]["mode"], "resumed", "{result:#}");
    assert_eq!(
        f.read("events")
            .lines()
            .filter(|s| *s == "boundary:refs")
            .count(),
        2,
        "inspection must be restored, while publication runs exactly once"
    );
    assert!(
        f.git(&["log", "-1", "--format=%B"])
            .lines()
            .any(|s| s == "Refs #42")
    );
    assert!(f.read("body").lines().any(|s| s == "Refs #42"));
}

#[test]
fn restored_invalid_policy_cannot_become_auto_and_legacy_absence_defaults_to_auto() {
    for restored in [None, Some("invalid"), Some("")] {
        let f = Fixture::new();
        f.change("src/feature.rs");
        let recipe = recovery_recipe(&f);
        let path = stopped_run(&f, &recipe, "auto");
        let mut saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        // Mutate only the exported-context delta in a real runner-written
        // record, as compatibility/fault injection; do not manufacture replay.
        let record = saved["records"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|r| r["probe"]["location"]["step_id"] == "workflow-prep")
            .expect("completed checkpoint must be recoverable");
        match restored {
            Some(value) => {
                record["delta"]["pr_issue_link"] = json!({"operation":"assign", "value":value})
            }
            None => {
                record["probe"]["inputs"]
                    .as_object_mut()
                    .unwrap()
                    .remove("pr_issue_link");
            }
        }
        fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
        f.write("continue", "yes");
        let out = real_run(&f, &recipe).output().unwrap();
        if restored.is_some() {
            assert!(
                !out.status.success(),
                "invalid restored policy published: {}",
                describe(&out)
            );
            assert!(
                String::from_utf8_lossy(&out.stderr).contains("pr_issue_link"),
                "{}",
                describe(&out)
            );
            assert!(!f.state.join("created").exists());
            assert!(!f.read("events").contains("pre-commit"));
        } else {
            succeeds(&out);
            assert!(f.read("body").lines().any(|s| s == "Closes #42"));
        }
    }
}
