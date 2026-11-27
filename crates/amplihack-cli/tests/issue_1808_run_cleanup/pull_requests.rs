//! Outside-in coverage of repository-wide PR evidence in `amplihack cleanup`.

use super::fixture::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const SECOND_REPOSITORY: &str = "fixture-owner/second";
const PAGE_CAP: usize = 40;

fn seed_worktree(fx: &Fx, checkout: &Path, group: &str, branch: &str) -> PathBuf {
    let wt = fx.add_worktree(checkout, group, branch);
    fx.seed_run(&SeedRun::ended(72).owns_worktree(&wt, branch));
    wt
}

fn assert_kept(report: &Value, wt: &Path) {
    let entry = entry_for(report, wt);
    assert_eq!(entry["action"], "skipped", "{entry}");
    assert!(
        entry["reason"].as_str().is_some_and(|r| !r.is_empty()),
        "{entry}"
    );
    assert!(wt.is_dir(), "{wt:?}");
}

fn open_pr(number: usize, branch: &str) -> Value {
    json!({"number": number, "state": "open", "head": {"ref": branch},
           "closed_at": null, "merged_at": null})
}

#[test]
fn manual_report_and_apply_share_bounded_listings_across_checkouts() {
    for count in [30, 60] {
        let fx = Fx::new();
        let alias = fx.clone_for_repository("same", "FIXTURE-OWNER/FIRST");
        let other = fx.clone_for_repository("other", SECOND_REPOSITORY);
        let groups = [(&fx.repo, "first"), (&alias, "alias"), (&other, "second")];
        let mut worktrees = Vec::new();
        for index in 0..count {
            let (checkout, group) = groups[index % groups.len()];
            let branch = if index < groups.len() {
                "shared".into()
            } else {
                format!("branch-{index}")
            };
            let wt = seed_worktree(&fx, checkout, group, &branch);
            worktrees.push((wt, branch == "shared" && group != "second"));
        }

        fx.gh_page(DEFAULT_REPOSITORY, 1, true, "[]");
        fx.gh_page(
            DEFAULT_REPOSITORY,
            2,
            false,
            &json!([open_pr(1, "shared")]).to_string(),
        );

        for apply in [false, true] {
            let args = if apply { &["--apply"][..] } else { &[] };
            let report = fx.cleanup_json(args);
            for (wt, kept) in &worktrees {
                if *kept {
                    assert_kept(&report, wt);
                } else {
                    let action = if apply { "removed" } else { "would-remove" };
                    assert_eq!(entry_for(&report, wt)["action"], action, "{wt:?}");
                    assert_eq!(wt.is_dir(), !apply, "{wt:?}");
                }
            }
            let calls = fx.take_gh_calls();
            assert_eq!(calls.len(), 3, "{calls:?}");
            assert!(
                calls.iter().all(|call| {
                    call.starts_with("api ")
                        && call.contains("state=all")
                        && !call.contains("--head")
                        && !call.contains("graphql")
                }),
                "{calls:?}"
            );
            assert_eq!(
                calls
                    .iter()
                    .filter(|call| call.contains("repos/fixture-owner/first/pulls"))
                    .count(),
                2
            );
            assert_eq!(
                calls
                    .iter()
                    .filter(|call| call.contains("repos/fixture-owner/second/pulls"))
                    .count(),
                1
            );
        }
    }
}

#[test]
fn apply_uses_pr_state_and_activity_for_older_runs() {
    let fx = Fx::new();
    fx.gh_closed("open-wins", 1, 72, true);
    fx.gh_open("open-wins", 2);
    fx.gh_closed("closed-recent", 3, 1, false);
    fx.gh_closed("closed-recent", 4, 72, false);
    fx.gh_closed("merged-recent", 5, 1, true);
    fx.gh_closed("closed-old", 6, 72, false);
    fx.gh_closed("merged-old", 7, 72, true);
    let cases = [
        ("open-wins", 72, 72, true),
        ("closed-recent", 72, 72, true),
        ("merged-recent", 72, 72, true),
        ("closed-old", 72, 1, false),
        ("merged-old", 72, 72, false),
        ("missing-pr", 72, 72, false),
        ("touched-run", 72, 1, true),
    ];
    let mut worktrees = Vec::new();
    for (branch, ended, mtime, keep) in cases {
        let wt = fx.add_worktree(&fx.repo, "verdicts", branch);
        let mut run = SeedRun::ended(ended).owns_worktree(&wt, branch);
        run.run_json_mtime_hours_ago = mtime;
        fx.seed_run(&run);
        worktrees.push((wt, keep));
    }

    let report = fx.cleanup_json(&["--apply"]);
    for (wt, keep) in worktrees {
        if keep {
            assert_kept(&report, &wt);
        } else {
            assert_eq!(entry_for(&report, &wt)["action"], "removed", "{wt:?}");
            assert!(!wt.exists(), "{wt:?}");
        }
    }
    assert_eq!(fx.gh_calls().lines().count(), 1);
}

#[test]
fn recent_runs_and_detached_checkouts_do_not_need_listings() {
    let fx = Fx::new();
    let recent = fx.worktree("recent-run");
    fx.gh_closed("recent-run", 1, 72, true);
    fx.seed_run(&SeedRun::ended(1).owns_worktree(&recent, "recent-run"));

    let detached = fx.add_worktree(&fx.repo, "verdicts", "detached");
    fx.git_ok(&detached, &["switch", "--detach", "-q"]);
    fx.seed_run(&SeedRun::ended(72).owns_worktree(&detached, ""));

    let report = fx.cleanup_json(&["--apply"]);
    assert_kept(&report, &recent);
    assert_eq!(entry_for(&report, &detached)["action"], "removed");
    assert!(!detached.exists());
    assert!(fx.gh_calls().is_empty());
}

#[test]
fn failed_or_malformed_pages_keep_all_dependent_worktrees() {
    let fx = Fx::new();
    let first = seed_worktree(&fx, &fx.repo, "first", "first");
    let second = seed_worktree(&fx, &fx.repo, "first", "second");
    fx.gh_page(DEFAULT_REPOSITORY, 1, true, "[]");
    fx.gh_page(
        DEFAULT_REPOSITORY,
        2,
        false,
        r#"[{"number":1,"state":"unknown"}]"#,
    );
    let malformed = fx.cleanup_json(&["--apply"]);
    assert_kept(&malformed, &first);
    assert_kept(&malformed, &second);
    assert_eq!(fx.take_gh_calls().len(), 2);

    fx.gh_fail(DEFAULT_REPOSITORY, 2);
    let failed = fx.cleanup_json(&["--apply"]);
    assert_kept(&failed, &first);
    assert_kept(&failed, &second);
    assert_eq!(fx.take_gh_calls().len(), 2);
}

#[test]
fn a_terminal_page_before_the_advertised_last_page_keeps_worktrees() {
    let fx = Fx::new();
    let wt = seed_worktree(&fx, &fx.repo, "first", "on-unread-page");
    fx.gh_page_with_links(DEFAULT_REPOSITORY, 1, &[("next", 2), ("last", 3)], "[]");
    fx.gh_page_with_links(DEFAULT_REPOSITORY, 2, &[("prev", 1)], "[]");
    fx.gh_page(
        DEFAULT_REPOSITORY,
        3,
        false,
        &json!([open_pr(1, "on-unread-page")]).to_string(),
    );

    let report = fx.cleanup_json(&["--apply"]);
    assert_kept(&report, &wt);
    assert_eq!(fx.gh_calls().lines().count(), 2);
}

#[test]
fn cap_requires_an_end_signal_even_when_the_last_page_is_full() {
    let fx = Fx::new();
    let absent = seed_worktree(&fx, &fx.repo, "first", "absent");
    let present = seed_worktree(&fx, &fx.repo, "first", "last-page");
    let old_branch = "touched-old-merge";
    let old_merge = fx.worktree(old_branch);
    let mut run = SeedRun::ended(72).owns_worktree(&old_merge, old_branch);
    run.run_json_mtime_hours_ago = 1;
    fx.seed_run(&run);
    let old = rfc3339_hours_ago(72);
    let first_page = json!([
        {"number": 101, "state": "closed", "head": {"ref": old_branch},
         "closed_at": old, "merged_at": old}
    ]);
    fx.gh_page(DEFAULT_REPOSITORY, 1, true, &first_page.to_string());
    for page in 2..PAGE_CAP {
        // A short page can still have an explicit continuation.
        fx.gh_page(DEFAULT_REPOSITORY, page, true, "[]");
    }
    let mut rows: Vec<_> = (1..100).map(|number| open_pr(number, "other")).collect();
    rows.push(open_pr(100, "last-page"));
    rows.reverse();
    let rows = json!(rows).to_string();
    fx.gh_page(DEFAULT_REPOSITORY, PAGE_CAP, true, &rows);
    let truncated = fx.cleanup_json(&["--apply"]);
    assert_kept(&truncated, &absent);
    assert_kept(&truncated, &present);
    assert_kept(&truncated, &old_merge);
    assert_eq!(fx.take_gh_calls().len(), PAGE_CAP);

    fx.gh_page(DEFAULT_REPOSITORY, PAGE_CAP, false, &rows);
    let complete = fx.cleanup_json(&["--apply"]);
    assert_eq!(entry_for(&complete, &absent)["action"], "removed");
    assert!(!absent.exists());
    assert_eq!(entry_for(&complete, &old_merge)["action"], "removed");
    assert!(!old_merge.exists());
    assert_kept(&complete, &present);
    assert_eq!(fx.take_gh_calls().len(), PAGE_CAP);
}

#[test]
fn conflicting_repository_identities_keep_the_worktree_without_a_listing() {
    let fx = Fx::new();
    let wt = seed_worktree(&fx, &fx.repo, "first", "ambiguous");
    fx.git_ok(
        &fx.repo,
        &[
            "config",
            "remote.other.url",
            "git@github.com:fixture-owner/second.git",
        ],
    );

    let report = fx.cleanup_json(&["--apply"]);
    assert_kept(&report, &wt);
    assert!(fx.gh_calls().is_empty());
}

#[test]
fn configured_github_repository_selects_the_upstream_of_a_fork() {
    let fx = Fx::new();
    let open = seed_worktree(&fx, &fx.repo, "first", "upstream-open");
    let absent = seed_worktree(&fx, &fx.repo, "first", "upstream-absent");
    fx.git_ok(
        &fx.repo,
        &[
            "config",
            "remote.upstream.url",
            "git@github.com:fixture-owner/second.git",
        ],
    );
    fx.gh_page(
        SECOND_REPOSITORY,
        1,
        false,
        &json!([open_pr(1, "upstream-open")]).to_string(),
    );
    fx.gh_open("upstream-absent", 2);

    // gh writes "base" on a matching remote, or owner/repo on a remote
    // when the upstream has no corresponding fetch URL.
    fx.git_ok(&fx.repo, &["config", "remote.upstream.gh-resolved", "base"]);
    let report = fx.cleanup_json(&[]);
    assert_kept(&report, &open);
    assert_eq!(entry_for(&report, &absent)["action"], "would-remove");
    let calls = fx.take_gh_calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(calls[0].contains("repos/fixture-owner/second/pulls"));

    fx.git_ok(&fx.repo, &["remote", "remove", "upstream"]);
    fx.git_ok(
        &fx.repo,
        &[
            "config",
            "remote.github.gh-resolved",
            "Fixture-Owner/Second",
        ],
    );
    let report = fx.cleanup_json(&["--apply"]);
    assert_kept(&report, &open);
    assert_eq!(entry_for(&report, &absent)["action"], "removed");
    assert!(!absent.exists());
    let calls = fx.take_gh_calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(calls[0].contains("repos/fixture-owner/second/pulls"));
}

#[test]
fn selecting_only_legacy_targets_still_reads_their_worktrees_prs() {
    let fx = Fx::new();
    let wt = fx.worktree("legacy-worktree");
    let target = fx.target("amplihack-precommit-target-0123456789ab-run-legacy", 64);
    fx.seed_run(
        &SeedRun::ended(72)
            .owns_worktree(&wt, "legacy-worktree")
            .owns_target(&target),
    );
    fx.gh_open("legacy-worktree", 1);

    let open = fx.cleanup_json(&["--apply", "--only", "run-target"]);
    assert_kept(&open, &target);
    assert_eq!(fx.take_gh_calls().len(), 1);

    fx.gh_page(DEFAULT_REPOSITORY, 1, false, "[]");
    let absent = fx.cleanup_json(&["--apply", "--only", "run-target"]);
    assert_eq!(entry_for(&absent, &target)["action"], "removed");
    assert!(!target.exists());
    assert!(wt.is_dir());
    assert_eq!(fx.take_gh_calls().len(), 1);
}
