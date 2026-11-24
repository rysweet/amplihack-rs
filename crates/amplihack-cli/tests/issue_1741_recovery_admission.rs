//! Admission and launch contracts. Assertions select the latest attempt first.
#![cfg(unix)]

use crate::issue_1741_fixture::{Fixture, describe};
use amplihack_cli::Cli;
use serde_json::json;
use std::fs;

#[test]
fn fresh_is_a_launch_flag_and_composes_with_detach_and_dry_run() {
    for flags in [
        vec!["--fresh"],
        vec!["--fresh", "--detach"],
        vec!["--fresh", "--dry-run"],
    ] {
        let args: Vec<_> = ["amplihack", "recipe", "run", "smart-orchestrator"]
            .into_iter()
            .chain(flags)
            .collect();
        assert!(
            Cli::try_parse_from(args).is_ok(),
            "--fresh must be accepted without becoming recipe context"
        );
    }
}

#[test]
fn latest_non_limit_failure_blocks_older_usage_limit_history() {
    let f = Fixture::new();
    f.stop_at("design");
    f.mode("design", "error");
    let middle = f.run(&[]);
    assert!(
        !middle.output.status.success(),
        "{}",
        describe(&middle.output)
    );
    f.mode("design", "success");
    let before = f.events().len();

    let latest = f.run(&[]);
    assert!(
        latest.output.status.success(),
        "{}",
        describe(&latest.output)
    );
    assert_eq!(&f.events()[before..], ["classify", "design", "implement"]);
    latest.assert_fresh("latest_not_usage_limit");
}

#[test]
fn latest_success_blocks_older_usage_limit_history() {
    let f = Fixture::new();
    f.stop_at("design");
    f.mode("design", "success");
    let middle = f.run(&[]);
    assert!(
        middle.output.status.success(),
        "{}",
        describe(&middle.output)
    );
    let before = f.events().len();

    let latest = f.run(&[]);
    assert!(
        latest.output.status.success(),
        "{}",
        describe(&latest.output)
    );
    assert_eq!(&f.events()[before..], ["classify", "design", "implement"]);
    latest.assert_fresh("latest_not_usage_limit");
}

#[test]
fn removing_latest_detail_does_not_resurrect_older_limit_history() {
    let f = Fixture::new();
    f.stop_at("design");
    f.mode("design", "error");
    let middle = f.run(&[]);
    assert!(
        !middle.output.status.success(),
        "{}",
        describe(&middle.output)
    );
    fs::remove_dir_all(middle.dir()).unwrap();
    f.mode("design", "success");
    let before = f.events().len();

    let latest = f.run(&[]);
    assert!(
        latest.output.status.success(),
        "{}",
        describe(&latest.output)
    );
    assert_eq!(&f.events()[before..], ["classify", "design", "implement"]);
    assert_eq!(latest.recovery()["mode"], "fresh");
    assert!(latest.recovery()["source_run_id"].is_null());
}

#[test]
fn issue_and_recipe_identities_do_not_share_completions() {
    let f = Fixture::new();
    f.stop_at("design");
    f.mode("design", "success");
    let other_issue = f.run_at(&f.recipe, &f.repo, "1742", &[]);
    assert!(
        other_issue.output.status.success(),
        "{}",
        describe(&other_issue.output)
    );
    other_issue.assert_fresh("no_history");
    let other_recipe = f.root.join("other.yaml");
    let mut recipe = f.workflow();
    recipe["name"] = json!("different-recipe");
    f.write_recipe(&other_recipe, &recipe);
    let result = f.run_at(&other_recipe, &f.repo, "1741", &[]);
    assert!(
        result.output.status.success(),
        "{}",
        describe(&result.output)
    );
    result.assert_fresh("no_history");
    assert_eq!(
        f.events(),
        [
            "classify",
            "design",
            "classify",
            "design",
            "implement",
            "classify",
            "design",
            "implement"
        ]
    );
}

#[test]
fn independent_clone_with_the_same_remote_does_not_reuse_repository_history() {
    let f = Fixture::new();
    f.stop_at("design");
    let clone = f.root.join("independent-clone");
    f.git(&[
        "clone",
        "--no-local",
        f.repo.to_str().unwrap(),
        clone.to_str().unwrap(),
    ]);
    let out = amplihack_git::command()
        .arg("-C")
        .arg(&clone)
        .args([
            "remote",
            "set-url",
            "origin",
            "https://github.com/example/recovery.git",
        ])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", &f.home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    f.mode("design", "success");

    let result = f.run_at(&f.recipe, &clone, "1741", &[]);
    assert!(
        result.output.status.success(),
        "{}",
        describe(&result.output)
    );
    assert_eq!(
        f.events(),
        ["classify", "design", "classify", "design", "implement"]
    );
    result.assert_fresh("no_history");
}

#[test]
fn structurally_valid_issue_url_and_explicit_number_select_the_same_history() {
    let f = Fixture::new();
    let first = f.stop_at("design");
    f.mode("design", "success");
    // Clear the explicit number through its normal context override. URL
    // normalization supplies identity; it is not parsed out of task prose.
    let second = f.run_at(
        &f.recipe,
        &f.repo,
        "",
        &[
            "-c",
            "issue_url=https://github.com/example/recovery/issues/1741",
        ],
    );
    assert!(
        second.output.status.success(),
        "{}",
        describe(&second.output)
    );
    second.assert_resumed(&first, "design");
    assert_eq!(f.events(), ["classify", "design", "design", "implement"]);
}

#[test]
fn untrusted_or_conflicting_issue_identity_never_reuses_history() {
    for url in [
        "https://github.com.evil.invalid/example/recovery/issues/1741",
        "https://github.com/other/repository/issues/1741",
        "https://github.com/example/recovery/issues/0",
        "https://github.com/example/recovery/issues/1742",
    ] {
        let f = Fixture::new();
        f.stop_at("design");
        f.mode("design", "success");
        let result = f.run(&["-c", &format!("issue_url={url}")]);
        assert!(
            result.output.status.success(),
            "{}",
            describe(&result.output)
        );
        result.assert_fresh("identity_unavailable");
        assert_eq!(
            f.events(),
            ["classify", "design", "classify", "design", "implement"]
        );
    }
}

#[test]
fn prose_issue_number_does_not_establish_work_identity() {
    let f = Fixture::new();
    f.stop_at("design");
    f.mode("design", "success");
    let result = f.run_at(
        &f.recipe,
        &f.repo,
        "",
        &[
            "-c",
            "task_description=please fix https://github.com/example/recovery/issues/1741",
        ],
    );
    assert!(
        result.output.status.success(),
        "{}",
        describe(&result.output)
    );
    result.assert_fresh("identity_unavailable");
    assert_eq!(
        f.events(),
        ["classify", "design", "classify", "design", "implement"]
    );
}

#[test]
fn linked_worktree_and_symlink_alias_find_the_latest_common_repository_attempt() {
    let f = Fixture::new();
    let first = f.run(&[]);
    assert!(first.output.status.success(), "{}", describe(&first.output));
    let linked = f.root.join("linked-worktree");
    f.git(&[
        "worktree",
        "add",
        "--detach",
        linked.to_str().unwrap(),
        "HEAD",
    ]);
    let alias = f.root.join("repo-alias");
    std::os::unix::fs::symlink(&f.repo, &alias).unwrap();

    for repo in [&linked, &alias] {
        let result = f.run_at(&f.recipe, repo, "1741", &[]);
        assert!(
            result.output.status.success(),
            "{}",
            describe(&result.output)
        );
        // Check admission independently of cwd/revision compatibility: a
        // successful latest predecessor rules out replay before those checks.
        // A new namespace would incorrectly report no_history here.
        result.assert_fresh("latest_not_usage_limit");
    }
}

#[test]
fn explicit_repository_context_selects_work_identity_independently_of_launcher_cwd() {
    let f = Fixture::new();
    f.stop_at("design");
    let clone = f.root.join("selected-repository");
    f.git(&[
        "clone",
        "--no-local",
        f.repo.to_str().unwrap(),
        clone.to_str().unwrap(),
    ]);
    f.mode("design", "success");
    let result = f.run(&["-c", &format!("repo_path={}", clone.display())]);
    assert!(
        result.output.status.success(),
        "{}",
        describe(&result.output)
    );
    result.assert_fresh("no_history");
    assert_eq!(
        f.events(),
        ["classify", "design", "classify", "design", "implement"]
    );
}
