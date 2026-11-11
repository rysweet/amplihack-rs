use super::*;

#[test]
fn valid_noop_sync_does_not_require_synced_true() {
    // A clean behind branch is intentionally untouched. Mechanical validation
    // must not reinterpret synced=false as a policy failure.
    valid_snapshot();
}

#[test]
fn published_conflict_resolution_and_required_updates_remain_valid_syncs() {
    valid_snapshot();
    for decision in ["conflict-resolved", "required-update"] {
        let mut input = snapshot();
        input["merge_sync"]["remote_head_before"] = fixture()["heads"]["replacement"].clone();
        input["merge_sync"]["synced"] = json!("true");
        input["merge_sync"]["decision"] = json!(decision);
        input["merge_sync"]["update_required"] = json!(if decision == "required-update" {
            "true"
        } else {
            "false"
        });
        let out = helper(&input.to_string());
        assert!(
            out.status.success(),
            "published {decision} rejected: {}",
            diagnostic(&out)
        );
        let result: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(result["valid"], true);
        assert_eq!(result["refresh_required"], false);
    }
}

#[test]
fn accepts_complete_sha256_object_ids_as_well_as_sha1() {
    valid_snapshot();
    let mut raw = snapshot().to_string();
    for digit in ['1', '3', '5'] {
        raw = raw.replace(&digit.to_string().repeat(40), &digit.to_string().repeat(64));
    }
    let out = helper(&raw);
    assert!(
        out.status.success(),
        "supported SHA-256 objects rejected: {}",
        diagnostic(&out)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["valid"], true);
}

#[test]
fn rejects_malformed_identifiers_statuses_counts_and_timestamps() {
    valid_snapshot(); // An unknown subcommand must not make negative tests pass.
    for (pointer, bad) in [
        (
            "/current/identity/host",
            json!("github.com; touch injected"),
        ),
        ("/current/identity/repository", json!("acme/../widgets")),
        ("/current/identity/pr", json!(0)),
        ("/current/identity/pr", json!(-1)),
        ("/current/head_sha", json!("1111111")),
        ("/current/head_sha", json!("g".repeat(40))),
        ("/ci_evidence/ci_status", json!("MOSTLY_GREEN")),
        ("/ci_evidence/ci_total", json!("-1")),
        ("/ci_evidence/ci_total", json!(1)), // preserve the string interface
        ("/ci_evidence/ci_pending", json!("")),
        ("/ci_evidence/observation/checks/0/id", json!(0)),
        ("/ci_evidence/observation/checks/0/run_id", json!(-1)),
        ("/ci_evidence/observation/checks/0/attempt", json!(0)),
        (
            "/ci_evidence/observation/checks/0/observed_at",
            json!("yesterday"),
        ),
        (
            "/ci_evidence/observation/checks/0/completed_at",
            json!("2026-13-40T99:00:00Z"),
        ),
    ] {
        let mut input = snapshot();
        *input.pointer_mut(pointer).unwrap() = bad;
        let out = helper(&input.to_string());
        assert!(
            !out.status.success(),
            "accepted malformed {pointer}: {}",
            diagnostic(&out)
        );
        let result: Value =
            serde_json::from_slice(&out.stdout).expect("validation errors are JSON");
        assert_eq!(result["valid"], false, "{pointer}: {}", diagnostic(&out));
    }
}

#[test]
fn rejects_duplicate_security_fields_before_json_values_can_overwrite_them() {
    valid_snapshot();
    for (needle, replacement) in [
        ("\"pr\":1240", "\"pr\":99,\"pr\":1240"),
        (
            "\"ci_status\":\"GREEN\"",
            "\"ci_status\":\"RED\",\"ci_status\":\"GREEN\"",
        ),
        (
            "\"ci_pending\":\"0\"",
            "\"ci_pending\":\"1\",\"ci_pending\":\"0\"",
        ),
    ] {
        let raw = snapshot().to_string();
        assert!(
            raw.contains(needle),
            "duplicate fixture did not mutate input"
        );
        let out = helper(&raw.replacen(needle, replacement, 1));
        assert!(
            !out.status.success(),
            "duplicate field silently overwritten: {}",
            diagnostic(&out)
        );
        let result: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(result["valid"], false);
    }
}

#[test]
fn forged_green_cannot_hide_pending_failures_partial_pages_or_foreign_producers() {
    valid_snapshot();
    for (pointer, bad) in [
        ("/ci_evidence/ci_pending", json!("1")),
        ("/ci_evidence/ci_failing", json!("1")),
        ("/ci_evidence/ci_total", json!("2")),
        ("/ci_evidence/observation", fixture()["pending"].clone()),
        ("/ci_evidence/observation", fixture()["failed"].clone()),
        ("/ci_evidence/observation", fixture()["unreadable"].clone()),
        ("/ci_evidence/observation", fixture()["no_checks"].clone()),
        ("/ci_evidence/observation/complete", json!(false)),
        ("/ci_evidence/observation/pages_expected", json!(2)),
        ("/ci_evidence/observation/checks/0/producer", json!(null)),
        (
            "/ci_evidence/observation/checks/0/producer/app_id",
            json!(999),
        ),
        (
            "/ci_evidence/observation/checks/0/repository",
            json!("attacker/widgets"),
        ),
        ("/ci_evidence/observation/checks/0/pr", json!(99)),
        (
            "/ci_evidence/observation/checks/0/conclusion",
            json!("cancelled"),
        ),
    ] {
        let mut input = snapshot();
        *input.pointer_mut(pointer).unwrap() = bad;
        let out = helper(&input.to_string());
        assert!(
            !out.status.success(),
            "GREEN contradicted raw observation at {pointer}: {}",
            diagnostic(&out)
        );
        let result: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(result["valid"], false);
    }
}

#[test]
fn equal_head_shas_do_not_authorise_evidence_for_another_pr_or_repository() {
    valid_snapshot();
    for record in ["merge_sync", "ci_evidence", "qa_evidence", "assessment"] {
        for (key, bad) in [
            ("repository", json!("foreign/widgets")),
            ("pr", json!(1239)),
            ("host", json!("github.example.org")),
        ] {
            let mut input = snapshot();
            input[record]["identity"][key] = bad;
            let out = helper(&input.to_string());
            assert!(
                !out.status.success(),
                "foreign {record}.{key} accepted: {}",
                diagnostic(&out)
            );
        }
    }
}

#[test]
fn stale_heads_attempts_base_and_policy_request_refresh_without_becoming_ready() {
    valid_snapshot();
    for (pointer, changed) in [
        (
            "/current/head_sha",
            fixture()["heads"]["replacement"].clone(),
        ),
        (
            "/ci_evidence/head_sha",
            fixture()["heads"]["replacement"].clone(),
        ),
        (
            "/qa_evidence/head_sha",
            fixture()["heads"]["replacement"].clone(),
        ),
        (
            "/assessment/assessed_head_sha",
            fixture()["heads"]["replacement"].clone(),
        ),
        ("/current/ci_attempts/0/attempt", json!(2)),
        (
            "/current/base_sha",
            fixture()["heads"]["replacement"].clone(),
        ),
        ("/current/policy_revision", json!("ruleset-42-v2")),
        (
            "/current/tree_sha",
            fixture()["heads"]["replacement"].clone(),
        ),
    ] {
        let mut input = snapshot();
        *input.pointer_mut(pointer).unwrap() = changed;
        let out = helper(&input.to_string());
        // Valid shape, stale evidence: this is a refresh result, not a
        // malformed record or a request to fix the implementation.
        assert!(
            out.status.success(),
            "staleness must be a structured refresh: {}",
            diagnostic(&out)
        );
        let result: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(
            result["valid"], false,
            "{pointer}: stale readiness accepted"
        );
        assert_eq!(
            result["refresh_required"], true,
            "{pointer}: refresh not signalled"
        );
        assert!(!result["reason"].as_str().unwrap_or_default().is_empty());
    }
}

#[test]
fn tested_content_must_be_clean_before_and_after_qa() {
    valid_snapshot();
    for key in ["clean_before", "clean_after"] {
        let mut input = snapshot();
        input["qa_evidence"][key] = json!("false");
        let out = helper(&input.to_string());
        assert!(
            !out.status.success(),
            "dirty tested content accepted: {}",
            diagnostic(&out)
        );
    }
}

#[test]
fn synthetic_merge_requires_association_and_pull_request_target_is_not_source_ci() {
    valid_snapshot();
    let mut synthetic = snapshot();
    let check = &mut synthetic["ci_evidence"]["observation"]["checks"][0];
    check["head_sha"] = fixture()["heads"]["synthetic_merge"].clone();
    check["association"] = json!({
        "kind":"synthetic_merge", "source_head_sha":fixture()["heads"]["source"],
        "base_sha":fixture()["heads"]["base"],
        "merge_sha":fixture()["heads"]["synthetic_merge"], "source":"github-rest:pulls/1240"
    });
    let out = helper(&synthetic.to_string());
    assert!(
        out.status.success(),
        "proven synthetic association rejected: {}",
        diagnostic(&out)
    );
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["valid"], true);

    synthetic["ci_evidence"]["observation"]["checks"][0]
        .as_object_mut()
        .unwrap()
        .remove("association");
    let out = helper(&synthetic.to_string());
    assert!(
        !out.status.success(),
        "unassociated synthetic run accepted: {}",
        diagnostic(&out)
    );

    let mut target = snapshot();
    target["ci_evidence"]["observation"]["checks"][0]["event"] = json!("pull_request_target");
    target["ci_evidence"]["observation"]["checks"][0]["head_sha"] =
        fixture()["heads"]["base"].clone();
    let out = helper(&target.to_string());
    assert!(
        !out.status.success(),
        "base-only pull_request_target accepted as source CI: {}",
        diagnostic(&out)
    );
}

#[test]
fn private_input_is_accepted_but_symlinks_and_public_records_are_rejected() {
    valid_snapshot();
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("snapshot.json");
    private_file(&file, &snapshot().to_string());
    let run = |path: &Path| {
        crate::isolated_amplihack::isolated(env!("CARGO_BIN_EXE_amplihack"))
            .args([
                "orch",
                "helper",
                "autodrive-evidence",
                "validate",
                "--input",
            ])
            .arg(path)
            .output()
            .unwrap()
    };
    let out = run(&file);
    assert!(
        out.status.success(),
        "private input rejected: {}",
        diagnostic(&out)
    );
    let alias = tmp.path().join("alias.json");
    std::os::unix::fs::symlink(&file, &alias).unwrap();
    let out = run(&alias);
    assert!(
        !out.status.success(),
        "symlink input accepted: {}",
        diagnostic(&out)
    );
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
    let out = run(&file);
    assert!(
        !out.status.success(),
        "public input accepted: {}",
        diagnostic(&out)
    );
}
