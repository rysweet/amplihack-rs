//! #1554: actual recipe policy and native template-based removal diagnostics.
//!
//! These newly derived controls do NOT replace the original 25 controls held
//! on candide. Runner #201 acceptance additionally needs the owner's release
//! and pin-adoption evidence. No runner source or pin is changed by this suite.

use crate::issue_1554_fixture::{Repo, recipe, source_root, step, write};
use crate::support::quiet_process::{child_remove_var, child_set_var, in_quiet_process};
use recipe_runner_rs::adapters::Adapter;
use recipe_runner_rs::runner::RecipeRunner;
use serde_yaml::Value;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

// The round and repair recipes are invoked from shell helpers, so they are
// additional roots rather than YAML `type: recipe` edges.
const ROOTS: &[&str] = &[
    "default-workflow",
    "consensus-workflow",
    "auto-drive-to-merge",
    "autodrive-crusty-round",
    "autodrive-merge-round",
    "hook-rejection-repair",
    "loop-health-evaluator",
];

fn roles(name: &str, seen: &mut BTreeSet<String>, result: &mut Vec<(String, Value)>) {
    if !seen.insert(name.to_owned()) {
        return;
    }
    for s in recipe(name)["steps"].as_sequence().expect("recipe steps") {
        if s["agent"].as_str().is_some() || s["type"].as_str() == Some("agent") {
            result.push((name.to_owned(), s.clone()));
        }
        if let Some(child) = s["recipe"].as_str() {
            assert!(
                !child.contains("{{"),
                "inventory dynamic nested recipe explicitly: {name}/{child}"
            );
            roles(child.trim_end_matches(".yaml"), seen, result);
        }
    }
}

fn applicable_roles() -> Vec<(String, Value)> {
    let mut found = Vec::new();
    let mut seen = BTreeSet::new();
    for root in ROOTS {
        roles(root, &mut seen, &mut found);
    }
    assert!(
        !found.is_empty(),
        "empty role inventory cannot establish staging policy"
    );
    found
}

#[test]
fn every_reachable_default_consensus_and_autodrive_agent_explicitly_disables_staging() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "every_reachable_default_consensus_and_autodrive_agent_explicitly_disables_staging",
        || {
            let missing: Vec<_> = applicable_roles()
                .into_iter()
                .filter(|(_, s)| s["auto_stage"].as_bool() != Some(false))
                .map(|(name, s)| format!("{name}/{}", s["id"].as_str().unwrap()))
                .collect();
            assert!(
                missing.is_empty(),
                "missing explicit auto_stage: false:\n{}",
                missing.join("\n")
            );
        },
    );
}

#[test]
fn crusty_review_step_02_has_its_own_explicit_opt_out() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "crusty_review_step_02_has_its_own_explicit_opt_out",
        || {
            assert_eq!(
                step("autodrive-crusty-round", "step-02-crusty-review")["auto_stage"].as_bool(),
                Some(false)
            );
        },
    );
}

#[test]
fn concern_fix_step_04_has_its_own_explicit_opt_out() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "concern_fix_step_04_has_its_own_explicit_opt_out",
        || {
            assert_eq!(
                step("autodrive-crusty-round", "step-04-address-concerns")["auto_stage"].as_bool(),
                Some(false)
            );
        },
    );
}

/// Replaces the model only. The real pinned runner still renders and executes
/// the actual working_dir and applies its own automatic staging implementation.
struct SourceAndRuntimeWriter;

impl Adapter for SourceAndRuntimeWriter {
    fn execute_agent_step(
        &self,
        _prompt: &str,
        _agent_name: Option<&str>,
        _system_prompt: Option<&str>,
        _mode: Option<&str>,
        working_dir: &str,
        _model: Option<&str>,
        timeout: Option<u64>,
    ) -> anyhow::Result<String> {
        assert_eq!(timeout, None, "these controls have no timer");
        let cwd = Path::new(working_dir);
        assert!(
            cwd.join(".git").exists(),
            "agent ran outside its rendered checkout: {working_dir}"
        );
        write(cwd.join("src/allowed.rs"), "intended agent source\n");
        write(cwd.join(".amplihack/indexing.pid"), "fixture runtime\n");
        write(
            cwd.join(".claude/runtime/sessions.jsonl.lock"),
            "fixture lock\n",
        );
        Ok("{}".to_owned())
    }

    fn execute_bash_step(
        &self,
        _command: &str,
        _working_dir: &str,
        _timeout: Option<u64>,
        _extra_env: &HashMap<String, String>,
    ) -> anyhow::Result<String> {
        anyhow::bail!("native opt-out controls must execute exactly one agent role")
    }
    fn is_available(&self) -> bool {
        true
    }
    fn name(&self) -> &str {
        "issue-1554-source-and-runtime-writer"
    }
}

fn native_run(r: &Repo, original: &Value, remove_opt_out: bool) -> Vec<u8> {
    let mut s = original.clone();
    let fields = s.as_mapping_mut().unwrap();
    // Only model execution/prompt routing is replaced. Preserve the production
    // working_dir template byte for byte in both halves of the control.
    for field in [
        "condition",
        "parallel_group",
        "timeout",
        "recovery",
        "model",
        "provider",
    ] {
        fields.remove(Value::String(field.to_owned()));
    }
    fields.insert(
        Value::String("prompt".to_owned()),
        Value::String("write fixture source and runtime".to_owned()),
    );
    fields.insert(
        Value::String("agent".to_owned()),
        Value::String("fixture".to_owned()),
    );
    fields.insert(Value::String("parse_json".to_owned()), Value::Bool(false));
    if remove_opt_out {
        fields.remove(Value::String("auto_stage".to_owned()));
    }
    assert_eq!(
        s["working_dir"], original["working_dir"],
        "never materialise a native working_dir control"
    );
    let yaml = serde_yaml::to_string(&serde_json::json!({
        "name": "issue-1554-native-removal", "description": "real runner staging boundary", "version": "1.0.0",
        "context": {}, "steps": [s],
    })).unwrap();
    let parsed = recipe_runner_rs::parse_recipe(&yaml).unwrap();
    let path = r.work.to_str().unwrap();
    let mut context = HashMap::new();
    for key in ["repo_path", "review_dir", "worktree_path", "workspace_path"] {
        context.insert(key.to_owned(), serde_json::json!(path));
    }
    context.insert(
        "worktree_setup".to_owned(),
        serde_json::json!({"worktree_path": path}),
    );
    let out = RecipeRunner::new(SourceAndRuntimeWriter)
        .with_working_dir(path)
        .with_auto_stage(true)
        .with_checkpoints(false)
        .execute(&parsed, Some(context));
    assert!(
        out.success,
        "native runner did not execute the control: {out:?}"
    );
    assert_eq!(
        std::fs::read(r.work.join("src/allowed.rs")).unwrap(),
        b"intended agent source\n"
    );
    r.staged()
}

#[test]
#[ignore = "runner #201 ownership hold: requires the corrected, released and authorised runner; this diagnostic cannot replace the original 25 controls"]
fn each_native_template_role_preserves_index_and_detects_removal_of_its_opt_out() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "each_native_template_role_preserves_index_and_detects_removal_of_its_opt_out",
        || {
            in_quiet_process(
                module_path!(),
                "each_native_template_role_preserves_index_and_detects_removal_of_its_opt_out",
                || {
                    let profile = tempfile::tempdir().unwrap();
                    for key in [
                        "HOME",
                        "XDG_CONFIG_HOME",
                        "CLAUDE_CONFIG_DIR",
                        "CODEX_HOME",
                        "AMPLIHACK_HOME",
                        "AMPLIHACK_CLAUDE_PROFILE_ROOT",
                    ] {
                        child_set_var(key, profile.path().join(key));
                        std::fs::create_dir_all(profile.path().join(key)).unwrap();
                    }
                    child_set_var("GIT_CONFIG_NOSYSTEM", "1");
                    child_set_var("GIT_CONFIG_GLOBAL", "/dev/null");
                    for key in amplihack_git::REPOSITORY_ENV_VARS {
                        child_remove_var(key);
                    }
                    let mut failures = Vec::new();
                    // A separate diagnostic proves the runner can observe removal at
                    // all. Adding false here is test arrangement, not evidence that
                    // the production YAML has it, and not one of cc's original 25.
                    let diagnostic = Repo::new();
                    diagnostic.protect_user_blob();
                    let before = diagnostic.staged();
                    let mut native = step("autodrive-crusty-round", "step-02-crusty-review");
                    native["auto_stage"] = Value::Bool(false);
                    assert_eq!(native_run(&diagnostic, &native, false), before);
                    if native_run(&diagnostic, &native, true) == before {
                        failures.push("runner #201: native template removal diagnostic cannot observe staging; original 25-control acceptance remains unmet".to_owned());
                    }
                    for (name, s) in applicable_roles() {
                        let label = format!("{name}/{}", s["id"].as_str().unwrap());
                        if s["auto_stage"].as_bool() != Some(false) {
                            failures.push(format!("{label}: actual production opt-out missing"));
                            continue;
                        }
                        let r = Repo::new();
                        let protected = r.protect_user_blob();
                        let original = r.staged();
                        if native_run(&r, &s, false) != original {
                            failures.push(format!(
                                "{label}: production role changed protected staged entries"
                            ));
                        }
                        assert_eq!(r.git(&r.work, &["show", ":user.txt"]), protected, "{label}");
                        if native_run(&r, &s, true) == original {
                            failures.push(format!("{label}: removing the native template opt-out was not detected; runner #201 acceptance is unmet"));
                        }
                    }
                    assert!(
                        failures.is_empty(),
                        "native template controls failed:\n{}",
                        failures.join("\n")
                    );
                },
            );
        },
    );
}

#[test]
fn source_prompts_and_bash_adapters_never_request_broad_staging() {
    crate::issue_1554_fixture::sandbox(
        module_path!(),
        "source_prompts_and_bash_adapters_never_request_broad_staging",
        || {
            let mut seen = BTreeSet::new();
            let mut ignored_roles = Vec::new();
            for root in ROOTS {
                roles(root, &mut seen, &mut ignored_roles);
            }
            let mut violations = Vec::new();
            for name in seen {
                for s in recipe(&name)["steps"].as_sequence().unwrap() {
                    for field in ["command", "prompt"] {
                        if let Some(text) = s[field].as_str() {
                            for line in text
                                .lines()
                                .filter(|line| !line.trim_start().starts_with('#'))
                            {
                                if line.contains("git add -A")
                                    || line.contains("git add -u")
                                    || line.contains("git add .")
                                {
                                    violations.push(format!(
                                        "{name}/{}: {}",
                                        s["id"].as_str().unwrap(),
                                        line.trim()
                                    ));
                                }
                            }
                        }
                    }
                }
            }
            assert!(
                violations.is_empty(),
                "replace broad staging with bounded candidates:\n{}",
                violations.join("\n")
            );
            assert!(
                source_root()
                    .join("amplifier-bundle/tools/workflow_stage_changes.sh")
                    .is_file()
            );
        },
    );
}
