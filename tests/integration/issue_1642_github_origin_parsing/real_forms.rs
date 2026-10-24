//! Group (c): every real origin form still resolves to `o/r` through every
//! caller, and gh is addressed with exactly `--repo o/r`.

use super::callers::{self, Caller};
use super::fixture::{Fixture, Outcome, REAL_ORIGINS};

fn resolved(caller: Caller, fx: &Fixture, outcome: &Outcome) -> bool {
    let calls = fx.all_gh_calls();
    match caller {
        Caller::TerminalEvidence => callers::evidence_repo(outcome) == "o/r",
        Caller::Collect => callers::github_remote(outcome) == "true",
        Caller::FinalStatusBranchLookup => calls.contains("pr list --repo o/r --head feat/x"),
        _ => calls.contains("--repo o/r "),
    }
}

#[test]
fn every_caller_resolves_every_real_origin_to_o_r() {
    let mut failures = Vec::new();
    for caller in callers::ALL {
        for origin in REAL_ORIGINS {
            let fx = Fixture::with_origin(origin);
            let outcome = callers::run(&fx, *caller);
            if !resolved(*caller, &fx, &outcome) {
                failures.push(format!(
                    "{caller:?} with origin {origin:?} did not resolve to o/r; gh calls:\n{}\n{}",
                    fx.all_gh_calls(),
                    outcome.describe()
                ));
            }
            let stray: Vec<String> = fx
                .repo_gh_calls()
                .into_iter()
                .filter(|call| call.contains("--repo ") && !call.contains("--repo o/r "))
                .collect();
            if !stray.is_empty() {
                failures.push(format!(
                    "{caller:?} with {origin:?} addressed another repo: {stray:?}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn collect_still_treats_real_pr_urls_as_github() {
    for pr_url in [
        "https://github.com/o/r/pull/5",
        "https://tok@github.com/o/r/pull/5",
        "https://x:tok@github.com/o/r/pull/5",
    ] {
        let fx = Fixture::with_origin("/srv/git/not-github/repo.git");
        let outcome = callers::run_collect(&fx, Some(pr_url));
        assert_eq!(
            callers::github_remote(&outcome),
            "true",
            "{pr_url} is a github.com PR URL\n{}",
            outcome.describe()
        );
    }
}
