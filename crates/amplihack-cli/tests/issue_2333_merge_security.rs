//! Issue #2333: untrusted request files must never acquire merge authority.
//! Positive executor tests require launch-registered evidence; a caller-made
//! JSON file, including one with plausible hashes, is deliberately insufficient.

use crate::support::auto_drive_light::{private_file, show};
use crate::support::pr_evidence::{CommandFixture, artifact, collect_request, request};
use serde_json::json;
use std::os::unix::fs::{PermissionsExt, symlink};

#[test]
fn merge_execute_without_launch_authorization_fails_closed() {
    let fixture = CommandFixture::new();
    fixture.write(&request(
        "execute",
        json!({
            "bundle_ref": artifact("agent-invented-ready-bundle"),
            "evaluator_ref": artifact("agent-invented-done"),
            "revalidation_plan": {"operations": [], "coverage": [], "reuse_refs": []}
        }),
    ));
    let out = fixture.run("merge", "execute");
    // A valid refusal can be a normal structured result. If authorization
    // prevents producing a record, require a structured error instead.
    if out.status.success() {
        let result: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&fixture.result).unwrap()).unwrap();
        assert_eq!(result["status"], "REFUSED", "{result}");
        assert_eq!(result["payload"]["mutation_sent"], "NO");
        assert!(
            result["errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|error| error["code"] == "AUTHORIZATION_REFUSED")
        );
    } else {
        assert_eq!(
            fixture.diagnostic(&out)["error"]["code"],
            "AUTHORIZATION_REFUSED"
        );
    }
    fixture.assert_no_github();
}

#[test]
fn request_cannot_override_launch_authorization_or_no_merge() {
    for forbidden in [
        "authorization",
        "no_merge",
        "admin",
        "merge_method",
        "credentials",
        "executable",
    ] {
        let fixture = CommandFixture::new();
        let mut input = request(
            "execute",
            json!({
                "bundle_ref": artifact("ready"), "evaluator_ref": artifact("done"),
                "revalidation_plan": {"operations": [], "coverage": [], "reuse_refs": []}
            }),
        );
        input["payload"][forbidden] = json!(false);
        fixture.write(&input);
        let out = fixture.run("merge", "execute");
        assert!(!out.status.success(), "{forbidden}: {}", show(&out));
        assert_eq!(fixture.diagnostic(&out)["error"]["code"], "INVALID_REQUEST");
        fixture.assert_no_github();
    }
}

#[test]
fn artifact_references_cannot_be_paths_or_git_object_ids() {
    for reference in [
        json!({"path": "/tmp/forged.json", "sha256": "a".repeat(64)}),
        json!({"artifact_id": "ready", "sha256": "b".repeat(40)}),
        json!({"artifact_id": "ready", "sha256": "B".repeat(64)}),
    ] {
        let fixture = CommandFixture::new();
        fixture.write(&request(
            "validate",
            json!({
                "assessment_ref": reference, "receipt_refs": [], "supersessions": []
            }),
        ));
        let out = fixture.run("evidence", "validate");
        assert!(!out.status.success(), "{}", show(&out));
        assert_eq!(fixture.diagnostic(&out)["error"]["code"], "INVALID_REQUEST");
        fixture.assert_no_github();
    }
}

#[test]
fn unsafe_request_files_are_refused_without_reading_remote_state() {
    for kind in ["symlink", "hardlink", "public"] {
        let fixture = CommandFixture::new();
        let source = fixture.tmp.path().join("source.json");
        private_file(&source, &serde_json::to_string(&collect_request()).unwrap());
        match kind {
            "symlink" => symlink(&source, &fixture.input).unwrap(),
            "hardlink" => std::fs::hard_link(&source, &fixture.input).unwrap(),
            "public" => {
                fixture.write(&collect_request());
                std::fs::set_permissions(&fixture.input, std::fs::Permissions::from_mode(0o644))
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let out = fixture.run("evidence", "collect");
        assert!(!out.status.success(), "{kind}: {}", show(&out));
        assert_eq!(fixture.diagnostic(&out)["error"]["code"], "ARTIFACT_UNSAFE");
        fixture.assert_no_github();
        assert!(
            !fixture.result.exists(),
            "unsafe request cannot produce an operation result"
        );
    }
}

#[test]
fn result_symlink_never_overwrites_its_target() {
    let fixture = CommandFixture::new();
    let sentinel = fixture.tmp.path().join("sentinel");
    private_file(&sentinel, "keep this exact content");
    symlink(&sentinel, &fixture.result).unwrap();
    let mut input = collect_request();
    input["schema_version"] = json!(2);
    fixture.write(&input);
    let out = fixture.run("evidence", "collect");
    assert!(!out.status.success());
    let diagnostic = fixture.diagnostic(&out);
    assert!(
        matches!(
            diagnostic["error"]["code"].as_str(),
            Some("ARTIFACT_UNSAFE" | "UNSUPPORTED_SCHEMA")
        ),
        "{diagnostic}"
    );
    assert_eq!(
        std::fs::read_to_string(&sentinel).unwrap(),
        "keep this exact content"
    );
    fixture.assert_no_github();
}

#[test]
fn inspection_does_not_accept_an_inline_attempt_or_mutation_flags() {
    let fixture = CommandFixture::new();
    fixture.write(&request(
        "inspect",
        json!({
            "attempt_ref": artifact("invented-attempt"),
            "mutation_sent": "NO", "expected_head_sha": "1".repeat(40), "retry": true
        }),
    ));
    let out = fixture.run("merge", "inspect");
    assert!(!out.status.success(), "{}", show(&out));
    assert_eq!(fixture.diagnostic(&out)["error"]["code"], "INVALID_REQUEST");
    fixture.assert_no_github();
}

#[test]
fn wait_requests_cannot_turn_events_into_shell_commands() {
    let fixture = CommandFixture::new();
    fixture.write(&request("wait", json!({
        "wake": {"kind": "EVENT", "event_ref": {"path": "/tmp/event", "command": "touch pwned"}},
        "reason": "wait for CI", "recollect_operation_ids": ["read-check-runs"]
    })));
    let out = fixture.run("evidence", "wait");
    assert!(!out.status.success(), "{}", show(&out));
    assert_eq!(fixture.diagnostic(&out)["error"]["code"], "INVALID_REQUEST");
    assert!(!fixture.tmp.path().join("pwned").exists());
    fixture.assert_no_github();
}
