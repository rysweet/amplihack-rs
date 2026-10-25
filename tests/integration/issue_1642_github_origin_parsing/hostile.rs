//! Group (b): every caller rejects every hostile origin and never calls gh for
//! a repository. The missing-helper rules live in `missing_helper.rs`.

use super::callers::{self, Caller};
use super::fixture::{Fixture, HOSTILE_ORIGINS, Outcome};

/// The caller-specific "this is not a GitHub repo" signal.
fn rejected(caller: Caller, outcome: &Outcome) -> Result<(), String> {
    let ok = match caller {
        Caller::PrReady => {
            !outcome.success()
                && outcome
                    .stderr
                    .contains("unable to determine current GitHub repo identity")
        }
        // No evidence was supplied, so the run fails as before; what matters
        // is that the branch-PR lookup never ran (checked by the caller).
        Caller::FinalStatusBranchLookup => !outcome.success(),
        Caller::FinalStatusScopedPr => !outcome
            .stdout
            .contains("All 23 workflow steps completed successfully"),
        Caller::PrScope => {
            !outcome.success()
                && outcome
                    .json()
                    .is_some_and(|json| json["reason"] == "missing_scope")
        }
        Caller::PublishPr => !outcome.success(),
        Caller::TerminalEvidence => callers::evidence_repo(outcome).is_empty(),
        Caller::PrSignal => {
            !outcome.success() && outcome.stderr.contains("WARNING: no open-PR signal")
        }
        Caller::Collect => callers::github_remote(outcome) == "false",
    };
    if ok {
        Ok(())
    } else {
        Err("caller did not report the origin as unrecognised".into())
    }
}

fn check(caller: Caller, origin: &str, failures: &mut Vec<String>) {
    let fx = Fixture::with_origin(origin);
    let outcome = callers::run(&fx, caller);
    let mut problems = Vec::new();
    let calls = fx.repo_gh_calls();
    if !calls.is_empty() {
        problems.push(format!("gh was invoked for a repository: {calls:?}"));
    }
    if let Err(problem) = rejected(caller, &outcome) {
        problems.push(problem);
    }
    if outcome.stdout.contains("evil.example") || outcome.stderr.contains("evil.example") {
        problems.push("output echoes the raw origin".into());
    }
    if let Some(leak) = outcome.leaked_token() {
        problems.push(format!("output prints the token ({leak:?})"));
    }
    if !problems.is_empty() {
        failures.push(format!(
            "{caller:?} with origin {origin:?}: {}\n{}",
            problems.join("; "),
            outcome.describe()
        ));
    }
}

#[test]
fn every_caller_rejects_every_hostile_origin_without_calling_gh() {
    let mut failures = Vec::new();
    for caller in callers::ALL {
        for origin in HOSTILE_ORIGINS {
            check(*caller, origin, &mut failures);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn collect_does_not_treat_a_hostile_pr_url_as_a_github_remote() {
    let mut failures = Vec::new();
    for pr_url in [
        "https://evil.example/x@github.com/o/r/pull/5",
        "https://evil.example#@github.com/o/r/pull/5",
        "https://evil.example?@github.com/o/r/pull/5",
        r"https://evil.example\@github.com/o/r/pull/5",
    ] {
        let fx = Fixture::with_origin("/srv/git/not-github/repo.git");
        let outcome = callers::run_collect(&fx, Some(pr_url));
        if callers::github_remote(&outcome) != "false" {
            failures.push(format!("{pr_url:?}\n{}", outcome.describe()));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// Path-shape origins: github.com is the host, but the path is neither
/// `owner/repo` nor `owner/repo/pull/<n>`, so the identity the parser would
/// report can differ from the repository git fetches once dot segments are
/// removed. Every caller must treat them as unrecognised, like a hostile host.
const PATH_SHAPE_ORIGINS: &[&str] = &[
    "https://github.com/o/r/../../x/y",
    "https://github.com/o/r/extra",
    "https://github.com/o/r/tree/main",
    "https://github.com/o/r.git/pull/5",
];

#[test]
fn every_caller_rejects_path_shape_origins_without_calling_gh() {
    let mut failures = Vec::new();
    for caller in callers::ALL {
        for origin in PATH_SHAPE_ORIGINS {
            check(*caller, origin, &mut failures);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn collect_does_not_treat_a_path_shape_pr_url_as_a_github_remote() {
    let mut failures = Vec::new();
    for pr_url in [
        "https://github.com/o/r/pull/5/../../../x/y",
        "https://github.com/o/r/pull/0",
        "https://github.com/o/r/pull/5/files",
        "https://github.com/o/r.git/pull/5",
    ] {
        let fx = Fixture::with_origin("/srv/git/not-github/repo.git");
        let outcome = callers::run_collect(&fx, Some(pr_url));
        if callers::github_remote(&outcome) != "false" {
            failures.push(format!("{pr_url:?}\n{}", outcome.describe()));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
