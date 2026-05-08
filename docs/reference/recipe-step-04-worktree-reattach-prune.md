# step-04-setup-worktree: Re-Prune After Orphan Directory Cleanup

`step-04-setup-worktree` creates an isolated git worktree for each workflow
run. When reattaching to an existing branch whose worktree directory was
deleted out-of-band (e.g., by `rm -rf` or a cleanup script), the step now
runs `git worktree prune` immediately after removing the orphaned directory
to clear stale registrations before calling `git worktree add`.

**Added in:** PR #4394 (merged 2026-04-18)
**Affects:** `amplifier-bundle/recipes/default-workflow.yaml`

---

## Quick Start

No configuration required. `step-04-setup-worktree` resolves a supported remote
base ref automatically and handles stale worktree registrations during any
reattach path.

```bash
# Run the default workflow — step-04 handles stale registrations transparently
amplihack recipe run default-workflow -c task_description="Fix issue #1234" \
  -c repo_path="$(pwd)"
```

The step works in repositories whose remote default branch is `main`, `master`,
`develop`, or another Git-verified `origin/HEAD` target. It no longer treats
`origin/main` as the only valid base.

---

## Remote Base Branch Resolution

`workflow-worktree` resolves the base ref before creating or reattaching a
workflow branch. Resolution is fixed and deterministic:

1. `origin/HEAD`
2. `origin/master`
3. `origin/develop`

`origin/HEAD` is preferred because it represents the remote's configured default
branch. Its symbolic target is accepted only after Git verifies that it resolves
to a remote-tracking ref under `refs/remotes/origin/`; the target may be
`origin/main`, `origin/master`, `origin/develop`, or another remote default. The
fallback refs support clones that do not have `origin/HEAD` populated locally and
repositories that still use `master` or `develop` as the default branch.

### Behavior by Repository Shape

| Remote refs available                   | Selected base ref | Result                                      |
| --------------------------------------- | ----------------- | ------------------------------------------- |
| `origin/HEAD -> origin/main`            | `origin/main`     | Branches from the remote default branch.    |
| `origin/HEAD -> origin/master`          | `origin/master`   | Works without a manual `main` override.     |
| `origin/HEAD -> origin/release`         | `origin/release`  | Uses the Git-verified remote default.       |
| `origin/master` only                    | `origin/master`   | Uses the first supported fallback.          |
| `origin/develop` only                   | `origin/develop`  | Uses the second supported fallback.         |
| none of the supported sources are valid | none              | Fails closed with an actionable error.      |

### Command Shape

For a new workflow branch, the worktree command uses the resolved base ref:

```bash
git worktree add "$WORKTREE_PATH" -b "$BRANCH_NAME" "$BASE_REF"
```

The resolved base is also used for branch currency and wrong-base diagnostics,
so a `master`-based repository is not reported as wrong solely because it lacks
`origin/main`.

### Failure Semantics

If `origin/HEAD`, `origin/master`, and `origin/develop` are all missing or
unresolvable, the step stops before creating a worktree. The failure message
names the checked refs and tells the operator to fetch or configure the remote
default branch. The step does not silently fall back to a local branch,
unqualified ref, or local `HEAD` bootstrap mode.

If a prior worktree directory was removed without `git worktree remove`, the
step logs:

```
INFO: Removing orphaned worktree directory '/path/to/worktrees/fix-issue-1234'
```

and then prunes the stale `.git/worktrees/` registration before re-creating
the worktree.

---

## Problem

`step-04-setup-worktree` uses a three-state idempotency guard (see below) to
handle re-runs. Two of those states involve reattaching to a branch when the
worktree directory is missing:

- **State 2** — Branch exists, worktree missing → `git worktree add`
- **State 3** — New branch, but orphan directory present → `rm -rf` + `git worktree add -b`

Both states deleted the orphaned directory with `rm -rf` but did **not**
re-run `git worktree prune` before calling `git worktree add`. If git's
internal `.git/worktrees/<name>` registration still existed (because the
initial prune at the top of the step ran before the directory was removed, or
because a user/script deleted the directory out-of-band), the subsequent
`worktree add` would fail with:

```
fatal: '<path>' is a missing but already registered worktree;
use 'add -f' to override, or 'prune' or 'remove' to clear
```

This made workflow re-runs fragile — any interrupted run that left a stale
worktree directory would block all subsequent attempts.

---

## Solution

An explicit `git worktree prune >&2` call is inserted immediately after
every `rm -rf "${WORKTREE_PATH}"` in the reattach branches. This clears
the stale `.git/worktrees/` registration so the following `worktree add`
succeeds without `--force`.

### Prune Points in step-04

The step now prunes at four points:

| #   | When                                                     | Purpose                                               |
| --- | -------------------------------------------------------- | ----------------------------------------------------- |
| 1   | Top of step (pre-existing)                               | Clear any stale refs before detection                 |
| 2   | After wrong-base-branch cleanup (pre-existing, PR #4254) | Clear ref after `worktree remove --force`             |
| 3   | **After orphan cleanup in State 2** (new, PR #4394)      | Clear ref after `rm -rf` of missing worktree dir      |
| 4   | **After orphan cleanup in State 3** (new, PR #4394)      | Clear ref after `rm -rf` of orphan dir for new branch |

---

## Three-State Idempotency Guard

`step-04-setup-worktree` detects three possible states on each run:

```
input: BRANCH_NAME + WORKTREE_PATH
         │
         ▼
┌─────────────────────────────────────────────────────────┐
│  State 1 — Branch exists + worktree exists              │
│  → Reuse silently, output created=false                 │
│  (Also checks for wrong base branch — PR #4254)        │
├─────────────────────────────────────────────────────────┤
│  State 2 — Branch exists + worktree missing             │
│  → rm -rf orphan dir (if present)                       │
│  → git worktree prune          ← NEW (PR #4394)        │
│  → git worktree add <path> <branch>                     │
├─────────────────────────────────────────────────────────┤
│  State 3 — Branch missing (new branch)                  │
│  → rm -rf orphan dir (if present)                       │
│  → git worktree prune          ← NEW (PR #4394)        │
│  → git worktree add <path> -b <branch> <base-ref>      │
└─────────────────────────────────────────────────────────┘
```

---

## Reproduction & Verification

To reproduce the original failure:

```bash
# 1. Create a worktree normally
git worktree add ./worktrees/test-branch -b test-branch main

# 2. Delete the directory (simulating cleanup script or interrupted run)
rm -rf ./worktrees/test-branch

# 3. Without the fix, this fails:
git worktree add ./worktrees/test-branch test-branch
# fatal: './worktrees/test-branch' is a missing but already registered worktree

# 4. With the fix, step-04 runs `git worktree prune` first, so add succeeds
git worktree prune
git worktree add ./worktrees/test-branch test-branch
# ✓ success
```

---

## Related Documentation

- [step-03 Idempotency Guards](recipe-step-03-idempotency.md) — Issue-creation deduplication
- [Troubleshoot Worktree](../howto/troubleshoot-worktree.md) — General worktree debugging
- [Worktree Support](../concepts/worktree-support.md) — Feature overview
