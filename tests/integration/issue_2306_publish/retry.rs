//! #2306 retry component and publisher error contracts; no transient waits.
use super::fixture::{PublishFixture, describe, executable, json, root};
use std::fs;
use std::process::Output;

fn retry(f: &PublishFixture, setup: &str) -> Output {
    executable(
        &f.bin.join("gh"),
        r#"#!/bin/bash
printf 'called\n' >> "$CASE_ROOT/retry-gh.log"
for file in "$GH_RETRY_TMPDIR"/* "$GH_RETRY_TMPDIR"/.[!.]*; do
  [ -f "$file" ] || continue
  ls -l "$file" >> "$CASE_ROOT/capture.permissions"
done
cat "$CASE_ROOT/create.stderr" >&2
exit 23
"#,
    );
    f.command("bash")
        .arg("-c")
        .arg(format!(
            r#"set -euo pipefail
. "$1/amplifier-bundle/tools/workflow_gh_retry.sh"
{setup}
_gh_retry_core 'pr create' pr create
"#
        ))
        .arg("retry-test")
        .arg(root())
        .output()
        .unwrap()
}

fn assert_diagnostic(text: &str, message: &str) {
    assert!(
        text.contains("gh pr create"),
        "failed operation missing: {text:?}"
    );
    assert!(
        text.contains("23"),
        "original exit status missing: {text:?}"
    );
    assert!(
        text.contains(message),
        "original diagnostic missing: {text:?}"
    );
}

#[test]
fn permanent_retry_failure_retains_command_status_and_provider_stderr() {
    let f = PublishFixture::new();
    f.write("create.stderr", "distinctive publish rejection");
    let output = retry(&f, "");
    assert_eq!(output.status.code(), Some(23));
    assert!(
        output.stdout.is_empty(),
        "failed gh stdout must not contaminate structured output"
    );
    assert_diagnostic(
        &String::from_utf8_lossy(&output.stderr),
        "distinctive publish rejection",
    );
    assert_eq!(
        fs::read_dir(f.tmp.path().join("private")).unwrap().count(),
        0
    );
}

#[test]
fn silent_retry_failure_still_identifies_operation_status_and_empty_stderr() {
    let f = PublishFixture::new();
    let output = retry(&f, "");
    assert_eq!(output.status.code(), Some(23));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_diagnostic(&stderr, "");
    assert!(
        stderr.contains("stderr") && (stderr.contains("empty") || stderr.contains("no error")),
        "must explain absence of provider stderr: {stderr:?}"
    );
}

#[test]
fn retry_diagnostics_redact_all_publishing_credential_formats_and_controls() {
    for secret in [
        "ghp_ABCDEFGHIJKLMNOPQRSTUVWX",
        "github_pat_ABCDEFGHIJKLMNOP_1234",
        "sk-ant-api03-ABCDEFGHIJKLMNOPQRST",
        "sk-proj-ABCDEFGHIJKLMNOPQRSTUV",
        "sk-ABCDEFGHIJKLMNOPQRSTUVWX",
        "AKIAABCDEFGHIJKLMNOP",
        "ASIAABCDEFGHIJKLMNOP",
        "AKIAABCDEFGHIJKLMNOP ASIAABCDEFGHIJKLMNOP",
        "https://user:pass@github.com/private",
    ] {
        let f = PublishFixture::new();
        f.write(
            "create.stderr",
            &format!("distinctive publish rejection \u{1b}[31m {secret}\u{7}\r\n"),
        );
        let output = retry(&f, "");
        assert_eq!(output.status.code(), Some(23));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_diagnostic(&stderr, "distinctive publish rejection");
        assert!(
            !stderr.contains(secret),
            "credential leaked in diagnostic: {stderr}"
        );
        for identifier in ["AKIAABCDEFGHIJKLMNOP", "ASIAABCDEFGHIJKLMNOP"] {
            assert!(
                !stderr.contains(identifier),
                "AWS identifier leaked: {stderr}"
            );
        }
        assert!(
            !stderr.chars().any(|c| c.is_control() && c != '\n'),
            "terminal control leaked: {stderr:?}"
        );
    }
}

#[test]
fn sanitizer_failure_discards_partial_output_and_preserves_gh_status() {
    let f = PublishFixture::new();
    f.write("create.stderr", "distinctive publish rejection");
    let output = retry(
        &f,
        r#"
gh_sanitize_stderr() {
  printf 'ghp_PARTIAL_SECRET_ABCDEFGHIJKLMNOP'
  return 76
}
"#,
    );
    assert_eq!(
        output.status.code(),
        Some(23),
        "formatter must not replace gh status"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_diagnostic(&stderr, "");
    assert!(
        !stderr.contains("PARTIAL_SECRET"),
        "partial sanitizer output leaked"
    );
    assert!(
        stderr.contains("sanitiz") || stderr.contains("unavailable"),
        "safe formatter failure explanation missing: {stderr}"
    );
}

#[test]
fn retry_redacts_before_truncation_including_a_token_crossing_the_limit() {
    let f = PublishFixture::new();
    let secret = "ghp_ABCDEFGHIJKLMNOPQRSTUVWX";
    f.write(
        "create.stderr",
        &format!("{}{} end", "x".repeat(490), secret),
    );
    let output = f
        .command("bash")
        .args([
            "-c",
            r#"
set -euo pipefail
. "$1/amplifier-bundle/tools/workflow_gh_retry.sh"
gh_sanitize_stderr "$CASE_ROOT/create.stderr"
"#,
            "sanitize-test",
        ])
        .arg(root())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", describe(&output));
    assert!(output.stdout.len() <= 500);
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("ghp_"),
        "truncated secret fragment survived"
    );
}

#[test]
fn retry_temp_creation_failure_is_actionable_and_never_runs_gh() {
    let f = PublishFixture::new();
    f.shim(
        "mktemp",
        "echo 'injected private file creation failure' >&2\nexit 77\n",
    );
    // `retry` replaces gh; its stderr marker proves whether it was invoked.
    f.write("create.stderr", "GH_MUST_NOT_RUN");
    let output = retry(&f, "");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("GH_MUST_NOT_RUN"),
        "retry must use checked atomic temp creation"
    );
    assert!(
        stderr.contains("gh pr create") && (stderr.contains("77") || stderr.contains("temp")),
        "temp-file operation failure needs context: {stderr:?}"
    );
}

#[test]
fn retry_capture_files_are_private_cleaned_and_do_not_replace_caller_traps() {
    let f = PublishFixture::new();
    let output = retry(
        &f,
        r#"
trap 'printf caller-trap > "$CASE_ROOT/trap-ran"' EXIT
umask 000
"#,
    );
    assert_eq!(output.status.code(), Some(23));
    let permissions = f.read("capture.permissions");
    assert!(!permissions.is_empty(), "must capture stderr in a file");
    assert!(
        permissions
            .lines()
            .all(|line| line.starts_with("-rw-------")),
        "stderr must be private: {permissions}"
    );
    assert_eq!(f.read("trap-ran"), "caller-trap");
    assert_eq!(
        fs::read_dir(f.tmp.path().join("private")).unwrap().count(),
        0
    );
}

#[test]
fn create_failure_retains_diagnostic_in_structured_result_even_if_collision_lookup_fails() {
    for collision_fails in ["0", "1"] {
        let f = PublishFixture::new();
        f.write("create.stderr", "distinctive publish rejection");
        let output = f
            .publish_command()
            .env("CREATE_STATUS", "23")
            .env("COLLISION_LOOKUP_FAIL", collision_fails)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{}", describe(&output));
        let result = json(&output);
        assert_eq!(result["state"], "FAILED_PR_CREATE");
        assert_eq!(result["terminal_status"], "failure");
        assert_diagnostic(
            result["message"].as_str().unwrap(),
            "distinctive publish rejection",
        );
        assert_diagnostic(
            &String::from_utf8_lossy(&output.stderr),
            "distinctive publish rejection",
        );
    }
}

#[test]
fn create_collision_adopts_only_a_matching_open_pr() {
    for head in [super::fixture::BRANCH, "feat/unrelated"] {
        let f = PublishFixture::new();
        f.write("create.stderr", "distinctive publish rejection");
        f.write(
            "collision.json",
            &serde_json::json!([f.pr("OPEN", head)]).to_string(),
        );
        let output = f
            .publish_command()
            .env("CREATE_STATUS", "23")
            .output()
            .unwrap();
        let result = json(&output);
        if head == super::fixture::BRANCH {
            assert!(output.status.success(), "{}", describe(&output));
            assert_eq!(result["state"], "existing-open-pr");
            assert_eq!(result["pr_url"], "https://github.com/owner/repo/pull/99");
        } else {
            assert!(!output.status.success());
            assert_eq!(result["state"], "FAILED_PR_CREATE");
            assert_diagnostic(
                result["message"].as_str().unwrap(),
                "distinctive publish rejection",
            );
        }
    }
}

#[test]
fn cleanup_failure_does_not_replace_original_gh_failure_status() {
    let f = PublishFixture::new();
    f.write("create.stderr", "distinctive publish rejection");
    f.shim("rm", "exit 80\n");
    let output = retry(&f, "");
    assert_eq!(
        output.status.code(),
        Some(23),
        "cleanup status must not replace provider failure: {}",
        describe(&output)
    );
    assert_diagnostic(
        &String::from_utf8_lossy(&output.stderr),
        "distinctive publish rejection",
    );
}

#[test]
fn mandatory_body_file_creation_and_write_failures_remain_terminal_with_diagnostics() {
    for failure in ["creation", "write"] {
        let f = PublishFixture::new();
        let mut command = f.publish_command();
        if failure == "creation" {
            f.shim(
                "mktemp",
                r#"
if [ -f "$CASE_ROOT/issue-looked-up" ]; then
  echo 'injected mktemp rejection' >&2; exit 77
fi
exec "$REAL_TOOL" "$@"
"#,
            );
        } else {
            let helper = f.tmp.path().join("body-helper.sh");
            executable(
                &helper,
                "#!/bin/bash\nwrite_body_file() { cat >/dev/null; return 78; }\n",
            );
            command.env("WORKFLOW_BODY_FILE_HELPER", helper);
        }
        let output = command.output().unwrap();
        assert!(
            !output.status.success(),
            "mandatory {failure} failure must stop publishing"
        );
        assert!(!f.read("gh.log").contains("pr create"));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("body") || stderr.contains("temp"),
            "failed operation missing: {stderr:?}"
        );
        assert!(
            stderr.contains("77")
                || stderr.contains("78")
                || stderr.contains("failed")
                || stderr.contains("could not"),
            "failure condition missing: {stderr:?}"
        );
    }
}

#[test]
fn retry_normal_return_preserves_caller_traps_options_and_last_class() {
    let f = PublishFixture::new();
    f.write("create.stderr", "authentication failed");
    let output = retry(
        &f,
        r#"
trap 'exit 97' TERM
trap 'exit 98' HUP
trap 'exit 99' INT
saved_traps=$(trap -p)
saved_options=$-
if _gh_retry_core 'pr create' pr create; then exit 96; else status=$?; fi
[ "$status" = 23 ] && [ "$GH_RETRY_LAST_CLASS" = auth ] || exit 95
[ "$saved_traps" = "$(trap -p)" ] && [ "$saved_options" = "$-" ] || exit 94
printf verified > "$CASE_ROOT/restored"
"#,
    );
    assert_eq!(output.status.code(), Some(23), "{}", describe(&output));
    assert_eq!(f.read("restored"), "verified");
}
