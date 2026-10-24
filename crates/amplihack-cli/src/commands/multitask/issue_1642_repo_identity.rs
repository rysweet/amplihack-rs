//! Issue #1642: the multitask copy of the GitHub origin parser follows the same
//! contract as `amplifier-bundle/tools/workflow_github_repo.sh`.
//!
//! The parser is reached through `populate_workstream_scope`, which stores the
//! parsed `owner/repo` in `WorkstreamScope.repository`, or a fallback when the
//! URL is not a github.com repository. Contract assumed by these tests:
//!
//! - accepted: exact, case-sensitive `https://github.com/`, `http://github.com/`,
//!   `git@github.com:`, `ssh://git@github.com/` and `http(s)://<userinfo>@github.com/`
//!   prefixes. The userinfo is the text before the FIRST `@github.com/`; it must
//!   be non-empty and use only RFC 3986 userinfo characters other than `@`.
//! - normalised in order: cut `?`, cut `#`, strip one trailing `/`, strip one
//!   `.git`. Segments past owner/repo (`/pull/5`) are ignored.
//! - rejected: empty, `.` or `..` segments, any byte outside `[A-Za-z0-9._-]`,
//!   an owner starting with `-`, and surrounding whitespace (no trim).
//! - fallback (S3): a rejected string containing `://` has its userinfo removed
//!   before it is stored; a local path is stored unchanged. A fallback never
//!   looks like a parsed `owner/repo`.

use super::command_builder::populate_workstream_scope;
use super::command_builder_tests::workstream;

fn repository_for(url: &str) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut ws = workstream(dir.path());
    populate_workstream_scope(&mut ws, url, "main");
    ws.workstream_scope.repository
}

const REAL_FORMS: &[&str] = &[
    "https://github.com/o/r",
    "https://github.com/o/r.git",
    "http://github.com/o/r",
    "git@github.com:o/r",
    "git@github.com:o/r.git",
    "ssh://git@github.com/o/r",
    "https://tok@github.com/o/r",
    "https://x:tok@github.com/o/r",
    "http://x:tok@github.com/o/r",
    "https://github.com/o/r/",
    "https://github.com/o/r.git/",
    "https://github.com/o/r?tab=readme",
    "https://github.com/o/r#readme",
    "https://github.com/o/r/pull/5",
    "https://tok@github.com/o/r/pull/5",
];

#[test]
fn real_github_forms_parse_to_owner_repo() {
    for url in REAL_FORMS {
        assert_eq!(repository_for(url), "o/r", "{url} must parse to o/r");
    }
}

#[test]
fn real_names_with_dots_dashes_and_underscores_still_parse() {
    assert_eq!(
        repository_for("https://github.com/my-org/my.project_x.git"),
        "my-org/my.project_x"
    );
}

#[test]
fn dot_git_suffix_is_stripped_exactly_once() {
    assert_eq!(repository_for("https://github.com/o/r.git.git"), "o/r.git");
}

/// Each of these must NOT be read as a github.com repository.
const HOSTILE: &[&str] = &[
    // The four URLs from the issue: git contacts evil.example.
    "https://evil.example/x@github.com/a/b",
    "https://evil.example#@github.com/o/r",
    "https://evil.example?@github.com/o/r",
    r"https://evil.example\@github.com/o/r",
    // Userinfo ends at the FIRST @github.com/; the owner is then x@github.com.
    "https://a@github.com/x@github.com/o/r",
    "https://a@b@github.com/o/r",
    "https://@github.com/o/r",
    "https://ev il@github.com/o/r",
    "https://evil\t@github.com/o/r",
    "https://evil\n@github.com/o/r",
    "https://github.com/-o/r",
    "https://github.com/./r",
    "https://github.com/o/..",
    "https://github.com/o$x/r",
    "https://github.com/\u{f3}wner/r",
    "https://github.com/o",
    "https://github.com//r",
    " https://github.com/o/r",
    "https://github.com/o/r ",
    "HTTPS://github.com/o/r",
    "https://GitHub.com/o/r",
    "https://github.com.evil.example/o/r",
];

#[test]
fn hostile_and_malformed_origins_are_never_a_parsed_identity() {
    for url in HOSTILE {
        let repository = repository_for(url);
        assert!(
            !repository_is_owner_repo_shape(&repository),
            "{url:?} must not be read as a github.com repository; got {repository:?}"
        );
    }
}

fn repository_is_owner_repo_shape(value: &str) -> bool {
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
fn credential_bearing_unparseable_url_is_stored_without_userinfo() {
    let repository = repository_for("https://user:tok-1642@gitlab.example/o/r.git");
    assert!(
        !repository.contains("tok-1642") && !repository.contains("user:"),
        "fallback must strip userinfo before it is saved: {repository:?}"
    );
    assert!(
        repository.contains("gitlab.example"),
        "fallback keeps the host so the scope stays distinguishable: {repository:?}"
    );
}

#[test]
fn local_path_fallback_is_unchanged() {
    let dir = tempfile::tempdir().expect("tempdir");
    let local = dir.path().join("checkout").display().to_string();
    assert_eq!(repository_for(&local), local);
}

// The case table shared with the bash parser's integration tests: a change to
// either parser without a matching row fails one of the two suites.
include!("../../../../../tests/integration/issue_1642_github_origin_parsing/cases.rs");

#[test]
fn shared_case_table_matches_the_rust_parser() {
    let mut failures = Vec::new();
    for (url, want) in ORIGIN_CASES {
        let repository = repository_for(url);
        let ok = match want {
            Some(want) => repository == *want,
            None => !repository_is_owner_repo_shape(&repository),
        };
        if !ok {
            failures.push(format!("{url:?}: want {want:?}, stored {repository:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn hostile_origin_with_a_token_is_stored_without_the_token() {
    for url in [
        "https://tok-1642@evil.example/o/r",
        "https://user:tok-1642@evil.example/x@github.com/a/b",
    ] {
        let repository = repository_for(url);
        assert!(
            !repository.contains("tok-1642"),
            "{url:?}: the fallback must drop userinfo before it is stored; got {repository:?}"
        );
        assert!(
            !repository_is_owner_repo_shape(&repository),
            "{url:?} must not be read as a github.com repository; got {repository:?}"
        );
    }
}
