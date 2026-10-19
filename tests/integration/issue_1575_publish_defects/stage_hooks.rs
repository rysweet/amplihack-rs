//! Defect 2 (generated paths): `.github/hooks/` stubs are unstaged with a
//! WARNING per path, and untracked ones are added to `info/exclude` so later
//! `git add -A` steps do not sweep them in.

use std::fs;

use super::stage::{assert_ok, git_path, repo, stage, staged, touch_source};
use super::{Sandbox, assert_line, write_file};

// --- generated paths -------------------------------------------------------

#[test]
fn untracked_hook_stubs_are_unstaged_and_excluded_once() {
    let sb = Sandbox::new();
    let repo = repo(&sb);
    touch_source(&repo);
    write_file(&repo.join(".github/hooks/pre-commit"), "#!/bin/sh\n");
    let stderr = assert_ok(&stage(&sb, &repo, &[]));
    assert_eq!(staged(&sb, &repo), vec!["src/lib.rs"]);
    assert_line(
        &stderr,
        "WARNING: step-15 unstaged generated path .github/hooks/pre-commit (added to info/exclude)",
        "untracked hook",
    );
    let exclude = git_path(&sb, &repo, "info/exclude");
    let content = fs::read_to_string(&exclude).expect("info/exclude");
    assert!(
        content.contains("# amplihack #1575: generated paths"),
        "{content}"
    );
    assert_eq!(
        content
            .lines()
            .filter(|l| *l == "/.github/hooks/pre-commit")
            .count(),
        1
    );

    // Second run: no duplicate line, and git now ignores the stub.
    sb.git(&repo, &["reset", "-q"]);
    assert_ok(&stage(&sb, &repo, &[]));
    let content = fs::read_to_string(&exclude).expect("info/exclude");
    assert_eq!(
        content
            .lines()
            .filter(|l| *l == "/.github/hooks/pre-commit")
            .count(),
        1
    );
    assert!(
        !sb.git(&repo, &["status", "--porcelain"])
            .contains(".github/hooks")
    );
}

#[test]
fn tracked_hook_changes_are_unstaged_and_left_in_the_worktree() {
    let sb = Sandbox::new();
    let repo = repo(&sb);
    sb.commit_file(
        &repo,
        ".github/hooks/post-checkout",
        "old\n",
        "tracked hook",
    );
    write_file(&repo.join(".github/hooks/post-checkout"), "new\n");
    touch_source(&repo);
    let stderr = assert_ok(&stage(&sb, &repo, &[]));
    assert_eq!(staged(&sb, &repo), vec!["src/lib.rs"]);
    assert_line(
        &stderr,
        "WARNING: step-15 unstaged generated path .github/hooks/post-checkout (tracked; modification left in the working tree)",
        "tracked hook",
    );
    assert_eq!(
        fs::read_to_string(repo.join(".github/hooks/post-checkout")).unwrap(),
        "new\n"
    );
    let exclude = fs::read_to_string(git_path(&sb, &repo, "info/exclude")).unwrap_or_default();
    assert!(!exclude.contains("post-checkout"), "{exclude}");
}

#[test]
fn hook_paths_with_unsupported_characters_are_unstaged_but_not_excluded() {
    let sb = Sandbox::new();
    let repo = repo(&sb);
    for name in ["!x", "a b"] {
        write_file(&repo.join(".github/hooks").join(name), "x\n");
    }
    touch_source(&repo);
    let stderr = assert_ok(&stage(&sb, &repo, &[]));
    assert_eq!(staged(&sb, &repo), vec!["src/lib.rs"]);
    for name in ["!x", "a b"] {
        assert_line(
            &stderr,
            &format!(
                "WARNING: step-15 unstaged generated path .github/hooks/{name} (not added to info/exclude: path contains unsupported characters)"
            ),
            "unsupported path",
        );
    }
    let exclude = fs::read_to_string(git_path(&sb, &repo, "info/exclude")).unwrap_or_default();
    assert!(
        !exclude.contains("!x") && !exclude.contains("a b"),
        "{exclude}"
    );
}
