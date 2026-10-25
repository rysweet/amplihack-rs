//! Issue #1642: the remote repository classifier must never label a URL that
//! makes git contact another host as a GitHub repository, and redaction must
//! never print a byte it hid before #1642.
//!
//! Contract (docs/reference/workflow-github-repo-identity.md, "Remote
//! repository classifier"):
//!
//! - the parse authority ends at the first `/`, `?`, `#` or `\`;
//! - userinfo is the text before the FIRST `@`; if present it must be
//!   non-empty and use only `[A-Za-z0-9._~%!$&'()*+,;=:-]`, otherwise the
//!   provider is `Manual`;
//! - scp form: a host part containing `\` or starting with `-` is never GitHub;
//! - GitHub paths strip one trailing `/` and one `.git`, with no trim, and
//!   owner/name are checked afterwards; failure gives `("unknown", fallback)`;
//! - host matching is unchanged (`github.com` and subdomains, any case, any
//!   port): the one documented place Rust accepts what bash rejects, always
//!   with the owner the URL names (SR6).
//!
//! Every token here is fake.

use amplihack_workflows::remote_repository::{
    provider_from_remote_url, redact_remote_url, repository_identity_from_remote_url,
};
use amplihack_workflows::workflow_contract::RepositoryProvider;

const FALLBACK: &str = "fallback-name";

/// The four URLs from issue #1642 plus the userinfo smuggles. git contacts a
/// host other than github.com (or the userinfo is malformed) for each.
const HOSTILE: &[&str] = &[
    "https://evil.example/x@github.com/a/b",
    "https://evil.example#@github.com/o/r",
    "https://evil.example?@github.com/o/r",
    r"https://evil.example\@github.com/o/r",
    r"https://u:p@evil.example\@github.com/o/r",
    r"https://evil.example\tok@github.com/o/r",
    "https://a@b@github.com/o/r",
    "https://@github.com/o/r",
    "https://ev il@github.com/o/r",
];

/// The real forms from the issue: each must keep classifying as GitHub `o/r`.
const REAL_FORMS: &[&str] = &[
    "https://github.com/o/r",
    "https://github.com/o/r.git",
    "http://github.com/o/r",
    "http://github.com/o/r.git",
    "git@github.com:o/r",
    "git@github.com:o/r.git",
    "ssh://git@github.com/o/r",
    "ssh://git@github.com/o/r.git",
    "https://tok@github.com/o/r",
    "https://x:tok@github.com/o/r",
    "https://x-access-token:ghp_FAKE1642@github.com/o/r.git",
    "https://tok%40x@github.com/o/r",
];

fn classify(url: &str) -> (RepositoryProvider, String, String) {
    let (provider, id) = repository_identity_from_remote_url(Some(url), FALLBACK);
    assert_eq!(
        provider,
        provider_from_remote_url(Some(url)),
        "provider_from_remote_url and repository_identity_from_remote_url disagree for {url:?}"
    );
    (provider, id.owner, id.name)
}

fn assert_manual_unknown(url: &str) {
    let (provider, owner, name) = classify(url);
    assert_eq!(
        provider,
        RepositoryProvider::Manual,
        "{url:?} must be Manual"
    );
    assert_eq!(owner, "unknown", "{url:?} owner");
    assert_eq!(name, FALLBACK, "{url:?} name");
}

fn assert_github_unknown(url: &str) {
    let (provider, owner, name) = classify(url);
    assert_eq!(provider, RepositoryProvider::GitHub, "{url:?} provider");
    assert_eq!(
        (owner.as_str(), name.as_str()),
        ("unknown", FALLBACK),
        "{url:?} must fall back"
    );
}

#[test]
fn hostile_origins_are_manual_with_unknown_owner() {
    for url in HOSTILE {
        assert_manual_unknown(url);
    }
}

#[test]
fn backslash_origin_from_the_issue_is_not_github() {
    // The concrete hole #1642 reported for this classifier.
    assert_ne!(
        provider_from_remote_url(Some(r"https://evil.example\@github.com/o/r")),
        RepositoryProvider::GitHub
    );
}

#[test]
fn every_real_form_is_github_o_r() {
    for url in REAL_FORMS {
        let (provider, owner, name) = classify(url);
        assert_eq!(provider, RepositoryProvider::GitHub, "{url:?} provider");
        assert_eq!((owner.as_str(), name.as_str()), ("o", "r"), "{url:?}");
    }
}

#[test]
fn second_at_in_the_path_never_becomes_the_owner() {
    // Authority is `a@github.com`; owner `x@github.com` fails the segment check.
    let url = "https://a@github.com/x@github.com/o/r";
    assert_github_unknown(url);
    let (_, owner, _) = classify(url);
    assert_ne!(owner, "x@github.com");
}

#[test]
fn host_matching_rows_bash_rejects_keep_the_named_owner() {
    // SR6: Rust may accept what bash rejects only with the same owner/repo.
    for url in [
        "https://www.github.com/o/r",
        "https://GitHub.com/o/r",
        "https://github.com:443/o/r",
    ] {
        let (provider, owner, name) = classify(url);
        assert_eq!(provider, RepositoryProvider::GitHub, "{url:?} provider");
        assert_eq!((owner.as_str(), name.as_str()), ("o", "r"), "{url:?}");
    }
}

#[test]
fn identity_segments_are_checked_after_git_stripping() {
    // `..git` normalises to `.`; `.git.git` strips once, to `.git`, as in bash.
    assert_github_unknown("https://github.com/o/..git");
    let (_, owner, name) = classify("https://github.com/o/.git.git");
    assert_eq!((owner.as_str(), name.as_str()), ("o", ".git"));
}

#[test]
fn invalid_owner_segments_fall_back() {
    for url in [
        "https://github.com/-o/r",
        "https://github.com/../r",
        "https://github.com/o$x/r",
        "git@github.com:-o/r",
    ] {
        assert_github_unknown(url);
    }
}

#[test]
fn paths_past_owner_repo_fall_back_unless_they_are_a_pr() {
    // Path shape: only `owner/repo` or `owner/repo/pull/<n>`, with `<n>`
    // matching ^[1-9][0-9]{0,9}$, names a repository; anything else could
    // resolve to another repository once git removes dot segments.
    for url in [
        "https://github.com/o/r/extra",
        "https://github.com/o/r/tree/main",
        "https://github.com/o/r/../../x/y",
        "https://github.com/o/r/pull/5/../../../x/y",
        "https://github.com/o/r/pull/5/files",
        "https://github.com/o/r/pull/0",
        "https://github.com/o/r/pull/05",
        "https://github.com/o/r/pull/12345678901",
        "https://github.com/o/r/issues/5",
        "https://github.com/o/r.git/pull/5",
        "git@github.com:o/r/extra",
        "ssh://git@github.com/o/r/../../x/y",
        "https://tok@github.com/o/r/../../x/y",
    ] {
        assert_github_unknown(url);
    }
}

#[test]
fn pr_urls_keep_classifying_as_github_o_r() {
    for url in [
        "https://github.com/o/r/pull/5",
        "https://github.com/o/r/pull/5/",
        "https://github.com/o/r/pull/1234567890",
        "https://tok@github.com/o/r/pull/5",
        "git@github.com:o/r/pull/5",
    ] {
        let (provider, owner, name) = classify(url);
        assert_eq!(provider, RepositoryProvider::GitHub, "{url:?} provider");
        assert_eq!((owner.as_str(), name.as_str()), ("o", "r"), "{url:?}");
    }
}

#[test]
fn scp_forms_with_backslash_or_leading_dash_are_never_github() {
    for url in [r"evil.example\@github.com:o/r", "-oProxyCommand=x:o/r"] {
        assert_ne!(
            provider_from_remote_url(Some(url)),
            RepositoryProvider::GitHub,
            "{url:?}"
        );
        let (_, id) = repository_identity_from_remote_url(Some(url), FALLBACK);
        assert_eq!(id.owner, "unknown", "{url:?}");
    }
}

#[test]
fn azure_devops_with_plain_userinfo_still_classifies() {
    let url = "https://org@dev.azure.com/org/proj/_git/repo";
    let (provider, owner, name) = classify(url);
    assert_eq!(provider, RepositoryProvider::AzureDevOps);
    assert_eq!((owner.as_str(), name.as_str()), ("org/proj", "repo"));
}

#[test]
fn redaction_table_gives_exact_outputs() {
    let rows = [
        (
            "https://x:tok@github.com/o/r.git",
            "https://[redacted]@github.com/o/r.git",
        ),
        (
            "https://a@b@github.com/o/r",
            "https://[redacted]@github.com/o/r",
        ),
        (
            r"https://evil.example\@github.com/o/r",
            "https://[redacted]/o/r",
        ),
        (
            r"https://u:p@evil.example\@github.com/o/r",
            "https://[redacted]/o/r",
        ),
        (
            r"https://evil.example\tok@github.com/o/r",
            "https://[redacted]/o/r",
        ),
        // No userinfo: unchanged.
        ("https://github.com/o/r", "https://github.com/o/r"),
        ("git@github.com:o/r.git", "git@github.com:o/r.git"),
    ];
    for (input, want) in rows {
        assert_eq!(redact_remote_url(input), want, "redact {input:?}");
        let (_, id) = repository_identity_from_remote_url(Some(input), FALLBACK);
        assert_eq!(id.remote_url.as_deref(), Some(want), "remote_url {input:?}");
    }
}

#[test]
fn redaction_never_prints_hidden_userinfo() {
    let rows: &[(&str, &[&str])] = &[
        (r"https://u:p@evil.example\@github.com/o/r", &["u:p"]),
        (
            r"https://evil.example\tok@github.com/o/r",
            &["tok", "evil.example"],
        ),
        ("https://a@b@github.com/o/r", &["a@b", "a@"]),
        ("https://x:tok@github.com/o/r.git", &["tok", "x:"]),
    ];
    for (input, secrets) in rows {
        let out = redact_remote_url(input);
        for secret in *secrets {
            assert!(
                !out.contains(secret),
                "{input:?} -> {out:?} leaks {secret:?}"
            );
        }
        assert!(
            !out.starts_with("https://[redacted]@github.com") || !input.contains('\\'),
            "a backslash authority must be hidden whole: {input:?} -> {out:?}"
        );
    }
}
