use super::*;

fn output_step<'a>(recipe: &'a serde_yaml::Value, name: &str) -> &'a serde_yaml::Value {
    steps(recipe)
        .iter()
        .find(|s| s["output"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("missing preserved output {name}"))
}

#[test]
fn base_and_ci_preparation_are_guarded_agent_decisions() {
    let recipe = recipe_yaml("autodrive-merge-evidence");
    for output in ["merge_sync", "ci_evidence"] {
        let s = output_step(&recipe, output);
        assert!(
            s.get("agent").is_some(),
            "{output} must be agent judgement, not a coded policy sequence"
        );
        assert_eq!(
            s["working_dir"].as_str(),
            Some("{{review_dir}}"),
            "{output} must start in the guarded review directory"
        );
        let prompt = field(s, "prompt");
        assert!(
            prompt.contains("autodrive_launch_tools"),
            "{output} must use launch-side helpers"
        );
        assert!(
            prompt.contains(super::super::STATE_DIR_SENTENCE)
                || super::super::squash(prompt).contains(super::super::STATE_DIR_SENTENCE),
            "{output} must preserve the state-directory prohibition"
        );
    }
}

#[test]
fn production_dependencies_are_sync_then_ci_then_qa() {
    let recipe = recipe_yaml("autodrive-merge-evidence");
    let position = |output| {
        steps(&recipe)
            .iter()
            .position(|s| s["output"].as_str() == Some(output))
            .expect("preserved output")
    };
    assert!(position("merge_sync") < position("ci_evidence"));
    assert!(
        position("ci_evidence") < position("qa_evidence"),
        "QA currently starts before the in-progress CI run is awaited"
    );
    let qa = output_step(&recipe, "qa_evidence");
    let consumed = serde_yaml::to_string(qa).unwrap();
    assert!(
        consumed.contains("ci_evidence"),
        "QA must consume validated CI, not just run later"
    );
}

#[test]
fn parent_forwards_guarded_context_and_empty_outputs_prevent_inherited_evidence() {
    let parent = recipe_yaml("autodrive-merge-round");
    let child = step(&parent, "merge-evidence");
    let recipe = recipe_yaml("autodrive-merge-evidence");
    for key in ["review_dir", "autodrive_launch_tools"] {
        assert_eq!(
            child["context"][key].as_str(),
            Some(format!("{{{{{key}}}}}").as_str()),
            "the nested judging agents need the parent's guarded {key}"
        );
        assert_eq!(recipe["context"][key].as_str(), Some(""));
    }
    for output in ["merge_sync", "ci_evidence", "qa_evidence"] {
        assert_eq!(
            recipe["context"][output].as_str(),
            Some(""),
            "nested preparation must not inherit old {output}"
        );
    }
}

#[test]
fn refresh_only_does_not_dispatch_the_corrective_builder() {
    let recipe = recipe_yaml("autodrive-merge-round");
    let builder = step(&recipe, "step-04-address-blockers");
    let condition = field(builder, "condition");
    assert!(
        condition.contains("refresh_required") && condition.contains("false"),
        "refresh-only NOT_MERGE_READY must skip the corrective builder: {condition}"
    );
}

#[test]
fn no_merge_defaults_true_and_reaches_the_final_gate() {
    let composer = recipe_yaml("auto-drive-to-merge");
    assert_eq!(composer["context"]["no_merge"].as_str(), Some("true"));
    let phase = steps(&composer)
        .iter()
        .find(|s| s["recipe"].as_str() == Some("autodrive-merge-loop"))
        .unwrap();
    assert_eq!(phase["context"]["no_merge"].as_str(), Some("{{no_merge}}"));
    let merge_loop = recipe_yaml("autodrive-merge-loop");
    assert_eq!(merge_loop["context"]["no_merge"].as_str(), Some("true"));
    let gate = field(step(&merge_loop, "step-03-merge-gate"), "command");
    assert!(
        gate.contains("--no-merge"),
        "build-phase suppression does not protect the final gate"
    );
}

/// Real measured-downgrade tool, real JSON extraction, real evidence hash.
/// Faked platform data only. This catches a forged agent MERGE_READY even
/// when its stale evidence has an internally consistent file hash.
fn measured(input: &Value) -> String {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let path = bin_path(&bin);
    executable(&bin.join("gh"), "#!/bin/sh\nprintf '%s\\n' \"$PR_JSON\"\n");
    executable(
        &bin.join("git"),
        "#!/bin/sh\ncase \"$*\" in *hash-object*) exec \"$REAL_GIT\" \"$@\" ;; *) printf '%s\\n' \"$LOCAL_HEAD\" ;; esac\n",
    );
    let real_git = Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    assert!(real_git.status.success());
    let real_git = String::from_utf8(real_git.stdout)
        .unwrap()
        .trim()
        .to_string();
    let qa = tmp.path().join("qa.json");
    private_file(&qa, &input["qa_evidence"].to_string());
    let hash = shell(
        "bash \"$ROUND_TOOL\" qa-evidence-sha \"$QA_FILE\"",
        tmp.path(),
        &[
            (
                "ROUND_TOOL",
                tool_path("autodrive_round_evidence.sh")
                    .display()
                    .to_string(),
            ),
            ("QA_FILE", qa.display().to_string()),
        ],
    );
    assert!(hash.status.success(), "{}", diagnostic(&hash));
    let hash = String::from_utf8(hash.stdout).unwrap().trim().to_owned();
    assert!(
        hash.len() == 40 || hash.len() == 64,
        "control evidence hash missing"
    );
    let current = &input["current"];
    let out = shell(
        "bash \"$ROUND_TOOL\" measured-downgrade",
        tmp.path(),
        &[
            ("PATH", path),
            (
                "ROUND_TOOL",
                tool_path("autodrive_round_evidence.sh")
                    .display()
                    .to_string(),
            ),
            ("REPO_PATH", tmp.path().display().to_string()),
            ("PR_NUMBER", "1240".into()),
            ("LOCAL_HEAD", current["head_sha"].as_str().unwrap().into()),
            ("REAL_GIT", real_git),
            (
                "PR_JSON",
                json!({"headRefOid":current["head_sha"], "number":1240,
            "baseRefOid":current["base_sha"], "state":"OPEN"})
                .to_string(),
            ),
            ("AUTODRIVE_QA_EVIDENCE", qa.display().to_string()),
            ("RECIPE_VAR_merge_sync", input["merge_sync"].to_string()),
            ("RECIPE_VAR_ci_evidence", input["ci_evidence"].to_string()),
            ("RECIPE_VAR_qa_evidence", input["qa_evidence"].to_string()),
            (
                "RECIPE_VAR_qa_evidence_hash",
                json!({"qa_evidence_sha":hash}).to_string(),
            ),
            (
                "RECIPE_VAR_platform_facts",
                json!({"unresolved_threads":"0", "approval_status":"MET",
            "identity":current["identity"], "head_sha":current["head_sha"],
            "base_sha":current["base_sha"], "policy_revision":current["policy_revision"],
            "ci_attempts":current["ci_attempts"]})
                .to_string(),
            ),
            (
                "RECIPE_VAR_crusty_evidence",
                json!({"crusty_status":"DONE_CLEAN"}).to_string(),
            ),
            (
                "RECIPE_VAR_merge_ready_review",
                input["assessment"].to_string(),
            ),
        ],
    );
    assert!(out.status.success(), "{}", diagnostic(&out));
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn acceptance_rejects_mismatched_heads_even_when_all_statuses_and_hashes_pass() {
    assert_eq!(
        measured(&snapshot()),
        "none",
        "fresh control must be accepted"
    );
    for pointer in [
        "/qa_evidence/head_sha",
        "/ci_evidence/head_sha",
        "/assessment/assessed_head_sha",
        "/current/head_sha",
    ] {
        let mut input = snapshot();
        *input.pointer_mut(pointer).unwrap() = fixture()["heads"]["replacement"].clone();
        assert_ne!(
            measured(&input),
            "none",
            "stale {pointer} escaped measured downgrade"
        );
    }
}

#[test]
fn acceptance_rejects_unknown_policy_and_unpublished_local_sync() {
    assert_eq!(measured(&snapshot()), "none");
    let mut unknown = snapshot();
    unknown["merge_sync"]["policy_status"] = json!("UNKNOWN");
    unknown["merge_sync"]["policy_sources"] = json!([]);
    assert_ne!(
        measured(&unknown),
        "none",
        "unreadable policy cannot establish readiness"
    );

    let mut failed_push = snapshot();
    failed_push["merge_sync"]["synced"] = json!("true");
    failed_push["merge_sync"]["head_sha"] = fixture()["heads"]["replacement"].clone();
    assert_ne!(
        measured(&failed_push),
        "none",
        "a local merge is not a published PR head"
    );
}

#[test]
fn acceptance_reobserves_same_head_attempts_and_policy_changes() {
    assert_eq!(measured(&snapshot()), "none");
    for (pointer, value) in [
        ("/current/ci_attempts/0/attempt", json!(2)),
        ("/current/policy_revision", json!("ruleset-42-v2")),
    ] {
        let mut input = snapshot();
        *input.pointer_mut(pointer).unwrap() = value;
        assert_ne!(
            measured(&input),
            "none",
            "head equality alone accepted stale {pointer}"
        );
    }
}

#[test]
fn completed_failure_unreadable_and_absent_checks_remain_distinct_blockers() {
    assert_eq!(measured(&snapshot()), "none");
    for (status, observation) in [
        ("RED", "failed"),
        ("UNREADABLE", "unreadable"),
        ("NO_CHECKS", "no_checks"),
    ] {
        let mut input = snapshot();
        input["ci_evidence"]["ci_status"] = json!(status);
        input["ci_evidence"]["observation"] = fixture()[observation].clone();
        assert!(
            measured(&input).contains(&format!("ci_status={status}")),
            "{status} was hidden or treated as successful absence"
        );
    }
}

#[test]
fn qa_does_not_execute_repository_code_before_completed_valid_green_ci() {
    for (status, observation) in [
        ("", "pending"),
        ("RED", "failed"),
        ("UNREADABLE", "unreadable"),
        ("NO_CHECKS", "no_checks"),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(
            repo.join("Cargo.toml"),
            "[package]\nname='qa-fixture'\nversion='0.1.0'\nedition='2024'\n",
        )
        .unwrap();
        fs::write(repo.join("src/lib.rs"), "pub fn answer() -> u8 { 42 }\n").unwrap();
        let init = shell(
            "git init -q && git add Cargo.toml src/lib.rs && git -c user.name=Fixture -c user.email=fixture@example.invalid -c core.hooksPath=/dev/null commit -qm fixture && git rev-parse HEAD",
            &repo,
            &[],
        );
        assert!(init.status.success(), "{}", diagnostic(&init));
        let head = String::from_utf8(init.stdout).unwrap().trim().to_string();
        let bin = tmp.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let path = bin_path(&bin);
        let mut ci = snapshot()["ci_evidence"].clone();
        ci["ci_status"] = json!(status);
        ci["observation"] = fixture()[observation].clone();
        let ci = ci
            .to_string()
            .replace(fixture()["heads"]["source"].as_str().unwrap(), &head);
        let out = shell(
            "bash \"$QA_TOOL\"",
            &repo,
            &[
                ("PATH", path),
                ("REPO_PATH", repo.display().to_string()),
                (
                    "QA_TOOL",
                    tool_path("autodrive_qa_evidence.sh").display().to_string(),
                ),
                (
                    "AUTODRIVE_QA_COMMANDS",
                    "printf ran > repository-code-ran".into(),
                ),
                ("RECIPE_VAR_ci_evidence", ci),
            ],
        );
        assert!(
            !repo.join("repository-code-ran").exists(),
            "repository suite executed while CI was {observation}: {}",
            diagnostic(&out)
        );
        // A named refusal or structured BLOCKED are both valid entry failures.
        if out.status.success() {
            let evidence: Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_ne!(evidence["qa_status"], "PASS");
        }
    }
}

#[test]
fn final_gate_reports_merged_pr_as_open_delivery_violation_under_no_merge() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let path = bin_path(&bin);
    executable(
        &bin.join("gh"),
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$CALLS"
case "$1 $2" in
  'pr view') printf '%s\n' '{"state":"MERGED","mergedAt":"2026-11-11T10:05:00Z"}' ;;
  *) exit 98 ;;
esac
"#,
    );
    let out = shell(
        "/bin/bash -p \"$GATE\" --pr 1240 --repo \"$REPO\" --state-dir \"$STATE\" --no-merge true",
        tmp.path(),
        &[
            ("PATH", path),
            (
                "GATE",
                tool_path("autodrive_merge_gate.sh").display().to_string(),
            ),
            ("REPO", tmp.path().display().to_string()),
            ("STATE", tmp.path().join("state").display().to_string()),
            ("CALLS", tmp.path().join("gh-calls").display().to_string()),
        ],
    );
    assert_eq!(out.status.code(), Some(1), "{}", diagnostic(&out));
    let result: Value =
        serde_json::from_slice(&out.stdout).expect("gate must report structured delivery failure");
    assert_eq!(result["merge_result"], "ALREADY_MERGED");
    assert_eq!(result["delivery_violation"], true);
    let calls = read(&tmp.path().join("gh-calls"));
    assert!(
        !calls
            .lines()
            .any(|line| line.contains("pr merge") || line.contains("--method PUT"))
    );
}
