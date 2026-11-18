//! Regressions for publication output and malformed PR metadata boundaries.
#![cfg(unix)]

use super::workflow_finalize_publish_ready::fixture::{BRANCH, Fixture, executable, show, success};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Output;

const TOKEN: &str = "ghp_SECURITYPROBE01234567890123456789";
const PASSWORD: &str = "FAKEJQSECRET";
const SECRETS: &str = "ghp_SECURITYPROBE01234567890123456789 https://fake-user:fake-password@example.invalid/ password=FAKEJQSECRET";

fn fixture() -> Fixture {
    Fixture::new(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
        Path::new(env!("CARGO_BIN_EXE_amplihack")),
    )
}

fn assert_safe(text: &str) {
    for secret in [TOKEN, PASSWORD, "fake-user", "fake-password"] {
        assert!(!text.contains(secret), "credential in output: {text:?}");
    }
    for control in ['\u{1b}', '\r', '\u{7}', '\u{202e}'] {
        assert!(!text.contains(control), "terminal control in {text:?}");
    }
}

fn publication_output(commit: bool, fail: bool) {
    let f = fixture();
    f.audit(!commit);
    let before = f.remote_head();
    let hook = if commit {
        let hooks = f.git(
            &f.pr,
            &["rev-parse", "--path-format=absolute", "--git-path", "hooks"],
        );
        Path::new(hooks.trim()).join("pre-commit")
    } else {
        f.remote.join(if fail {
            "hooks/pre-receive"
        } else {
            "hooks/post-receive"
        })
    };
    executable(
        &hook,
        &format!(
            "#!/bin/sh\nprintf '%s\\033[31m\\r\\007\\n' '{SECRETS}' >&2\nexit {}\n",
            i32::from(fail)
        ),
    );
    let output = f.publish();
    assert_eq!(
        output.status.code(),
        Some(i32::from(fail)),
        "{}",
        show(&output)
    );
    assert_safe(&show(&output));
    assert!(show(&output).contains("REDACTED"), "{}", show(&output));
    if fail {
        assert_eq!(f.remote_head(), before);
    } else {
        assert_eq!(f.remote_head(), f.head(&f.pr));
    }
    // Raw captures are removed on successful and failed commands.
    assert_eq!(fs::read_dir(f.root.join("tmp")).unwrap().count(), 1);
    f.assert_no_mutation();
}

#[test]
fn successful_commit_hook_output_is_sanitized() {
    publication_output(true, false);
}
#[test]
fn failed_commit_hook_output_is_sanitized() {
    publication_output(true, true);
}
#[test]
fn successful_push_output_is_sanitized() {
    publication_output(false, false);
}
#[test]
fn failed_push_output_is_sanitized() {
    publication_output(false, true);
}

#[test]
fn sanitizer_failure_never_releases_partial_output_or_masks_command_failure() {
    for commit in [false, true] {
        for fail in [false, true] {
            let f = fixture();
            f.audit(!commit);
            let hook = if commit {
                let hooks = f.git(
                    &f.pr,
                    &["rev-parse", "--path-format=absolute", "--git-path", "hooks"],
                );
                Path::new(hooks.trim()).join("pre-commit")
            } else {
                f.remote.join("hooks/pre-receive")
            };
            executable(&hook, &format!("#!/bin/sh\nexit {}\n", i32::from(fail)));
            executable(
                &f.root.join("bin/amplihack"),
                &format!(
                    "#!/bin/bash\nif [ \"$*\" = 'orch helper sanitize-output' ]; then\n printf '%s' '{SECRETS}'\n exit 71\nfi\nexec \"$REAL_AMPLIHACK\" \"$@\"\n"
                ),
            );
            let output = f.publish();
            assert_eq!(output.status.code(), Some(1), "{}", show(&output));
            assert_safe(&show(&output));
            assert!(show(&output).contains("sanitization unavailable"));
            assert_eq!(fs::read_dir(f.root.join("tmp")).unwrap().count(), 1);
            f.assert_no_mutation();
        }
    }
}

fn scope(f: &Fixture, explicit: bool) -> Output {
    let mut command = f.tool("workflow_pr_scope.sh", &f.main);
    command.args([
        "--repo",
        "acme/widget",
        "--head",
        BRANCH,
        "--base",
        "main",
        "--issue",
        "2787",
        "--any-base-open",
    ]);
    if explicit {
        command.args(["--pr-number", "7"]);
    }
    command.output().unwrap()
}

fn raw_metadata(f: &Fixture, data: &str) {
    fs::write(f.root.join("metadata"), data).unwrap();
    executable(
        &f.root.join("bin/gh"),
        "#!/bin/sh\ncat \"$FIXTURE/metadata\"\n",
    );
}

fn protected_report(f: &Fixture, output: &Output, reason: &str) -> Value {
    assert!(!output.status.success(), "{}", show(output));
    assert_safe(&show(output));
    let result: Value = serde_json::from_slice(&output.stdout).expect("structured failure");
    assert_eq!(result["reason"], reason, "{}", show(output));
    let path = Path::new(result["diagnostic_report"].as_str().unwrap());
    assert!(path.starts_with(f.root.join("runtime")));
    let report = fs::read_to_string(path).unwrap();
    assert_safe(&report);
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert!(show(output).contains(path.to_str().unwrap()));
    assert_eq!(fs::read_dir(f.root.join("tmp")).unwrap().count(), 1);
    f.assert_no_mutation();
    serde_json::from_str(&report).unwrap()
}

#[test]
fn unrelated_head_with_array_body_produces_protected_exclusion() {
    let f = fixture();
    f.patch_candidate(r#".headRefName="unrelated" | .body=["password=FAKEJQSECRET"]"#);
    let report = protected_report(&f, &scope(&f, true), "no_scoped_pr");
    assert_eq!(report["candidates"][0]["number"], 7);
    assert_eq!(
        report["candidates"][0]["exclusions"][0]["check"],
        "malformed candidate: body"
    );
    assert_eq!(
        report["candidates"][0]["exclusions"][0]["observed"],
        "array"
    );
}

#[test]
fn malformed_fresh_response_cannot_skip_readiness_or_its_protected_report() {
    for filter in [
        ".mergedAt=[]",
        r#".body=["password=FAKEJQSECRET"]"#,
        r#".isCrossRepository={"password=FAKEJQSECRET":true}"#,
    ] {
        let f = fixture();
        f.audit(true);
        success(f.publish());
        fs::write(f.root.join("final-filter.jq"), filter).unwrap();
        let output = f.ready();
        assert!(!output.status.success(), "{}", show(&output));
        assert_safe(&show(&output));
        let reports: Vec<_> = fs::read_dir(f.root.join("runtime")).unwrap().collect();
        assert_eq!(reports.len(), 1, "{}", show(&output));
        let path = reports[0].as_ref().unwrap().path().join("exclusions.json");
        let report = fs::read_to_string(&path).unwrap();
        assert_safe(&report);
        assert!(report.contains("malformed candidate:"));
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        f.assert_no_mutation();
    }
}

#[test]
fn wrong_type_candidates_are_excluded_before_all_selection_paths() {
    let f = fixture();
    let valid: Value =
        serde_json::from_str(&fs::read_to_string(f.root.join("candidate.json")).unwrap()).unwrap();
    let mut candidates = vec![Value::Null, json!([SECRETS]), json!(SECRETS), json!(3)];
    for field in [
        "number",
        "url",
        "state",
        "isDraft",
        "isCrossRepository",
        "title",
        "body",
        "createdAt",
        "mergedAt",
        "headRefName",
        "baseRefName",
        "headRefOid",
        "headRepository",
        "headRepositoryOwner",
    ] {
        let mut candidate = valid.clone();
        candidate[field] = json!([SECRETS]);
        candidates.push(candidate);
    }
    for (parent, field) in [
        ("headRepository", "name"),
        ("headRepository", "nameWithOwner"),
        ("headRepositoryOwner", "login"),
        ("headRepositoryOwner", "name"),
    ] {
        let mut candidate = valid.clone();
        candidate[parent][field] = json!({"password": PASSWORD});
        candidates.push(candidate);
    }
    let mut bad_number = valid.clone();
    bad_number["number"] = json!(-1);
    candidates.push(bad_number);
    raw_metadata(&f, &serde_json::to_string(&candidates).unwrap());
    let report = protected_report(&f, &scope(&f, false), "no_scoped_pr");
    let exclusions = report["candidates"].as_array().unwrap();
    assert_eq!(exclusions.len(), candidates.len());
    for candidate in exclusions {
        assert!(candidate["exclusions"].as_array().unwrap().iter().any(|e| {
            e["check"]
                .as_str()
                .unwrap()
                .starts_with("malformed candidate:")
        }));
    }
    // A valid observation remains selectable, while malformed observations
    // still get durable diagnostics rather than being silently discarded.
    candidates.push(valid);
    raw_metadata(&f, &serde_json::to_string(&candidates).unwrap());
    let output = scope(&f, false);
    let result: Value = serde_json::from_str(&success(output)).unwrap();
    assert_eq!(result["number"], 7);
    let reports: Vec<_> = fs::read_dir(f.root.join("runtime")).unwrap().collect();
    assert_eq!(reports.len(), 2);
    for dir in reports {
        assert_safe(&fs::read_to_string(dir.unwrap().path().join("exclusions.json")).unwrap());
    }
}

#[test]
fn invalid_documents_and_parser_errors_have_protected_evidence() {
    for explicit in [false, true] {
        for data in ["{ invalid-json", "{}\n{}", "42"] {
            let f = fixture();
            raw_metadata(&f, data);
            let reason = if explicit && data == "42" {
                "no_scoped_pr"
            } else {
                "invalid_pr_metadata"
            };
            protected_report(&f, &scope(&f, explicit), reason);
        }
        // Simulate a parser diagnostic that quotes sensitive input, including
        // a failure after parsing but before candidate selection.
        let f = fixture();
        executable(
            &f.root.join("bin/jq"),
            &format!(
                r#"#!/bin/bash
case "$*" in
  *'include "workflow_pr_scope";'*)
    printf '%s\033[31m\r\007\n' '{SECRETS}' >&2
    exit 5 ;;
esac
exec /usr/bin/jq "$@"
"#
            ),
        );
        let report = protected_report(&f, &scope(&f, explicit), "invalid_pr_metadata");
        assert!(
            report["parser_error"]
                .as_str()
                .unwrap()
                .contains("REDACTED")
        );
    }
}
