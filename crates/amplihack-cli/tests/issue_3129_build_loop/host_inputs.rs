//! H1 (#3102, AC10): git-ignored `.venv` and `.venv-*` folders of the launch
//! checkout are linked into a new worktree, named in the log, and never
//! committed. The hook that calls `.venv/bin/python` passes in the worktree.

#![cfg(target_os = "linux")]

use super::tools::{World, git, text};
use crate::support::exec_stub::write_executable;
use std::fs;
use std::path::{Path, PathBuf};

fn launch_with_venv(w: &World) {
    fs::write(
        w.repo.join(".gitignore"),
        ".venv/\n.venv-*/\nnode_modules/\n",
    )
    .unwrap();
    git(&w.repo, &["add", "."]);
    git(&w.repo, &["commit", "-qm", "ignore"]);
    fs::create_dir_all(w.repo.join(".venv/bin")).unwrap();
    write_executable(
        &w.repo.join(".venv/bin/python"),
        "#!/bin/sh\nexit 0\n",
        0o755,
    );
}

fn worktree(w: &World) -> PathBuf {
    let wt = w.base.join("wt");
    git(
        &w.repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/x",
            wt.to_str().unwrap(),
        ],
    );
    wt
}

fn link(w: &World, wt: &Path) -> std::process::Output {
    w.tool("workflow_worktree_host_inputs.sh", &w.repo)
        .arg("link")
        .arg(&w.repo)
        .arg(wt)
        .output()
        .unwrap()
}

#[test]
fn an_ignored_venv_is_linked_named_in_the_log_and_not_staged() {
    let w = World::new();
    launch_with_venv(&w);
    let wt = worktree(&w);
    let out = link(&w, &wt);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(
        fs::symlink_metadata(wt.join(".venv"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_link(wt.join(".venv")).unwrap(),
        w.repo.join(".venv")
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains(".venv"),
        "{}",
        text(&out)
    );
    assert!(out.stdout.is_empty(), "{}", text(&out));
    assert_eq!(
        git(&wt, &["status", "--porcelain"]),
        "",
        "a link must be ignored by git"
    );
}

#[test]
fn a_venv_dash_name_is_linked_too() {
    let w = World::new();
    launch_with_venv(&w);
    fs::create_dir_all(w.repo.join(".venv-docs")).unwrap();
    let wt = worktree(&w);
    link(&w, &wt);
    assert!(fs::symlink_metadata(wt.join(".venv-docs")).is_ok());
}

#[test]
fn other_ignored_folders_are_not_linked() {
    let w = World::new();
    launch_with_venv(&w);
    fs::create_dir_all(w.repo.join("node_modules")).unwrap();
    let wt = worktree(&w);
    link(&w, &wt);
    assert!(fs::symlink_metadata(wt.join("node_modules")).is_err());
}

#[test]
fn a_venv_that_git_does_not_ignore_is_not_linked() {
    let w = World::new();
    fs::create_dir_all(w.repo.join(".venv")).unwrap();
    let wt = worktree(&w);
    link(&w, &wt);
    assert!(fs::symlink_metadata(wt.join(".venv")).is_err());
}

#[test]
fn a_venv_already_in_the_worktree_is_left_alone() {
    let w = World::new();
    launch_with_venv(&w);
    let wt = worktree(&w);
    fs::create_dir_all(wt.join(".venv")).unwrap();
    fs::write(wt.join(".venv/own"), "mine").unwrap();
    let out = link(&w, &wt);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(
        !fs::symlink_metadata(wt.join(".venv"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_to_string(wt.join(".venv/own")).unwrap(), "mine");
}

#[test]
fn running_it_twice_changes_nothing_more() {
    let w = World::new();
    launch_with_venv(&w);
    let wt = worktree(&w);
    link(&w, &wt);
    let out = link(&w, &wt);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert_eq!(git(&wt, &["status", "--porcelain"]), "");
}

#[test]
fn a_symlinked_venv_that_points_outside_needs_a_pyvenv_cfg() {
    let w = World::new();
    launch_with_venv(&w);
    fs::remove_dir_all(w.repo.join(".venv")).unwrap();
    let elsewhere = w.base.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, w.repo.join(".venv")).unwrap();
    let wt = worktree(&w);
    link(&w, &wt);
    assert!(
        fs::symlink_metadata(wt.join(".venv")).is_err(),
        "no pyvenv.cfg, so no link"
    );
    fs::write(elsewhere.join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();
    link(&w, &wt);
    assert!(fs::symlink_metadata(wt.join(".venv")).is_ok());
}

#[test]
fn the_repositorys_hook_that_calls_the_venv_python_passes_in_the_worktree() {
    let w = World::new();
    launch_with_venv(&w);
    write_executable(
        &w.repo.join(".git/hooks/pre-commit"),
        "#!/bin/sh\n.venv/bin/python\n",
        0o755,
    );
    let wt = worktree(&w);
    fs::write(wt.join("b.txt"), "b\n").unwrap();
    git(&wt, &["add", "b.txt"]);
    let before = amplihack_git::command_in(&wt)
        .args([
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@example.invalid",
            "commit",
            "-qm",
            "b",
        ])
        .output()
        .unwrap();
    assert!(
        !before.status.success(),
        "the hook should fail without .venv"
    );
    link(&w, &wt);
    git(&wt, &["commit", "-qm", "b"]);
    assert_eq!(
        git(&wt, &["show", "--name-only", "--format=", "HEAD"]),
        "b.txt"
    );
}

#[test]
fn facts_names_what_is_missing() {
    let w = World::new();
    launch_with_venv(&w);
    let wt = worktree(&w);
    let out = w
        .tool("workflow_worktree_host_inputs.sh", &w.repo)
        .arg("facts")
        .arg(&w.repo)
        .arg(&wt)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["missing"], serde_json::json!([".venv"]));
    assert_eq!(v["worktree"][0]["kind"], "missing");
}

#[test]
fn a_usage_error_exits_two() {
    let w = World::new();
    let out = w
        .tool("workflow_worktree_host_inputs.sh", &w.repo)
        .arg("link")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
}
