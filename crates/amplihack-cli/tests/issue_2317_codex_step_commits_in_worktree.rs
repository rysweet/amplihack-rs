//! Issue #2317: a Codex agent step runs with the same trust as a Claude
//! step, `--sandbox danger-full-access`, so a step started in a linked git
//! worktree can commit. Under the old `workspace-write` sandbox the step
//! could not write the shared `.git` directory of the main checkout, which
//! lies outside the worktree.
//!
//! The launch is the real `amplihack codex -- exec --output-last-message F -`
//! with its working directory in a linked worktree. The Codex is the fake
//! from the #1985 test, extended with a step that writes one file, adds it
//! and commits it. The fake enforces no sandbox, so this test proves the argv
//! contract and the linked-worktree commit path; it does not prove how a real
//! Codex enforces its sandbox. `HOME`, `CODEX_HOME` and `AMPLIHACK_HOME` are
//! isolated in a temp dir, and `PATH` holds only the test's tools.
#![cfg(unix)]

use std::fs;
use std::path::Path;

use crate::issue_1985_codex_agent_step_flags::launch_in;
use crate::support::quick_change_repo::git;

const MARKER_FILE: &str = "step-2317.txt";
const MARKER_BODY: &str = "written by the codex step\n";
const MARKER_SUBJECT: &str = "codex step 2317 marker";

/// Shell the fake Codex runs after recording its argv, in the launch's
/// working directory. Records the directory it ran in, then adds and commits
/// only the marker file. Explicit identity, no signing and no hooks, so the
/// user's or the system's git configuration is never consulted.
fn commit_step() -> String {
    format!(
        r#"pwd -P > "$HOME/step-cwd.txt"
printf '{body}' > {file}
git -c user.name=step -c user.email=step@example.invalid \
    -c commit.gpgsign=false -c core.hooksPath= add -- {file} \
  || exit 41
git -c user.name=step -c user.email=step@example.invalid \
    -c commit.gpgsign=false -c core.hooksPath= \
    commit -q -m '{subject}' -- {file} \
  || exit 42
"#,
        body = MARKER_BODY.replace('\n', "\\n"),
        file = MARKER_FILE,
        subject = MARKER_SUBJECT,
    )
}

/// A main repository with one commit and a linked worktree on `step-branch`.
fn repo_with_worktree(root: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let main = root.join("main");
    let wt = root.join("wt");
    fs::create_dir(&main).unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    fs::write(main.join("README.md"), "base\n").unwrap();
    git(&main, &["add", "README.md"]);
    git(&main, &["commit", "-q", "-m", "base"]);
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "step-branch",
            wt.to_str().unwrap(),
        ],
    );
    assert!(
        wt.join(".git").is_file(),
        "a linked worktree has a .git file, not a directory"
    );
    (main, wt)
}

#[test]
fn codex_step_in_linked_worktree_commits() {
    let tmp = tempfile::tempdir().unwrap();
    let (main, wt) = repo_with_worktree(tmp.path());
    let final_message = tmp.path().join("final-message");
    let final_path = final_message.to_str().unwrap();

    let launch = launch_in(
        &["exec", "--output-last-message", final_path, "-"],
        b"commit the marker\n",
        Some(&wt),
        &commit_step(),
    );
    assert!(
        launch.out.status.success(),
        "launch failed (exit 41: git add, 42: git commit)\nstatus: {}\nstderr: {}",
        launch.out.status,
        String::from_utf8_lossy(&launch.out.stderr)
    );

    // The step ran in the worktree: amplihack did not change directory.
    let step_cwd = fs::read_to_string(launch.home.path().join("step-cwd.txt"))
        .expect("the fake codex ran its step");
    assert_eq!(
        Path::new(step_cwd.trim()),
        wt.canonicalize().unwrap(),
        "the step must run in the worktree"
    );

    // The commit is visible from the main checkout on the step branch.
    let log = git(&main, &["log", "--format=%s", "step-branch"]);
    assert_eq!(
        log.lines().next(),
        Some(MARKER_SUBJECT),
        "step-branch log:\n{log}"
    );
    assert_eq!(
        git(&main, &["show", &format!("step-branch:{MARKER_FILE}")]),
        MARKER_BODY.trim_end(),
    );
    // The main branch is untouched.
    assert_eq!(git(&main, &["log", "--format=%s", "main"]), "base");

    // The launch argv carries full access once, hook trust once, and no
    // workspace-write sandbox in any form.
    let argv = &launch.argv;
    assert!(
        argv.windows(2)
            .any(|pair| pair[0] == "--sandbox" && pair[1] == "danger-full-access"),
        "--sandbox danger-full-access missing: {argv:?}"
    );
    assert_eq!(
        argv.iter().filter(|a| *a == "--sandbox").count(),
        1,
        "{argv:?}"
    );
    assert_eq!(
        argv.iter()
            .filter(|a| *a == "--dangerously-bypass-hook-trust")
            .count(),
        1,
        "{argv:?}"
    );
    assert!(
        !argv.iter().any(|a| a.contains("workspace-write")),
        "workspace-write in launch argv: {argv:?}"
    );
    assert!(
        !argv
            .iter()
            .any(|a| a == "--dangerously-bypass-approvals-and-sandbox" || a == "--yolo"),
        "approvals bypass added: {argv:?}"
    );
}
