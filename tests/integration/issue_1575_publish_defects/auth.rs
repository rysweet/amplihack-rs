//! Defect 3: `workflow_gh_auth_gate.sh` retries `gh auth status` with capped
//! exponential backoff and checks push permission before step 21 mutates the
//! PR. `sleep` is stubbed so backoff is recorded, never waited.

use std::fs;
use std::process::Output;

use super::{Sandbox, assert_line, dump, helper_path, text};

pub(super) const AUTH_ERROR: &str =
    "ERROR: gh CLI is unavailable or unauthenticated — cannot verify or mutate GitHub PR readiness";

/// gh stub driven by env: GH_AUTH_FAILS failing `auth status` calls before
/// success (`always` = never succeed); GH_PUSH_SEQ comma list of answers for
/// `api repos/...` (last one repeats; `ERR` = exit 1; `EMPTY` = an empty line);
/// GH_LOGIN for `api user`.
pub(super) const SCRIPTED_GH: &str = r#"#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$GH_CALL_LOG"
count() { local f="$GH_STUB_STATE/$1" n=0; [ -f "$f" ] && n=$(cat "$f"); n=$((n+1)); echo "$n" > "$f"; echo "$n"; }
case "${1:-} ${2:-}" in
  "auth status")
    n=$(count auth)
    if [ "${GH_AUTH_FAILS:-0}" = always ] || [ "$n" -le "${GH_AUTH_FAILS:-0}" ]; then
      echo "You are not logged into any GitHub hosts" >&2; exit 1
    fi
    exit 0 ;;
  "api user") printf '%s\n' "${GH_LOGIN:-tester}"; exit 0 ;;
  "api repos/"*)
    n=$(count push)
    IFS=, read -ra seq <<< "${GH_PUSH_SEQ:-true}"
    i=$(( n <= ${#seq[@]} ? n - 1 : ${#seq[@]} - 1 ))
    [ "${seq[$i]}" = ERR ] && { echo "HTTP 502" >&2; exit 1; }
    [ "${seq[$i]}" = EMPTY ] && { echo; exit 0; }
    printf '%s\n' "${seq[$i]}"; exit 0 ;;
esac
echo "unexpected gh call: $*" >&2
exit 99
"#;

pub(super) fn sandbox() -> Sandbox {
    let sb = Sandbox::new();
    sb.write_gh(SCRIPTED_GH);
    sb.stub_sleep();
    sb
}

/// Source the gate into a strict caller shell and run `call`.
fn gate(sb: &Sandbox, call: &str, env: &[(&str, &str)]) -> Output {
    let script = format!(
        "set -euo pipefail\n. {:?}\n{call} && echo RC=0 || echo RC=$?\n",
        helper_path("workflow_gh_auth_gate.sh")
            .display()
            .to_string()
    );
    let mut cmd = sb.command("bash", sb.root());
    cmd.arg("-c").arg(script);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("run gate")
}

fn rc(output: &Output) -> String {
    text(&output.stdout)
        .lines()
        .last()
        .unwrap_or_default()
        .to_string()
}

pub(super) fn count_calls(sb: &Sandbox, prefix: &str) -> usize {
    sb.gh_calls()
        .lines()
        .filter(|l| l.starts_with(prefix))
        .count()
}

fn sleeps(sb: &Sandbox) -> Vec<String> {
    fs::read_to_string(sb.root().join("sleep-calls.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn auth_gate_retries_then_succeeds() {
    let sb = sandbox();
    let out = gate(&sb, "gh_auth_gate", &[("GH_AUTH_FAILS", "2")]);
    assert_eq!(rc(&out), "RC=0", "{}", dump(&out));
    let stderr = text(&out.stderr);
    assert_line(
        &stderr,
        "WARNING: gh auth status failed (attempt 1/5); retrying in 0s",
        "retry 1",
    );
    assert_line(
        &stderr,
        "WARNING: gh auth status failed (attempt 2/5); retrying in 0s",
        "retry 2",
    );
    assert!(!stderr.contains("ERROR"), "{stderr}");
    assert_eq!(count_calls(&sb, "auth status"), 3);
}

#[test]
fn auth_gate_gives_the_contract_error_after_the_budget() {
    let sb = sandbox();
    let out = gate(
        &sb,
        "gh_auth_gate",
        &[
            ("GH_AUTH_FAILS", "always"),
            ("WORKFLOW_GH_AUTH_ATTEMPTS", "3"),
        ],
    );
    assert_eq!(rc(&out), "RC=1", "{}", dump(&out));
    let stderr = text(&out.stderr);
    assert_line(&stderr, AUTH_ERROR, "final error");
    assert_eq!(
        stderr.matches("WARNING: gh auth status failed").count(),
        2,
        "{stderr}"
    );
    assert_eq!(count_calls(&sb, "auth status"), 3);
}

#[test]
fn backoff_doubles_and_is_capped() {
    let sb = sandbox();
    let out = gate(
        &sb,
        "gh_auth_gate",
        &[
            ("GH_AUTH_FAILS", "always"),
            ("WORKFLOW_GH_AUTH_ATTEMPTS", "6"),
            ("WORKFLOW_GH_AUTH_BASE_DELAY", "4"),
            ("WORKFLOW_GH_AUTH_MAX_DELAY", "20"),
        ],
    );
    assert_eq!(rc(&out), "RC=1", "{}", dump(&out));
    assert_eq!(sleeps(&sb), ["4", "8", "16", "20", "20"]);
    assert_line(
        &text(&out.stderr),
        "WARNING: gh auth status failed (attempt 4/6); retrying in 20s",
        "cap",
    );
}

#[test]
fn defaults_apply_when_settings_are_unset() {
    let sb = sandbox();
    // The sandbox sets BASE_DELAY=0 for every child; empty means unset here.
    let out = gate(
        &sb,
        "gh_auth_gate",
        &[
            ("GH_AUTH_FAILS", "always"),
            ("WORKFLOW_GH_AUTH_BASE_DELAY", ""),
        ],
    );
    assert_eq!(rc(&out), "RC=1", "{}", dump(&out));
    assert_eq!(count_calls(&sb, "auth status"), 5);
    assert_eq!(sleeps(&sb), ["4", "8", "16", "32"]);
}

#[test]
fn invalid_settings_warn_once_and_use_defaults() {
    for bad in ["abc", "0", "11", "-1", "1e3", "5 "] {
        let sb = sandbox();
        let out = gate(
            &sb,
            "gh_auth_gate",
            &[
                ("GH_AUTH_FAILS", "always"),
                ("WORKFLOW_GH_AUTH_ATTEMPTS", bad),
            ],
        );
        assert_eq!(rc(&out), "RC=1", "attempts={bad:?}\n{}", dump(&out));
        assert_eq!(
            count_calls(&sb, "auth status"),
            5,
            "attempts={bad:?} must fall back to 5"
        );
        let warnings = text(&out.stderr)
            .lines()
            .filter(|l| l.starts_with("WARNING") && l.contains("WORKFLOW_GH_AUTH_ATTEMPTS"))
            .count();
        assert_eq!(warnings, 1, "attempts={bad:?}\n{}", dump(&out));
    }
}

#[test]
fn leading_zero_settings_are_decimal() {
    let sb = sandbox();
    let out = gate(
        &sb,
        "gh_auth_gate",
        &[
            ("GH_AUTH_FAILS", "always"),
            ("WORKFLOW_GH_AUTH_ATTEMPTS", "08"),
        ],
    );
    assert_eq!(rc(&out), "RC=1", "{}", dump(&out));
    assert_eq!(count_calls(&sb, "auth status"), 8);
}

#[test]
fn settings_never_reach_arithmetic_unvalidated() {
    let sb = sandbox();
    let marker = sb.root().join("pwned");
    let payload = format!("a[$(touch {})]", marker.display());
    for var in [
        "WORKFLOW_GH_AUTH_ATTEMPTS",
        "WORKFLOW_GH_AUTH_BASE_DELAY",
        "WORKFLOW_GH_AUTH_MAX_DELAY",
    ] {
        let out = gate(
            &sb,
            "gh_auth_gate",
            &[("GH_AUTH_FAILS", "1"), (var, payload.as_str())],
        );
        assert_eq!(rc(&out), "RC=0", "{var}\n{}", dump(&out));
        assert!(!marker.exists(), "{var} was evaluated as arithmetic");
    }
}

#[test]
fn sourcing_and_calling_leave_caller_options_and_traps_alone() {
    let sb = sandbox();
    let script = format!(
        "set -eu\nset +o pipefail\ntrap 'echo bye' EXIT\nbefore=\"$-|$(shopt -po pipefail)|$(trap -p EXIT)\"\n\
         . {:?}\ngh_auth_gate\ngh_push_permission_gate owner/repo\n\
         after=\"$-|$(shopt -po pipefail)|$(trap -p EXIT)\"\n[ \"$before\" = \"$after\" ] && echo SAME || echo \"DIFF $before // $after\"\n\
         [ -z \"${{attempt:-}}${{delay:-}}${{i:-}}\" ] && echo NOLEAK || echo LEAK\n",
        helper_path("workflow_gh_auth_gate.sh")
            .display()
            .to_string()
    );
    let out = sb
        .command("bash", sb.root())
        .arg("-c")
        .arg(script)
        .output()
        .unwrap();
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains("SAME") && stdout.contains("NOLEAK"),
        "{}",
        dump(&out)
    );
}

#[test]
fn push_permission_true_passes_with_one_call() {
    let sb = sandbox();
    let out = gate(&sb, "gh_push_permission_gate owner/repo", &[]);
    assert_eq!(rc(&out), "RC=0", "{}", dump(&out));
    assert_eq!(count_calls(&sb, "api repos/owner/repo"), 1);
    assert!(
        sb.gh_calls()
            .contains("api repos/owner/repo --jq .permissions.push"),
        "{}",
        sb.gh_calls()
    );
}

#[test]
fn push_false_then_true_retries_and_passes() {
    let sb = sandbox();
    let out = gate(
        &sb,
        "gh_push_permission_gate owner/repo",
        &[("GH_PUSH_SEQ", "false,true")],
    );
    assert_eq!(rc(&out), "RC=0", "{}", dump(&out));
    assert_line(
        &text(&out.stderr),
        "WARNING: gh reports push=false on owner/repo (attempt 1/5); retrying in 0s",
        "push retry",
    );
    assert_eq!(count_calls(&sb, "api repos/"), 2);
}

#[test]
fn push_false_throughout_fails_naming_the_login() {
    let sb = sandbox();
    let out = gate(
        &sb,
        "gh_push_permission_gate owner/repo",
        &[
            ("GH_PUSH_SEQ", "false"),
            ("GH_LOGIN", "ryan-mistt"),
            ("WORKFLOW_GH_AUTH_ATTEMPTS", "3"),
        ],
    );
    assert_eq!(rc(&out), "RC=1", "{}", dump(&out));
    assert_line(
        &text(&out.stderr),
        "ERROR: active gh account 'ryan-mistt' lacks push permission on owner/repo",
        "denied",
    );
    assert_eq!(count_calls(&sb, "api repos/"), 3);
}

#[test]
fn suspicious_login_is_printed_as_unknown() {
    let sb = sandbox();
    let out = gate(
        &sb,
        "gh_push_permission_gate owner/repo",
        &[("GH_PUSH_SEQ", "false"), ("GH_LOGIN", "x';rm -rf /")],
    );
    assert_eq!(rc(&out), "RC=1", "{}", dump(&out));
    assert_line(
        &text(&out.stderr),
        "ERROR: active gh account 'unknown' lacks push permission on owner/repo",
        "unknown",
    );
}

#[test]
fn unknown_permission_result_warns_once_and_passes_without_retry() {
    for answer in ["ERR", "null", "EMPTY", "TRUE", "maybe"] {
        let sb = sandbox();
        let out = gate(
            &sb,
            "gh_push_permission_gate owner/repo",
            &[("GH_PUSH_SEQ", answer)],
        );
        assert_eq!(rc(&out), "RC=0", "answer={answer:?}\n{}", dump(&out));
        assert_line(
            &text(&out.stderr),
            "WARNING: could not determine push permission on owner/repo; continuing (GitHub enforces permissions on the mutation)",
            "unknown",
        );
        assert_eq!(count_calls(&sb, "api repos/"), 1, "answer={answer:?}");
    }
}

#[test]
fn invalid_repo_identity_skips_the_check_without_calling_gh() {
    for id in [
        "",
        "owner",
        "owner/.",
        "owner/..",
        "owner/.hidden",
        "../x",
        "own er/repo",
        "owner/repo/extra",
    ] {
        let sb = sandbox();
        let out = gate(&sb, &format!("gh_push_permission_gate {id:?}"), &[]);
        assert_eq!(rc(&out), "RC=0", "id={id:?}\n{}", dump(&out));
        assert_line(
            &text(&out.stderr),
            "WARNING: repo identity is not a valid GitHub owner/repo; skipping push-permission check",
            "invalid id",
        );
        assert_eq!(count_calls(&sb, "api "), 0, "id={id:?}");
    }
}
