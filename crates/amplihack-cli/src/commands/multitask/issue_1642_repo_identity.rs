//! Issue #1642: `populate_workstream_scope` stores the `owner/repo` that
//! `amplihack_workflows::remote_repository::github_repository_from_remote_url`
//! derives, or the URL with its userinfo redacted when that parser rejects it.
//!
//! The parsing rules themselves are tested against the shared case table in
//! `crates/amplihack-workflows/tests/issue_1642_shared_cases.rs`; these tests
//! cover only the multitask wiring and the fallback.

use super::command_builder::populate_workstream_scope;
use super::command_builder_tests::workstream;

fn repository_for(url: &str) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut ws = workstream(dir.path());
    populate_workstream_scope(&mut ws, url, "main");
    ws.workstream_scope.repository
}

fn is_owner_repo_shape(value: &str) -> bool {
    let Some((owner, repo)) = value.split_once('/') else {
        return false;
    };
    let ok = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    };
    ok(owner) && ok(repo)
}

#[test]
fn github_origins_are_stored_as_owner_repo() {
    for url in [
        "https://github.com/o/r.git",
        "git@github.com:o/r.git",
        "ssh://git@github.com/o/r",
        "https://x:tok@github.com/o/r",
        "https://github.com/o/r/pull/5",
    ] {
        assert_eq!(repository_for(url), "o/r", "{url}");
    }
}

#[test]
fn hostile_origins_are_not_stored_as_owner_repo() {
    for url in [
        "https://evil.example/x@github.com/a/b",
        r"https://evil.example\@github.com/o/r",
        "https://a@github.com/x@github.com/o/r",
        "https://github.com/o/r/../../x/y",
        "https://github.com.evil.example/o/r",
    ] {
        let repository = repository_for(url);
        assert!(
            !is_owner_repo_shape(&repository),
            "{url:?} stored as {repository:?}"
        );
    }
}

#[test]
fn rejected_urls_are_stored_without_their_userinfo() {
    for url in [
        "https://user:tok-1642@gitlab.example/o/r.git",
        "https://tok-1642@evil.example/o/r",
        "https://user:tok-1642@evil.example/x@github.com/a/b",
    ] {
        let repository = repository_for(url);
        assert!(
            !repository.contains("tok-1642"),
            "{url:?} -> {repository:?}"
        );
        assert!(
            repository.contains("example"),
            "the host stays so scopes remain distinguishable: {repository:?}"
        );
    }
}

#[test]
fn local_path_is_stored_unchanged() {
    let dir = tempfile::tempdir().expect("tempdir");
    let local = dir.path().join("checkout").display().to_string();
    assert_eq!(repository_for(&local), local);
}
