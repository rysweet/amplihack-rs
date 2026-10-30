//! #1714 case 2, crusty round 5: what runs after the probe accepts a PR head
//! that is ahead of the local HEAD.
//!
//! `workflow-finalize` sets `accept_pr_head_ahead: "true"`, so the probe lets
//! the run continue when main was merged into the PR from elsewhere. The
//! remote branch is then ahead of the worktree. Before round 5, step-20b
//! pulled only when the branch had an upstream; without one it ran a bare
//! `git push -u`, which the remote rejects, and the run ended without a
//! hand-off. And the agent steps (step-20, step-20c) edited a tree that did
//! not hold the PR head's commits.
//!
//! These tests run `step-20-sync-pr-head` and `step-20b-push-cleanup` exactly
//! as `workflow-finalize.yaml` holds them, in a linked worktree whose branch
//! on a private bare remote is a merge of main on top of the local HEAD. They
//! check, with and without an upstream and with and without a
//! remote-tracking ref, that:
//! - the sync step fast-forwards the worktree to the PR head before any
//!   edit, and sets the upstream;
//! - an edit made after it (standing in for step-20/20c) is committed on top
//!   of the PR head and pushed, and the remote branch equals the local HEAD;
//! - step-20b alone, with no upstream and `origin/<branch>` stale or unknown
//!   locally, fetches, sets the upstream, pulls and pushes rather than being
//!   rejected.

use super::step20b_remote::*;

/// The cleanup commit sits on the PR head, holds the edit, and is what the
/// remote branch now points at.
fn assert_pushed_on_pr_head(fx: &Remote, why: &str) {
    let head = fx.head();
    assert_eq!(
        fx.remote_head(),
        head,
        "{why}: the remote must equal the local HEAD"
    );
    assert!(
        fx.is_ancestor(&fx.pr_head, &head),
        "{why}: the PR head {} must be in the pushed history",
        fx.pr_head
    );
    assert!(
        fx.is_ancestor(&fx.local, &head),
        "{why}: the run's own commit must be in the pushed history"
    );
    assert_eq!(
        git(&fx.wt, &["show", "HEAD:audit.txt"]),
        "audit fix",
        "{why}: the agent edit must be in the pushed commit"
    );
    assert!(
        git(&fx.wt, &["status", "--porcelain"]).is_empty(),
        "{why}: nothing may be left uncommitted"
    );
}

#[test]
fn sync_then_push_lands_on_the_pr_head_with_or_without_an_upstream() {
    for tracking in [Tracking::Upstream, Tracking::RefOnly, Tracking::Nothing] {
        let why = format!("{tracking:?}");
        let fx = Remote::new(tracking);

        let sync = fx.run("step-20-sync-pr-head");
        assert_ok("step-20-sync-pr-head", &sync);
        assert_eq!(
            fx.head(),
            fx.pr_head,
            "{why}: the agent steps must start from the PR head\n{}",
            describe("step-20-sync-pr-head", &sync)
        );
        assert_eq!(
            fx.upstream(),
            format!("origin/{BR}"),
            "{why}: the sync must leave an upstream for step-20b"
        );

        fx.agent_edit();
        let push = fx.run("step-20b-push-cleanup");
        assert_ok("step-20b-push-cleanup", &push);
        assert_pushed_on_pr_head(&fx, &why);
        assert_eq!(
            git(&fx.wt, &["rev-parse", "HEAD^"]),
            fx.pr_head,
            "{why}: the cleanup commit's parent must be the PR head"
        );
    }
}

#[test]
fn push_without_upstream_fetches_pulls_and_pushes() {
    // step-20b on its own (step-20-sync-pr-head skipped or its fetch failed):
    // with no upstream, and with origin/<branch> stale or unknown locally, it
    // must fetch, set the upstream and pull, not bare-push to a remote ahead.
    for tracking in [Tracking::RefOnly, Tracking::Nothing] {
        let why = format!("step-20b alone, {tracking:?}");
        let fx = Remote::new(tracking);
        assert!(
            git(
                &fx.wt,
                &["config", "--get-regexp", "^branch\\..*\\.remote$"]
            )
            .lines()
            .all(|l| !l.contains(BR)),
            "{why}: the branch must start with no upstream"
        );
        fx.agent_edit();
        let push = fx.run("step-20b-push-cleanup");
        assert_ok("step-20b-push-cleanup", &push);
        assert_eq!(fx.upstream(), format!("origin/{BR}"), "{why}");
        assert_pushed_on_pr_head(&fx, &why);
    }
}

#[test]
fn sync_leaves_a_worktree_alone_when_the_remote_is_not_ahead() {
    // Remote equal to the local HEAD: nothing to fast-forward, no commits.
    let fx = Remote::new(Tracking::Upstream);
    git(
        &fx.root.join("origin.git"),
        &["update-ref", &format!("refs/heads/{BR}"), &fx.local],
    );
    let sync = fx.run("step-20-sync-pr-head");
    assert_ok("step-20-sync-pr-head", &sync);
    assert_eq!(fx.head(), fx.local, "{}", describe("sync", &sync));
}
