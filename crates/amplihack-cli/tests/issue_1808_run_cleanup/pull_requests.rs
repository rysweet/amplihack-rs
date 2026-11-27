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

fn closed_pr(number: usize, branch: &str, closed_hours_ago: i64, updated_hours_ago: i64) -> Value {
    let at = rfc3339_hours_ago(closed_hours_ago);
    json!({"number": number, "state": "closed", "head": {"ref": branch},
           "updated_at": rfc3339_hours_ago(updated_hours_ago), "closed_at": at, "merged_at": at})
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

        fx.gh_page(DEFAULT_REPOSITORY, "open", 1, true, "[]");
        fx.gh_page(
            DEFAULT_REPOSITORY,
            "open",
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
            assert_eq!(calls.len(), 5, "{calls:?}");
            assert!(
                calls.iter().all(|call| {
                    call.starts_with("api ")
                        && (call.contains("state=open")
                            || (call.contains("state=closed")
                                && call.contains("sort=updated")
                                && call.contains("direction=desc")))
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
                3
            );
            assert_eq!(
                calls
                    .iter()
                    .filter(|call| call.contains("repos/fixture-owner/second/pulls"))
                    .count(),
                2
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
    assert_eq!(fx.gh_calls().lines().count(), 2);
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
    for (state, calls) in [("closed", 3), ("open", 2)] {
        fx.gh_page(DEFAULT_REPOSITORY, state, 1, true, "[]");
        fx.gh_page(
            DEFAULT_REPOSITORY,
            state,
            2,
            false,
            r#"[{"number":1,"state":"unknown"}]"#,
        );
        let malformed = fx.cleanup_json(&["--apply"]);
        assert_kept(&malformed, &first);
        assert_kept(&malformed, &second);
        assert_eq!(fx.take_gh_calls().len(), calls);

        fx.gh_listing_fail(DEFAULT_REPOSITORY, state, 2);
        let failed = fx.cleanup_json(&["--apply"]);
        assert_kept(&failed, &first);
        assert_kept(&failed, &second);
        assert_eq!(fx.take_gh_calls().len(), calls);
    }
}

#[test]
fn a_terminal_page_before_the_advertised_last_page_keeps_worktrees() {
    let fx = Fx::new();
    let wt = seed_worktree(&fx, &fx.repo, "first", "on-unread-page");
    fx.gh_page_with_links(
        DEFAULT_REPOSITORY,
        "open",
        1,
        &[("next", 2), ("last", 3)],
        "[]",
    );
    fx.gh_page_with_links(DEFAULT_REPOSITORY, "open", 2, &[("prev", 1)], "[]");
    fx.gh_page(
        DEFAULT_REPOSITORY,
        "open",
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
    fx.gh_open("last-page", 1);
    let first_page = json!([closed_pr(102, old_branch, 72, 0)]);
    fx.gh_page(
        DEFAULT_REPOSITORY,
        "closed",
        1,
        true,
        &first_page.to_string(),
    );
    let last_closed_page = PAGE_CAP - 1;
    for page in 2..last_closed_page {
        // A short page can still have an explicit continuation.
        fx.gh_page(DEFAULT_REPOSITORY, "closed", page, true, "[]");
    }
    let rows: Vec<_> = (2..102)
        .map(|number| closed_pr(number, "other", 1, 1))
        .collect();
    let rows = json!(rows).to_string();
    fx.gh_page(DEFAULT_REPOSITORY, "closed", last_closed_page, true, &rows);
    let truncated = fx.cleanup_json(&["--apply"]);
    assert_kept(&truncated, &absent);
    assert_kept(&truncated, &present);
    assert_kept(&truncated, &old_merge);
    assert_eq!(fx.take_gh_calls().len(), PAGE_CAP);

    fx.gh_page(DEFAULT_REPOSITORY, "closed", last_closed_page, false, &rows);
    let complete = fx.cleanup_json(&["--apply"]);
    assert_eq!(entry_for(&complete, &absent)["action"], "removed");
    assert!(!absent.exists());
    assert_eq!(entry_for(&complete, &old_merge)["action"], "removed");
    assert!(!old_merge.exists());
    assert_kept(&complete, &present);
    assert_eq!(fx.take_gh_calls().len(), PAGE_CAP);
}

#[test]
fn large_history_only_blocks_runs_that_need_older_closures() {
    let fx = Fx::new();
    let absent = seed_worktree(&fx, &fx.repo, "first", "absent");
    let open = seed_worktree(&fx, &fx.repo, "first", "old-open");
    let recent = seed_worktree(&fx, &fx.repo, "first", "recently-closed");
    let touched = fx.worktree("touched");
    let touched_without_pr = fx.worktree("touched-without-pr");
    let mut run = SeedRun::ended(72)
        .owns_worktree(&touched, "touched")
        .owns_worktree(&touched_without_pr, "touched-without-pr");
    run.run_json_mtime_hours_ago = 1;
    fx.seed_run(&run);
    fx.gh_page(DEFAULT_REPOSITORY, "open", 1, true, "[]");
    fx.gh_page(
        DEFAULT_REPOSITORY,
        "open",
        2,
        false,
        &json!([open_pr(1, "old-open")]).to_string(),
    );
    let first = json!([
        closed_pr(4010, "recently-closed", 1, 1),
        closed_pr(4009, "unrelated", 48, 48),
    ]);
    fx.gh_page_with_links(
        DEFAULT_REPOSITORY,
        "closed",
        1,
        &[("next", 2), ("last", 41)],
        &first.to_string(),
    );
    fx.gh_page(
        DEFAULT_REPOSITORY,
        "closed",
        2,
        false,
        &json!([closed_pr(2, "touched", 72, 72)]).to_string(),
    );

    let report = fx.cleanup_json(&["--apply"]);
    assert_eq!(entry_for(&report, &absent)["action"], "removed");
    assert!(
        entry_for(&report, &absent)["reason"]
            .as_str()
            .unwrap()
            .starts_with("no open or recent PR")
    );
    assert!(!absent.exists());
    for wt in [&open, &recent, &touched, &touched_without_pr] {
        assert_kept(&report, wt);
    }
    assert_eq!(entry_for(&report, &open)["reason"], "PR #1 is open");
    assert!(
        entry_for(&report, &recent)["reason"]
            .as_str()
            .unwrap()
            .contains("within keep period (PR #4010")
    );
    assert!(
        entry_for(&report, &touched)["reason"]
            .as_str()
            .unwrap()
            .contains("40-request limit")
    );
    assert_eq!(fx.take_gh_calls().len(), 3);

    fx.gh_page_with_links(
        DEFAULT_REPOSITORY,
        "closed",
        1,
        &[("next", 2), ("last", 2)],
        &first.to_string(),
    );
    let report = fx.cleanup_json(&["--apply"]);
    assert_eq!(entry_for(&report, &touched)["action"], "removed");
    assert!(!touched.exists());
    for wt in [&open, &recent, &touched_without_pr] {
        assert_kept(&report, wt);
    }
    assert_eq!(fx.take_gh_calls().len(), 4);
}

#[test]
fn inconsistent_closed_pr_timestamps_cannot_establish_the_cutoff() {
    let fx = Fx::new();
    let wt = seed_worktree(&fx, &fx.repo, "first", "inconsistent");
    for rows in [
        json!([closed_pr(1, "other", 1, 48)]),
        json!([closed_pr(1, "other", 72, 48), closed_pr(2, "other", 1, 1)]),
    ] {
        fx.gh_page(DEFAULT_REPOSITORY, "closed", 1, true, &rows.to_string());
        let report = fx.cleanup_json(&["--apply"]);
        assert_kept(&report, &wt);
        assert!(
            entry_for(&report, &wt)["reason"]
                .as_str()
                .unwrap()
                .contains("inconsistent update times")
        );
        assert_eq!(fx.take_gh_calls().len(), 2);
    }
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
        "open",
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
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(
        calls
            .iter()
            .all(|call| call.contains("repos/fixture-owner/second/pulls"))
    );

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
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(
        calls
            .iter()
            .all(|call| call.contains("repos/fixture-owner/second/pulls"))
    );
}

#[test]
fn gh_repo_override_keeps_the_upstream_pr_open_in_a_fork_checkout() {
    let fx = Fx::new();
    let wt = seed_worktree(&fx, &fx.repo, "first", "upstream-open");
    fx.git_ok(&fx.repo, &["config", "remote.github.gh-resolved", "base"]);
    fx.gh_page(DEFAULT_REPOSITORY, "open", 1, false, "[]");
    fx.gh_page(DEFAULT_REPOSITORY, "closed", 1, false, "[]");
    fx.gh_page(
        SECOND_REPOSITORY,
        "open",
        1,
        false,
        &json!([open_pr(1, "upstream-open")]).to_string(),
    );

    for (selector, hostname) in [
        ("Fixture-Owner/Second", false),
        ("github.com/fixture-owner/second", true),
    ] {
        let report = fx.cleanup_json_with(&["--apply"], |c| {
            c.env("GH_REPO", selector);
        });
        assert_kept(&report, &wt);
        assert_eq!(entry_for(&report, &wt)["reason"], "PR #1 is open");
        let calls = fx.take_gh_calls();
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert!(calls[0].contains("repos/fixture-owner/second/pulls"));
        assert_eq!(calls[0].contains("--hostname github.com"), hostname);
    }

    for selector in ["ghe.example/fixture-owner/second", "fixture-owner/"] {
        let report = fx.cleanup_json_with(&["--apply"], |c| {
            c.env("GH_REPO", selector);
        });
        assert_kept(&report, &wt);
        assert!(fx.gh_calls().is_empty());
    }
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

    fx.gh_page(DEFAULT_REPOSITORY, "open", 1, false, "[]");
    let absent = fx.cleanup_json(&["--apply", "--only", "run-target"]);
    assert_eq!(entry_for(&absent, &target)["action"], "removed");
    assert!(!target.exists());
    assert!(wt.is_dir());
    assert_eq!(fx.take_gh_calls().len(), 2);
}
