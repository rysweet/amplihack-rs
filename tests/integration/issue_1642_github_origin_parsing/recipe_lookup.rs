//! Group (d), helper lookup: `probe-terminal-state` in
//! `workflow-terminal-state.yaml` finds `workflow_github_repo.sh`.
//!
//! - With no helper at any rung of the resolution ladder it fails with
//!   FAILED_INVALID_INPUT, names the helper and says to run `amplihack install`.
//! - A WORKFLOW_GITHUB_REPO_HELPER override that is set but missing or
//!   unreadable fails with FAILED_INVALID_INPUT. It never falls through to a
//!   later candidate (S2), so these tests cannot pass by finding an installed
//!   copy.

use super::fixture::{BRANCH, Fixture, Outcome, helper_path, tool_path, workspace_root};
use super::recipe::probe_command;
use std::fs;

/// The probe with no override, no AMPLIHACK_HOME unless given, and a HOME
/// holding no installed bundle.
pub(super) fn run_probe_bare(fx: &Fixture, envs: &[(&str, &std::ffi::OsStr)]) -> Outcome {
    let empty_home = fx.path("empty-home");
    fs::create_dir_all(&empty_home).expect("empty home");
    // The probe also needs recipe_var.sh and worktree_setup_path.sh (#1561).
    // This bundle holds those two only, so workflow_github_repo.sh stays
    // unresolvable unless a test supplies it.
    let partial_bundle = fx.path("partial-bundle");
    let partial_tools = partial_bundle.join("amplifier-bundle/tools");
    fs::create_dir_all(&partial_tools).expect("partial bundle");
    for tool in ["recipe_var.sh", "worktree_setup_path.sh"] {
        fs::copy(tool_path(tool), partial_tools.join(tool)).expect("copy bundle tool");
    }
    let mut cmd = fx.command("bash");
    cmd.arg("-c")
        .arg(probe_command())
        .env("HOME", &empty_home)
        .env("AMPLIHACK_HOME", &partial_bundle)
        .env("REPO_PATH", &fx.repo)
        // #1561: the probe reads its inputs only as RECIPE_VAR_<key>.
        .env("RECIPE_VAR_repo_path", &fx.repo)
        .env("RECIPE_VAR_branch_name", BRANCH)
        .env("RECIPE_VAR_base_ref", "main")
        .env("RECIPE_VAR_pr_number", "")
        .env("RECIPE_VAR_pr_url", "")
        .env("RECIPE_VAR_goal_already_met", "false")
        // Keep the scope helper resolvable so only the identity helper
        // lookup can make this run fail.
        .env(
            "WORKFLOW_PR_SCOPE_HELPER",
            tool_path("workflow_pr_scope.sh"),
        );
    for (k, v) in envs {
        cmd.env(k, v);
    }
    fx.run(cmd)
}

fn assert_failed_invalid_input(fx: &Fixture, out: &Outcome, why: &str) {
    assert!(
        out.stderr.contains("workflow_github_repo.sh"),
        "{why}: the failure must name the helper\n{}",
        out.describe()
    );
    let state = out.json().map(|json| json["terminal_state"].clone());
    assert_eq!(
        state,
        Some(serde_json::json!("FAILED_INVALID_INPUT")),
        "{why}: must fail closed\n{}",
        out.describe()
    );
    assert!(
        fx.repo_gh_calls().is_empty(),
        "{why}: gh calls {:?}",
        fx.repo_gh_calls()
    );
}

#[test]
fn probe_fails_closed_when_no_helper_is_found() {
    // REPO_PATH and cwd are the fixture repo and HOME is empty, so no rung of
    // the ladder holds the helper.
    let fx = Fixture::with_origin("https://github.com/o/r.git");
    let out = run_probe_bare(&fx, &[]);
    assert_failed_invalid_input(&fx, &out, "no helper found");
    assert!(
        out.stderr.contains("amplihack install"),
        "the failure must say how to install the helper\n{}",
        out.describe()
    );
}

#[test]
fn probe_fails_on_a_set_but_missing_override_instead_of_falling_through() {
    let fx = Fixture::with_origin("https://github.com/o/r.git");
    let missing = fx.path("nowhere/workflow_github_repo.sh");
    // AMPLIHACK_HOME points at this repo, whose bundle has the real helper:
    // a fall-through would find it and the probe would succeed.
    let root = workspace_root();
    let out = run_probe_bare(
        &fx,
        &[
            ("WORKFLOW_GITHUB_REPO_HELPER", missing.as_os_str()),
            ("AMPLIHACK_HOME", root.as_os_str()),
        ],
    );
    assert_failed_invalid_input(&fx, &out, "set-but-missing override");
}

#[test]
fn probe_fails_on_an_unreadable_override_instead_of_falling_through() {
    let uid = std::process::Command::new("id")
        .arg("-u")
        .output()
        .expect("id -u");
    if String::from_utf8_lossy(&uid.stdout).trim() == "0" {
        eprintln!("skipped: root can read a mode-000 file");
        return;
    }
    let fx = Fixture::with_origin("https://github.com/o/r.git");
    let unreadable = fx.path("unreadable/workflow_github_repo.sh");
    fs::create_dir_all(unreadable.parent().unwrap()).expect("mkdir");
    fs::copy(helper_path(), &unreadable).expect("copy helper");
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).expect("chmod 000");
    }
    let root = workspace_root();
    let out = run_probe_bare(
        &fx,
        &[
            ("WORKFLOW_GITHUB_REPO_HELPER", unreadable.as_os_str()),
            ("AMPLIHACK_HOME", root.as_os_str()),
        ],
    );
    assert_failed_invalid_input(&fx, &out, "unreadable override");
}

#[test]
fn probe_with_a_valid_override_succeeds_for_a_real_origin() {
    // Control for the two override tests above: the same setup with the real
    // helper must not fail, so their failure is caused by the override.
    let fx = Fixture::with_origin("https://github.com/o/r.git");
    let helper = helper_path();
    let out = run_probe_bare(&fx, &[("WORKFLOW_GITHUB_REPO_HELPER", helper.as_os_str())]);
    let state = out.json().map(|json| json["terminal_state"].clone());
    assert_ne!(
        state,
        Some(serde_json::json!("FAILED_INVALID_INPUT")),
        "a valid override must resolve\n{}",
        out.describe()
    );
}
