# Merge Flow Reference

> [Home](../index.md) > Reference > Merge Flow

`main` is protected by 7 required status checks (see [CI Pipeline](ci-pipeline.md)):
`Lint & Format`, `Test`, `Install Smoke Test`, and the four `Build <target>` legs.

`main` also requires that a branch be **up to date** with `main` before it can
merge (the "strict" policy). This strict up-to-date protection, together with all
7 required checks, stays **on**. It is intentional and must not be turned off.

## Why not a GitHub merge queue?

A GitHub merge queue would let several PRs line up and merge in order without each
one needing a manual update first, but it is **not available for this repository**.
Merge queue is offered only for repositories owned by an **organization**;
`rysweet/amplihack-rs` is owned by a personal user account, so the REST API rejects
a `merge_queue` ruleset rule regardless of plan or visibility. See
[Managing a merge queue](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/managing-a-merge-queue)
("available for public repositories … owned by an organization").

If this repository is ever transferred to an organization, a merge queue can be
enabled then. The CI workflow already triggers on the `merge_group` event, so no
workflow change would be needed at that point.

## Merge flow for contributors and agents

Because there is no merge queue, PRs are merged **one at a time (serially)**. Each
PR is merged only after it is both green and up to date with `main`:

1. Wait until every required check on the PR is green.
2. Make sure the PR branch is up to date with `main`. If `main` has moved ahead,
   update the branch (merge or rebase `main` into it) and let the required checks
   run again on the updated branch.
3. When the PR is green **and** up to date with the latest `main`, squash-merge it:

   ```bash
   gh pr merge <number> --squash
   ```

4. Merge the next PR only after the previous one has landed, repeating the same
   steps so each merge is preceded by an up-to-date CI run.

Do **not** use `gh pr merge --admin`, and do **not** use `--no-verify`. Every PR
goes through the same required checks with the strict up-to-date policy in place.

## Keeping the strict policy on

The strict up-to-date policy above is enforced automatically by the
[Branch Protection Guard](branch-protection-guard.md): an in-repo check that
reads the live `required_status_checks.strict` setting and fails within one CI
cycle if it is ever turned off. That guard is detection, not prevention — it
makes the change loud so it can be reverted; only an org- or enterprise-level
ruleset can prevent the change outright.
