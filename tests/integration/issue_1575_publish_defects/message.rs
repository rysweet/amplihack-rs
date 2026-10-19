//! Defect 2 (message): `workflow_commit_message.sh` builds the step-15 subject
//! and body from the staged index, never from the task description.

use std::path::{Path, PathBuf};
use std::process::Output;

use super::{Sandbox, dump, text, write_file};

const MSG: &str = "workflow_commit_message.sh";

fn repo(sb: &Sandbox) -> PathBuf {
    let repo = sb.root().join("repo");
    sb.git(
        sb.root(),
        &["init", "-q", "-b", "main", repo.to_str().unwrap()],
    );
    sb.commit_file(&repo, "README.md", "base\n", "base");
    repo
}

fn stage_files(sb: &Sandbox, repo: &Path, files: &[&str]) {
    for file in files {
        write_file(&repo.join(file), "changed\n");
        sb.git(repo, &["add", "--", file]);
    }
}

fn msg(sb: &Sandbox, dir: &Path, args: &[&str]) -> Output {
    sb.run_helper(
        dir,
        MSG,
        args,
        &[(
            "TASK_DESCRIPTION",
            "SECRET-PROMPT Continue fixing issue #130",
        )],
    )
}

fn subject(sb: &Sandbox, dir: &Path, prefix: &str) -> String {
    let output = msg(sb, dir, &["subject", prefix]);
    assert_eq!(output.status.code(), Some(0), "{}", dump(&output));
    text(&output.stdout).trim_end_matches('\n').to_string()
}

#[test]
fn single_file_subject_names_the_path() {
    let sb = Sandbox::new();
    let repo = repo(&sb);
    stage_files(&sb, &repo, &["src/lib.rs"]);
    assert_eq!(subject(&sb, &repo, "fix: "), "fix: update src/lib.rs");
}

#[test]
fn multi_file_subject_counts_files_and_lists_top_level_dirs() {
    let sb = Sandbox::new();
    let repo = repo(&sb);
    stage_files(&sb, &repo, &["README.md", "docs/a.md", "src/b.rs"]);
    assert_eq!(
        subject(&sb, &repo, "feat: "),
        "feat: update 3 files in ., docs, src"
    );
}

#[test]
fn more_than_three_dirs_are_elided() {
    let sb = Sandbox::new();
    let repo = repo(&sb);
    stage_files(&sb, &repo, &["a/1", "b/1", "c/1", "d/1", "d/2"]);
    assert_eq!(
        subject(&sb, &repo, "feat: "),
        "feat: update 5 files in a, b, c, …"
    );
}

#[test]
fn subject_is_cut_to_72_characters_not_bytes() {
    let sb = Sandbox::new();
    let repo = repo(&sb);
    let long = format!("dir/{}é.rs", "x".repeat(120));
    stage_files(&sb, &repo, &[&long]);
    let s = subject(&sb, &repo, "fix: ");
    assert!(s.starts_with("fix: update dir/xxx"), "{s}");
    assert_eq!(s.chars().count(), 72, "{s}");
}

#[test]
fn subject_has_no_control_characters() {
    let sb = Sandbox::new();
    let repo = repo(&sb);
    stage_files(&sb, &repo, &["we\tird\u{1b}[31m.txt"]);
    let s = subject(&sb, &repo, "fix: ");
    assert!(s.starts_with("fix: update "), "{s}");
    assert!(!s.chars().any(char::is_control), "{s:?}");
}

#[test]
fn empty_index_and_non_repo_print_nothing_and_exit_0() {
    let sb = Sandbox::new();
    let repo = repo(&sb);
    assert_eq!(subject(&sb, &repo, "fix: "), "");
    let not_a_repo = sb.root().join("scratch");
    assert_eq!(subject(&sb, &not_a_repo, "fix: "), "");
    let body = msg(&sb, &not_a_repo, &["body"]);
    assert_eq!(body.status.code(), Some(0), "{}", dump(&body));
}

#[test]
fn task_description_never_reaches_subject_or_body() {
    let sb = Sandbox::new();
    let repo = repo(&sb);
    stage_files(&sb, &repo, &["src/lib.rs", "src/main.rs"]);
    let s = subject(&sb, &repo, "fix: ");
    let body = text(&msg(&sb, &repo, &["body"]).stdout);
    for out in [&s, &body] {
        assert!(
            !out.contains("SECRET-PROMPT") && !out.contains("Continue fixing"),
            "{out}"
        );
    }
}

#[test]
fn body_is_the_staged_stat() {
    let sb = Sandbox::new();
    let repo = repo(&sb);
    stage_files(&sb, &repo, &["src/lib.rs", "docs/x.md"]);
    let expected = sb.git(
        &repo,
        &[
            "-c",
            "core.quotePath=true",
            "diff",
            "--cached",
            "--stat=72",
            "--no-color",
            "--no-ext-diff",
        ],
    );
    let output = msg(&sb, &repo, &["body"]);
    assert_eq!(output.status.code(), Some(0), "{}", dump(&output));
    assert_eq!(text(&output.stdout).trim_end_matches('\n'), expected);
}

#[test]
fn long_stat_is_truncated_to_30_lines_with_a_marker() {
    let sb = Sandbox::new();
    let repo = repo(&sb);
    let files: Vec<String> = (0..40).map(|i| format!("many/f{i:02}.txt")).collect();
    let refs: Vec<&str> = files.iter().map(String::as_str).collect();
    stage_files(&sb, &repo, &refs);
    let body = text(&msg(&sb, &repo, &["body"]).stdout);
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(lines.len(), 31, "{body}");
    assert_eq!(lines[30], "… (stat truncated)");
}
