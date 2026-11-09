# Branch protection on main

What a pull request needs before it can merge into `main`. The repository has
no separate git-policy file; this page is the record.

## Required status checks

`main` has classic branch protection that requires these 7 status checks. All
come from GitHub Actions in `.github/workflows/ci.yml`, which runs on every
pull request to `main` with no path filter:

- `Lint & Format`
- `Test`
- `Install Smoke Test`
- `Build x86_64-unknown-linux-gnu`
- `Build aarch64-unknown-linux-gnu`
- `Build x86_64-apple-darwin`
- `Build aarch64-apple-darwin`

Ryan, 9 Nov 2026: "our repos need to require that CI is passing before a merge
is allowed."

## Strict is off

The "require branches to be up to date before merging" setting is off. It was
turned off on 9 Nov 2026 on Ryan's word, through bb:0. A pull request does not
need to be up to date with `main` before it merges. Merge `main` into a pull
request only to resolve a real conflict.

Two older pages were written when strict was on:
[Merge Flow Reference](../reference/merge-flow.md) and
[Branch Protection Guard Reference](../reference/branch-protection-guard.md).
Both now carry a note pointing here; where they disagree with this page, this
page wins.

The guard workflow, `.github/workflows/branch-protection-guard.yml`, still
fails when strict is off once `BRANCH_PROTECTION_READ_TOKEN` is set. Today
that token is absent, so the guard skips with a warning. Retiring or inverting
the guard is tracked in
[#2203](https://github.com/rysweet/amplihack-rs/issues/2203).

## Checks that are not required

| Check | Why it is not required |
| --- | --- |
| `GitGuardian Security Checks` | An outside service, not CI. |
| `build`, `deploy` | `docs.yml` runs them only for some paths. |
| `Recipe runner pin ancestry` | It does not run on every pull request. |
| `Release`, `Prune sccache caches` | Skipped by design on pull requests. |

## Rule for CI changes

A required job must keep its name and must run, or be skipped by design, on
every pull request. To skip a job, use a job-level `if:`, which reports the
check as "skipped". Never use a workflow-level `paths:` filter for a workflow
that holds a required job: the check then never reports, and every merge
blocks waiting for it.

If you rename or remove a required job, update the branch protection settings
and this page in the same change.
