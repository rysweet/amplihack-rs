//! Group (d), helper lookup: `probe-terminal-state` in
//! `workflow-terminal-state.yaml` finds `workflow_github_repo.sh`.
//!
//! - It never sources a helper whose physical file is inside the checkout it
//!   is judging, whether planted there or reached through a symlink (S3).
//! - A WORKFLOW_GITHUB_REPO_HELPER override that is set but missing or
//!   unreadable fails with FAILED_INVALID_INPUT. It never falls through to a
//!   later candidate (S2), so these tests cannot pass by finding an installed
//!   copy.

use super::fixture::{
    BRANCH, Fixture, Outcome, helper_path, tool_path, workspace_root, write_executable,
};
use super::recipe::probe_command;
use std::fs;
use std::path::{Path, PathBuf};

/// A helper that leaves a marker when sourced and claims every URL is o/r.
fn plant_helper(fx: &Fixture, path: &Path) -> PathBuf {
    let marker = fx.path("planted-helper-ran");
    write_executable(
        path,
        &format!(
            "#!/usr/bin/env bash\n: > '{}'\nparse_github_repo_identity() {{ printf 'o/r\\n'; }}\n",
            marker.display()
        ),
    );
    marker
}

/// The probe with no override, no AMPLIHACK_HOME unless given, and a HOME
/// holding no installed bundle.
fn run_probe_bare(fx: &Fixture, envs: &[(&str, &std::ffi::OsStr)]) -> Outcome {
    let empty_home = fx.path("empty-home");
    fs::create_dir_all(&empty_home).expect("empty home");
    let mut cmd = fx.command("bash");
    cmd.arg("-c")
        .arg(probe_command())
        .env("HOME", &empty_home)
        .env("REPO_PATH", &fx.repo)
        .env("BRANCH_NAME", BRANCH)
        .env("BASE_REF", "main")
        .env("PR_NUMBER", "")
        .env("PR_URL", "")
        .env("GOAL_ALREADY_MET", "false")
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
fn probe_never_sources_a_helper_planted_inside_the_target_checkout() {
    let fx = Fixture::with_origin("https://github.com/o/r.git");
    let marker = plant_helper(
        &fx,
        &fx.repo
            .join("amplifier-bundle/tools/workflow_github_repo.sh"),
    );
    fx.git(&["add", "amplifier-bundle"]);
    fx.git(&["commit", "-q", "-m", "plant helper"]);

    // REPO_PATH and cwd are the target, HOME is empty: every rung either is
    // the target or misses.
    let out = run_probe_bare(&fx, &[]);
    assert!(
        !marker.exists(),
        "a helper under TARGET_REPO_DIR must never be sourced\n{}",
        out.describe()
    );
    assert_failed_invalid_input(&fx, &out, "no trusted helper found");
}

#[test]
fn probe_never_sources_a_tools_dir_symlinked_into_the_target_checkout() {
    let fx = Fixture::with_origin("https://github.com/o/r.git");
    let marker = plant_helper(&fx, &fx.repo.join("planted/workflow_github_repo.sh"));
    let home = fx.path("fake-amplihack-home");
    fs::create_dir_all(home.join("amplifier-bundle")).expect("mkdir fake home");
    std::os::unix::fs::symlink(fx.repo.join("planted"), home.join("amplifier-bundle/tools"))
        .expect("symlink tools dir");

    let out = run_probe_bare(&fx, &[("AMPLIHACK_HOME", home.as_os_str())]);
    assert!(
        !marker.exists(),
        "a tools dir resolving into the checkout must be skipped\n{}",
        out.describe()
    );
    assert_failed_invalid_input(&fx, &out, "symlinked tools dir");
}

#[test]
fn probe_never_sources_a_helper_file_symlinked_into_the_target_checkout() {
    // The candidate directory is outside the checkout, but the helper file in
    // it is a symlink to a file inside. The physical file is what is sourced,
    // so the check must apply to it, not only to the directory.
    let fx = Fixture::with_origin("https://github.com/o/r.git");
    let marker = plant_helper(&fx, &fx.repo.join("planted/workflow_github_repo.sh"));
    let home = fx.path("fake-amplihack-home");
    let tools = home.join("amplifier-bundle/tools");
    fs::create_dir_all(&tools).expect("mkdir fake tools");
    std::os::unix::fs::symlink(
        fx.repo.join("planted/workflow_github_repo.sh"),
        tools.join("workflow_github_repo.sh"),
    )
    .expect("symlink helper file");

    let out = run_probe_bare(&fx, &[("AMPLIHACK_HOME", home.as_os_str())]);
    assert!(
        !marker.exists(),
        "a helper file resolving into the checkout must be skipped\n{}",
        out.describe()
    );
    assert_failed_invalid_input(&fx, &out, "symlinked helper file");
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
