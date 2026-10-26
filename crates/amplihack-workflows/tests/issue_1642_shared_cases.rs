//! Issue #1642: the shared origin table (`cases.rs`) run against the remote
//! repository classifier, so it cannot drift from the bash parser.
//!
//! `Some("o/r")` rows must classify as GitHub `o/r`. `None` rows must either
//! not be GitHub or fall back to `("unknown", fallback)`. The only rows
//! exempt are the host-matching rows of exception SR6 (any case, any
//! subdomain, any port), which may be accepted only with the owner/repo the
//! URL names.

include!("../../../tests/integration/issue_1642_github_origin_parsing/cases.rs");

use amplihack_workflows::remote_repository::{
    github_repository_from_remote_url, repository_identity_from_remote_url,
};
use amplihack_workflows::workflow_contract::RepositoryProvider;

const FALLBACK: &str = "fallback-name";

/// SR6: host matching accepts any case, subdomains and ports. Nothing else
/// is exempt.
const SR6_EXEMPT: &[&str] = &[
    "HTTPS://github.com/o/r",
    "https://GitHub.com/o/r",
    "https://www.github.com/o/r",
    "https://github.com:443/o/r",
    "https://tok@github.com:443/o/r",
    "ssh://git@github.com:22/o/r",
];

/// Both public entry points, which must agree on every row: the classifier
/// and `github_repository_from_remote_url`, used by multitask.
fn classify(url: &str) -> Option<String> {
    let (provider, id) = repository_identity_from_remote_url(Some(url), FALLBACK);
    let classified = (provider == RepositoryProvider::GitHub
        && (id.owner.as_str(), id.name.as_str()) != ("unknown", FALLBACK))
        .then(|| format!("{}/{}", id.owner, id.name));
    assert_eq!(
        github_repository_from_remote_url(url),
        classified,
        "{url:?}: the two entry points disagree"
    );
    classified
}

#[test]
fn every_shared_row_matches_the_bash_parser() {
    let mut failures = Vec::new();
    for (url, want) in ORIGIN_CASES {
        let got = classify(url);
        if SR6_EXEMPT.contains(url) {
            if got.as_deref().is_some_and(|got| got != "o/r") {
                failures.push(format!("SR6 row {url:?} gave {got:?}, not o/r"));
            }
            continue;
        }
        if got.as_deref() != *want {
            failures.push(format!("{url:?}: want {want:?}, got {got:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn every_sr6_exemption_is_a_shared_row() {
    for url in SR6_EXEMPT {
        assert!(
            ORIGIN_CASES.iter().any(|(row, _)| row == url),
            "{url:?} is exempt but not in cases.rs"
        );
    }
}
