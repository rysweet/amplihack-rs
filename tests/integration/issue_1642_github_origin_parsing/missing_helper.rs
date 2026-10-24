//! Group (e): a missing or unreadable `workflow_github_repo.sh` fails closed.
//!
//! - The five standalone scripts and the `workflow_agentic_finalization.sh`
//!   entry point print an ERROR naming the helper and exit 2 before any gh
//!   repository call (D1b).
//! - The two sourced libraries never `exit`: they WARN, leave the parser
//!   undefined, and classify every origin as not GitHub (D1a).
//! - An override that names an existing but unreadable file (`chmod 000`) is
//!   rejected up front with exit 2, not later at `.` with some other code.

use super::callers;
use super::fixture::{BRANCH, Fixture, Outcome, tools_dir};
use std::fs;
use std::path::{Path, PathBuf};

/// (script, args, extra env) for each standalone caller, driven so that a
/// present helper would take its GitHub path for `o/r`.
/// (script, args, extra env).
type Standalone = (
    &'static str,
    &'static [&'static str],
    &'static [(&'static str, &'static str)],
);

const STANDALONE: &[Standalone] = &[
    ("workflow_pr_ready.sh", &[], &[]),
    ("workflow_pr_scope.sh", &["--base", "main"], &[]),
    (
        "workflow_publish_pr.sh",
        &[],
        &[("REMOTE_HOST_TYPE", "github"), ("ISSUE_NUMBER", "1642")],
    ),
    ("workflow_terminal_evidence.sh", &["collect"], &[]),
    (
        "workflow_final_status.sh",
        &[],
        &[("REMOTE_HOST_TYPE", "github"), ("ISSUE_NUMBER", "1642")],
    ),
];

/// Modes of the finalization entry point; each must stop before sourcing.
const FINALIZATION_MODES: &[&str] = &["collect", "validate", "complete"];

/// A copy of the bundle's top-level tools with the shared helper left out.
fn tools_without_helper(fx: &Fixture) -> PathBuf {
    let dest = fx.path("bundle/tools");
    fs::create_dir_all(&dest).expect("mkdir copied tools");
    for entry in fs::read_dir(tools_dir()).expect("read tools dir") {
        let entry = entry.expect("tools entry");
        let name = entry.file_name();
        if name == "workflow_github_repo.sh" || !entry.path().is_file() {
            continue;
        }
        fs::copy(entry.path(), dest.join(&name)).expect("copy tool");
    }
    dest
}

/// A copy of the real helper with mode 000, for the unreadable-override case.
fn unreadable_helper(fx: &Fixture) -> PathBuf {
    let path = fx.path("unreadable/workflow_github_repo.sh");
    fs::create_dir_all(path.parent().unwrap()).expect("mkdir unreadable");
    fs::copy(super::fixture::helper_path(), &path).expect("copy helper");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).expect("chmod 000");
    }
    path
}

/// root reads a mode-000 file anyway, so the unreadable case is meaningless.
fn running_as_root() -> bool {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
        .unwrap_or(false)
}

fn run_script(
    fx: &Fixture,
    script: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
    helper_override: Option<&Path>,
) -> Outcome {
    let mut cmd = fx.command("bash");
    cmd.arg(script).args(args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    if let Some(helper) = helper_override {
        cmd.env("WORKFLOW_GITHUB_REPO_HELPER", helper);
    }
    fx.run(cmd)
}

/// exit 2, an ERROR naming the helper, and no gh repository call.
fn check_exit_2(label: &str, fx: &Fixture, outcome: &Outcome, failures: &mut Vec<String>) {
    let calls = fx.repo_gh_calls();
    let named =
        outcome.stderr.contains("ERROR") && outcome.stderr.contains("workflow_github_repo.sh");
    if outcome.output.status.code() != Some(2) || !calls.is_empty() || !named {
        failures.push(format!(
            "{label}: want exit 2, an ERROR naming workflow_github_repo.sh and no gh repo call; gh calls {calls:?}\n{}",
            outcome.describe()
        ));
    }
}

#[test]
fn standalone_callers_fail_closed_with_exit_2_when_the_helper_is_missing() {
    let mut failures = Vec::new();
    for (script, args, envs) in STANDALONE {
        let fx = Fixture::with_origin("https://github.com/o/r.git");
        let tools = tools_without_helper(&fx);
        let outcome = run_script(&fx, &tools.join(script), args, envs, None);
        check_exit_2(script, &fx, &outcome, &mut failures);
        if *script == "workflow_pr_scope.sh"
            && outcome.json().map(|j| j["reason"].clone())
                != Some(serde_json::json!("missing_github_repo_helper"))
        {
            failures.push(format!(
                "{script}: stdout must carry reason missing_github_repo_helper\n{}",
                outcome.describe()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn agentic_finalization_exits_2_before_sourcing_when_the_helper_is_missing() {
    let mut failures = Vec::new();
    for mode in FINALIZATION_MODES {
        let fx = Fixture::with_origin("https://github.com/o/r.git");
        let tools = tools_without_helper(&fx);
        let script = tools.join("workflow_agentic_finalization.sh");
        let outcome = run_script(&fx, &script, &[mode], &[], None);
        check_exit_2(
            &format!("agentic_finalization {mode}"),
            &fx,
            &outcome,
            &mut failures,
        );
        if outcome.stdout.contains("github_remote") {
            failures.push(format!(
                "agentic_finalization {mode}: must stop before emitting evidence\n{}",
                outcome.describe()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn agentic_finalization_exits_2_when_the_override_names_a_missing_file() {
    let fx = Fixture::with_origin("https://github.com/o/r.git");
    let missing = fx.path("nowhere/workflow_github_repo.sh");
    // The real helper sits next to the script; a set-but-broken override must
    // still fail rather than fall back to it.
    let outcome = run_script(
        &fx,
        &super::fixture::tool_path("workflow_agentic_finalization.sh"),
        &["collect"],
        &[],
        Some(&missing),
    );
    let mut failures = Vec::new();
    check_exit_2(
        "agentic_finalization override",
        &fx,
        &outcome,
        &mut failures,
    );
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn unreadable_override_exits_2_for_every_standalone_caller_and_the_entry_point() {
    if running_as_root() {
        eprintln!("skipped: root can read a mode-000 file");
        return;
    }
    let mut failures = Vec::new();
    for (script, args, envs) in STANDALONE {
        let fx = Fixture::with_origin("https://github.com/o/r.git");
        let helper = unreadable_helper(&fx);
        let path = super::fixture::tool_path(script);
        let outcome = run_script(&fx, &path, args, envs, Some(&helper));
        check_exit_2(&format!("{script} chmod 000"), &fx, &outcome, &mut failures);
    }
    let fx = Fixture::with_origin("https://github.com/o/r.git");
    let helper = unreadable_helper(&fx);
    let outcome = run_script(
        &fx,
        &super::fixture::tool_path("workflow_agentic_finalization.sh"),
        &["collect"],
        &[],
        Some(&helper),
    );
    check_exit_2(
        "agentic_finalization chmod 000",
        &fx,
        &outcome,
        &mut failures,
    );
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn pr_signal_library_fails_closed_when_the_helper_is_missing() {
    let fx = Fixture::with_origin("https://github.com/o/r.git");
    let tools = tools_without_helper(&fx);

    let mut cmd = fx.command("bash");
    cmd.arg("-c")
        .arg(r#"set -u; . "$1"; . "$2"; shift 2; finalization_open_pr_signal "$@""#)
        .arg("signal")
        .arg(tools.join("workflow_gh_retry.sh"))
        .arg(tools.join("workflow_finalization_pr_signal.sh"))
        .arg(&fx.repo)
        .arg(BRANCH);
    let signal = fx.run(cmd);
    assert!(
        !signal.success() && fx.repo_gh_calls().is_empty(),
        "pr_signal without the helper must give no signal and call no gh; gh calls {:?}\n{}",
        fx.repo_gh_calls(),
        signal.describe()
    );
    assert!(
        signal.stderr.contains("WARNING") && signal.stderr.contains("workflow_github_repo.sh"),
        "pr_signal must warn that the helper is missing\n{}",
        signal.describe()
    );
}

/// The functions the entry point defines before sourcing the libraries, then
/// the libraries themselves (the identity helper is not among them).
const COLLECT_LIBRARY_ONLY: &str = r#"set -euo pipefail
d="$1"
boolish() { case "$1" in true|1|yes|y) printf 'true' ;; *) printf 'false' ;; esac; }
. "$d/workflow_finalization_context.sh"
resolve_publish_state_reached() { boolish "$(workflow_finalization_metadata publish_state_reached || :)"; }
resolve_pr_url() { workflow_finalization_metadata pr_url || :; }
resolve_pr_number() { workflow_finalization_metadata pr_number || :; }
. "$d/workflow_finalization_metadata.sh"
. "$d/workflow_finalization_collect.sh"
collect_evidence"#;

#[test]
fn collect_library_sourced_alone_fails_closed_when_the_helper_is_missing() {
    // Sourced directly, not through the entry point (which now exits 2): the
    // library itself must never exit and must report github_remote=false.
    let fx = Fixture::with_origin("https://github.com/o/r.git");
    let tools = tools_without_helper(&fx);
    let mut cmd = fx.command("bash");
    cmd.arg("-c")
        .arg(COLLECT_LIBRARY_ONLY)
        .arg("collect")
        .arg(&tools);
    let collect = fx.run(cmd);
    assert!(
        collect.stderr.contains("WARNING") && collect.stderr.contains("workflow_github_repo.sh"),
        "collect library must warn that the helper is missing\n{}",
        collect.describe()
    );
    assert_eq!(
        callers::github_remote(&collect),
        "false",
        "collect library without the helper must fail closed to github_remote=false\n{}",
        collect.describe()
    );
}
