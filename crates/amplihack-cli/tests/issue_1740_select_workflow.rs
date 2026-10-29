//! Issue #1740: the `amplihack orch helper select-workflow` CLI. Its wiring
//! into the smart-orchestrator recipes is in `issue_1740_routing_recipes`.
//!
//! The binary is started through [`crate::isolated_amplihack::isolated`] with
//! a temp `HOME`; no model, `gh` or network is involved. The classifier is
//! simulated by the JSON written to the helper's stdin. The output contract
//! (bare value on stdout, warnings then one log line on stderr, exit 0) is
//! fixed by `docs/reference/quick-change-recipe.md`.

use std::io::Write;
use std::process::{Output, Stdio};

use crate::support::amplihack_bin::amplihack_bin;

const LOG_RE: &str = r"^\[dev-orchestrator\] Workflow: (quick-change|default-workflow|none) source=(override|classifier|default) reason=[^\n]{1,200}$";

fn select(args: &[&str], stdin: &[u8]) -> (String, Vec<String>) {
    let home = tempfile::tempdir().expect("temp HOME");
    let mut cmd = crate::isolated_amplihack::isolated(amplihack_bin());
    cmd.args(["orch", "helper", "select-workflow"])
        .args(args)
        .env("HOME", home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn amplihack");
    // The helper may stop reading at 1 MiB; a broken pipe here is expected.
    let _ = child.stdin.take().unwrap().write_all(stdin);
    let out: Output = child.wait_with_output().expect("wait for amplihack");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success(),
        "select-workflow must always exit 0; stderr:\n{stderr}"
    );
    let stdout = String::from_utf8(out.stdout).expect("stdout is UTF-8");
    let lines = stderr.lines().map(str::to_string).collect();
    (stdout, lines)
}

fn classifier(workflow: &str, reason: &str) -> Vec<u8> {
    serde_json::json!({
        "task_type": "Development",
        "workstreams": [{"name": "w", "classification": "Development"}],
        "recommended_workflow": workflow,
        "workflow_reason": reason,
    })
    .to_string()
    .into_bytes()
}

fn dev(count: &str) -> Vec<&str> {
    vec!["--task-type", "Development", "--workstream-count", count]
}

fn log_line(stderr: &[String]) -> &str {
    let line = stderr
        .last()
        .unwrap_or_else(|| panic!("no log line on stderr: {stderr:?}"));
    let re = regex::Regex::new(LOG_RE).unwrap();
    assert!(
        re.is_match(line),
        "log line {line:?} does not match {LOG_RE}"
    );
    line
}

/// The warning must come before the log line, which is always last.
fn assert_has(stderr: &[String], warning: &str) {
    let at = stderr.iter().position(|l| l == warning);
    assert!(
        at.is_some_and(|i| i + 1 < stderr.len()),
        "stderr must hold {warning:?} before the log line: {stderr:?}"
    );
}

#[test]
fn small_task_categories_resolve_to_quick_change() {
    for reason in [
        "documentation-only edit to one file",
        "Cargo.lock update for a patch release",
        "comment typo fix",
    ] {
        let (out, err) = select(&dev("1"), &classifier("quick-change", reason));
        assert_eq!(out, "quick-change", "{reason}");
        assert_eq!(
            log_line(&err),
            format!("[dev-orchestrator] Workflow: quick-change source=classifier reason={reason}")
        );
    }
}

#[test]
fn feature_and_behaviour_change_resolve_to_default_workflow() {
    for reason in [
        "new CLI subcommand",
        "changes retry behaviour visible to users",
    ] {
        let (out, err) = select(&dev("1"), &classifier("default-workflow", reason));
        assert_eq!(out, "default-workflow");
        assert!(log_line(&err).contains("source=classifier"), "{err:?}");
    }
}

#[test]
fn several_workstreams_cannot_select_quick_change_from_the_classifier() {
    let (out, err) = select(&dev("2"), &classifier("quick-change", "doc"));
    assert_eq!(out, "default-workflow");
    assert_eq!(
        log_line(&err),
        "[dev-orchestrator] Workflow: default-workflow source=classifier reason=quick-change needs one workstream; got 2"
    );
}

#[test]
fn override_forces_either_recipe() {
    let mut args = vec!["--override", "quick-change"];
    args.extend(dev("3"));
    let (out, err) = select(&args, &classifier("default-workflow", "big"));
    assert_eq!(out, "quick-change");
    assert_eq!(
        log_line(&err),
        "[dev-orchestrator] Workflow: quick-change source=override reason=-c workflow=quick-change"
    );

    let mut args = vec!["--override", "default-workflow"];
    args.extend(dev("1"));
    let (out, err) = select(&args, &classifier("quick-change", "typo"));
    assert_eq!(out, "default-workflow");
    assert!(log_line(&err).contains("source=override"));
}

#[test]
fn unknown_override_warns_then_falls_back_to_the_classifier() {
    let mut args = vec!["--override", "quick"];
    args.extend(dev("1"));
    let (out, err) = select(&args, &classifier("quick-change", "typo"));
    assert_eq!(out, "quick-change");
    assert_has(
        &err,
        "[dev-orchestrator] WARNING: unknown workflow override 'quick' ignored",
    );
    assert!(log_line(&err).contains("source=classifier"));
}

#[test]
fn override_on_a_non_development_task_is_ignored_with_a_warning() {
    let args = [
        "--override",
        "quick-change",
        "--task-type",
        "Q&A",
        "--workstream-count",
        "1",
    ];
    let (out, err) = select(&args, &classifier("quick-change", "x"));
    assert_eq!(out, "none");
    assert_has(
        &err,
        "[dev-orchestrator] WARNING: workflow override 'quick-change' ignored for task_type Q&A",
    );
    assert!(log_line(&err).starts_with("[dev-orchestrator] Workflow: none source=override"));
}

#[test]
fn missing_or_invalid_classifier_output_gives_default_unspecified() {
    for stdin in [&b""[..], b"not json", b"{}"] {
        let (out, err) = select(&dev("1"), stdin);
        assert_eq!(out, "default-workflow");
        assert_eq!(
            log_line(&err),
            "[dev-orchestrator] Workflow: default-workflow source=default reason=unspecified"
        );
    }
}

#[test]
fn long_multi_line_reason_is_logged_as_one_line_of_200_chars() {
    let reason = format!("{}\n{}", "r".repeat(500), "second line");
    let (_, err) = select(&dev("1"), &classifier("quick-change", &reason));
    let line = log_line(&err);
    let logged = line.split_once("reason=").unwrap().1;
    assert_eq!(logged.chars().count(), 200);
    assert!(
        !err.iter()
            .any(|l| l.contains("second line") && !l.starts_with("[dev-orchestrator]")),
        "the reason must not add stderr lines: {err:?}"
    );
}

#[test]
fn escape_and_bidi_characters_never_reach_the_log() {
    let reason = "ok\x1b[2J\u{202e}evil\u{2066}x\u{200b}y";
    let (_, err) = select(&dev("1"), &classifier("quick-change", reason));
    let line = log_line(&err);
    assert!(!line.contains('\x1b'), "{line:?}");
    for c in ['\u{202e}', '\u{2066}', '\u{200b}'] {
        assert!(!line.contains(c), "{line:?} still holds {c:?}");
    }
}

#[test]
fn oversized_stdin_is_treated_as_empty() {
    let mut big = classifier("quick-change", "typo");
    big.extend(std::iter::repeat_n(b' ', 2 << 20));
    let (out, err) = select(&dev("1"), &big);
    assert_eq!(out, "default-workflow");
    assert!(log_line(&err).contains("source=default reason=unspecified"));
}
