# How to Watch CI and Auto-Merge a Pull Request

This guide shows how to use `amplihack pr watch-and-merge` to poll CI checks on a GitHub pull request and merge it automatically once all checks pass.

## Prerequisites

You need `gh` CLI installed and authenticated:

```sh
gh auth status
# ✓ Logged in to github.com account owner (keyring)
```

If this fails, run `gh auth login` first.

## Watch and Squash-Merge

The simplest invocation watches a PR and squash-merges it when CI is green:

```sh
amplihack pr watch-and-merge 352
```

The command polls every 30 seconds (default), prints progress to stderr, and merges when all checks pass:

```
⏳ Waiting for checks on PR #352... (attempt 1, next check in 30s)
⏳ Waiting for checks on PR #352... (attempt 2, next check in 30s)
✅ All checks passed. Merging PR #352 with squash strategy...
✓ PR #352 merged.
```

## Choose a Merge Strategy

Use `--rebase` or `--merge` instead of the default squash:

```sh
# Rebase-merge
amplihack pr watch-and-merge 352 --rebase

# Merge commit
amplihack pr watch-and-merge 352 --merge
```

## Delete the Remote Branch After Merge

Add `--delete-branch` to clean up:

```sh
amplihack pr watch-and-merge 352 --delete-branch
```

## Poll Faster

Reduce the interval to 10 seconds (minimum 5):

```sh
amplihack pr watch-and-merge 352 --interval 10
```

## Bypass Branch Protection

If you have admin access and want to merge before required reviews:

```sh
amplihack pr watch-and-merge 352 --admin
```

## Push and Watch in One Step

Combine `git push` with `watch-and-merge` for a single command that pushes your branch and watches it:

```sh
git push -u origin feat/my-feature
amplihack pr watch-and-merge "$(gh pr view --json number -q .number)" --delete-branch
```

The `gh pr view` command requires a PR to already exist for the current branch. If you just pushed a new branch, create the PR first with `gh pr create`, or use `gh pr create --json number -q .number` in place of `gh pr view` to create and watch in one step:

```sh
git push -u origin feat/my-feature
amplihack pr watch-and-merge "$(gh pr create --fill --json number -q .number)" --delete-branch
```

## Handle Check Failures

If any CI check fails, the command exits with code `1` and prints the failing checks:

```
✗ CI checks failed on PR #352:
  - build (linux) — https://github.com/owner/repo/actions/runs/123456
```

Fix the failing check, push again, and re-run the command.

## Troubleshooting

**"gh CLI is not authenticated"** — Run `gh auth login` and follow the prompts.

**"No checks found"** — The repository has no required checks configured. The command prints a warning and proceeds to merge. If this is unexpected, check your repository's branch protection settings.

**Merge fails after checks pass** — The PR may have a merge conflict or violate branch protection rules. The error from `gh pr merge` is printed directly. Resolve the conflict and re-run.

**Command hangs indefinitely** — A CI check is stuck in a pending state. Press Ctrl+C to abort, investigate the stuck check in the GitHub UI, and re-run when ready.
