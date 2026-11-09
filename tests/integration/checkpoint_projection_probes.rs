//! Edge probes for `Owner::project`, kept as Rust tests so they outlive the
//! Python reference (#1554, PR #2020). Each one was first run against the
//! Python projection at f14da87ed and the Rust projection side by side; the
//! expectations below are the outputs both produced.
//!
//! - an engine that exits 79 at once: the 79 reaches the caller untouched;
//! - an engine that prints bytes that are not UTF-8: they are passed to the
//!   step unchanged, which then fails on them in `cd`;
//! - a task description written to break the templates: it reaches the
//!   commit message as text and runs nothing.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PROBE_TASK: &str = "probe owner";

/// Shell metacharacters, recipe template syntax and non-ASCII text.
const HOSTILE_TASK: &str = "{{worktree_setup.branch_name}} $(touch PWNED) `touch PWNED2` 'q' \"dq\"\nline2 \u{e9} \u{2603} \\ {{nope}} {{ version_bump }}";

fn root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn git(dir: &Path, args: &[&str]) -> Output {
    let out = amplihack_git::command()
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// An origin with `main` and `feature`, and a worktree of `feature` with one
/// uncommitted change to the owned path `feature.txt`.
fn setup(tmp: &Path) -> PathBuf {
    let origin = tmp.join("origin.git");
    let repo = tmp.join("repo");
    git(tmp, &["init", "-q", "--bare", origin.to_str().unwrap()]);
    git(
        tmp,
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            repo.to_str().unwrap(),
        ],
    );
    git(&repo, &["switch", "-q", "-c", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Workflow Test"]);
    fs::write(repo.join("README.md"), "base\n").unwrap();
    git(&repo, &["add", "README.md"]);
    git(
        &repo,
        &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "base"],
    );
    git(&repo, &["push", "-q", "-u", "origin", "main"]);
    git(&repo, &["switch", "-q", "-c", "feature"]);
    fs::write(repo.join("feature.txt"), "f1\n").unwrap();
    git(&repo, &["add", "feature.txt"]);
    git(
        &repo,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "feature 1",
        ],
    );
    git(&repo, &["push", "-q", "-u", "origin", "feature"]);
    git(&repo, &["switch", "-q", "main"]);
    let worktree = tmp.join("worktree");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            worktree.to_str().unwrap(),
            "feature",
        ],
    );
    fs::write(worktree.join("feature.txt"), "f1\nf2\n").unwrap();
    worktree
}

/// Writes `engine` (a bash body) as the `AMPLIHACK_BIN` the step calls.
fn engine(tmp: &Path, body: &str) -> PathBuf {
    let path = tmp.join("engine.sh");
    fs::write(&path, format!("#!/bin/bash\n{body}")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// Projects workflow-publish for `task` with `engine` as `AMPLIHACK_BIN`.
fn publish(worktree: &Path, task: &'static str, engine: &Path) -> Output {
    let owner = crate::canonical_staging_checkpoint_fixture::Owner {
        task,
        paths: &["feature.txt"],
    };
    let mut command = Command::new("bash");
    command
        .env("PATH", crate::amplihack_path::path_with_amplihack_bin())
        .env("AMPLIHACK_HOME", root())
        .env("WORKTREE_SETUP_WORKTREE_PATH", worktree)
        .env(
            "RECIPE_VAR_worktree_setup",
            serde_json::json!({"worktree_path": worktree.to_string_lossy()}).to_string(),
        )
        .env("ISSUE_NUMBER", "978")
        .env("REMOTE_HOST_TYPE", "github")
        .env("AMPLIHACK_BIN", engine);
    owner.project(
        &command,
        "workflow-publish",
        worktree,
        worktree,
        "feature",
        "feature",
    )
}

fn origin_feature(tmp: &Path) -> String {
    let out = git(
        &tmp.join("origin.git"),
        &["rev-parse", "refs/heads/feature"],
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn an_engine_that_exits_79_at_once_reaches_the_caller_as_79() {
    let tmp = tempfile::tempdir().unwrap();
    let worktree = setup(tmp.path());
    let before = origin_feature(tmp.path());
    let engine = engine(tmp.path(), "exit 79\n");

    let out = publish(&worktree, PROBE_TASK, &engine);

    assert_eq!(out.status.code(), Some(79), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
    assert!(out.stderr.is_empty(), "{out:?}");
    assert_eq!(origin_feature(tmp.path()), before, "nothing was pushed");
}

#[test]
fn engine_output_that_is_not_utf8_is_passed_on_byte_for_byte() {
    let tmp = tempfile::tempdir().unwrap();
    let worktree = setup(tmp.path());
    let before = origin_feature(tmp.path());
    let engine = engine(tmp.path(), "printf '\\377\\376{\"a\":1}\\n'\nexit 0\n");

    let out = publish(&worktree, PROBE_TASK, &engine);

    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
    // bash quotes the bytes it could not use as a directory; the projection
    // neither decodes nor replaces them.
    let expected: &[u8] = b": cd: $'\\377\\376{\"a\":1}': No such file or directory\n";
    assert!(
        out.stderr.starts_with(b"bash: line ") && out.stderr.ends_with(expected),
        "stderr: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        out.stderr.iter().filter(|&&b| b == b'\n').count(),
        1,
        "one line of stderr"
    );
    assert_eq!(origin_feature(tmp.path()), before, "nothing was pushed");
}

#[test]
fn a_hostile_task_description_reaches_the_commit_as_text_and_runs_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let worktree = setup(tmp.path());
    let real = env!("CARGO_BIN_EXE_amplihack");
    let engine = engine(tmp.path(), &format!("exec '{real}' \"$@\"\n"));

    let out = publish(&worktree, HOSTILE_TASK, &engine);

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "stderr: {stderr}");
    let last = String::from_utf8_lossy(&out.stdout)
        .lines()
        .last()
        .unwrap_or_default()
        .to_owned();
    let result: serde_json::Value = serde_json::from_str(&last).expect("a JSON result line");
    assert_eq!(result["pushed"], "true", "{last}");
    assert_eq!(result["branch"], "feature", "{last}");

    let pushed = origin_feature(tmp.path());
    assert_eq!(result["sha"], pushed.trim(), "{last}");
    let subject = git(
        &tmp.path().join("origin.git"),
        &["log", "-1", "--format=%s", "refs/heads/feature"],
    );
    assert_eq!(
        String::from_utf8(subject.stdout).unwrap(),
        "fix: {{worktree_setup.branch_name}} $(touch PWNED) `touch PWNED2` 'q' \"dq\" li\n",
        "the task text, unexpanded and cut to the subject length, is the subject"
    );
    for name in ["PWNED", "PWNED2"] {
        let found: Vec<_> = walkdir(tmp.path())
            .into_iter()
            .filter(|p| p.file_name().is_some_and(|n| n == name))
            .collect();
        assert!(found.is_empty(), "the task text ran a command: {found:?}");
    }
}

/// Every path under `dir`, without following symlinks.
fn walkdir(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        let Ok(entries) = fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                pending.push(path.clone());
            }
            found.push(path);
        }
    }
    found
}
