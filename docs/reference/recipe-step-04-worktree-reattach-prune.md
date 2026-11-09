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
base ref automatically, or starts from the `base_branch` / `base_sha` you pass
(see [Explicit Worktree Source](#explicit-worktree-source-base_branch-base_sha)),
and handles stale worktree registrations during any reattach path.

```bash
# Run the default workflow — step-04 handles stale registrations transparently
amplihack recipe run default-workflow -c task_description="Fix issue #1234" \
  -c repo_path="$(pwd)"
```

The step works in repositories whose remote default branch is `main`, `master`,
`develop`, or another Git-verified `origin/HEAD` target. It no longer treats
`origin/main` as the only valid base.

---

## Explicit Worktree Source (`base_branch`, `base_sha`)

A caller can name the commit the worktree starts from with two inputs, declared
in both `default-workflow.yaml` and `consensus-workflow.yaml` (issue #1957):

```bash
amplihack recipe run default-workflow \
  -c task_description="Fix flaky reconnect in the session store" \
  -c repo_path="$(pwd)" \
  -c base_branch=dev
```

| Inputs | Worktree starts at | PR base |
| --- | --- | --- |
| `base_branch=dev` | tip of `origin/dev` after a fetch of `+refs/heads/dev:refs/remotes/origin/dev` | `dev` |
| `base_sha=<full sha>` | that commit | none: the PR steps refuse |
| both | the SHA, which must be contained in `origin/dev` | `dev` |
| neither | the legacy base below, with a WARNING | resolved from `origin/HEAD`, as before |

`base_sha` must be a full 40- or 64-character hex SHA; abbreviated SHAs are
refused. When an explicit input is set, the fallback chain below is never used
and a symbolic `origin/HEAD` is never read. If `origin/dev` does not exist on
the remote, the step stops with an ERROR.

A `base_sha` alone pins the exact tree but does not prove which branch the
commit came from. The run can build and test from it, but the PR steps refuse
to publish, because there is no verified branch to target. Pass `base_branch`
as well when the run must open a PR:

```bash
amplihack recipe run default-workflow \
  -c task_description="Fix the reconnect regression introduced at the release commit" \
  -c repo_path="$(pwd)" \
  -c base_branch=dev \
  -c base_sha=3f9c2a7e1b0d4c5a6e7f8091a2b3c4d5e6f70812
```

Resolution and verification live in
[`workflow_worktree_source.sh`](../../amplifier-bundle/tools/workflow_worktree_source.md),
which documents every output key, the fetch-failure table and the full list of
ERROR messages.

### Order of checks

On the explicit path, all of these run before the deconflict helper and before
any branch or worktree exists. A refusal exits 1 and leaves the worktree and
branch lists unchanged.

1. Validate the inputs.
2. Identity check: `origin` exists and the physical `MAIN_REPO_PATH` equals
   `git rev-parse --show-toplevel`.
3. Strict fetch, one attempt each with no retry or sleep: a plain
   `git fetch origin --no-tags` first, so a failure means origin is
   unreachable, then the explicit refspec
   `+refs/heads/<b>:refs/remotes/origin/<b>`. A refspec fetch that fails after
   the plain fetch worked means the branch is not on origin
   (`refuse-missing-branch`).
4. If the plain fetch failed, refuse with `refuse-fetch-failed` whatever the
   inputs. A cached `origin/<b>` or a locally present commit is never used in
   place of a fresh fetch.
5. `git rev-parse --verify --quiet <sha>^{commit}` must print exactly
   `<sha>`; otherwise `git cat-file -e <sha>` picks `refuse-missing-sha` or
   `refuse-not-commit` (tags, trees and blobs). Then containment in
   `origin/<b>`, only when both inputs are set.

`KEPT_AHEAD` and `git worktree add` use `${BASE_SHA:-$BASE_REF}`, so the
#1565 work-preservation check compares against the verified commit, and with
both inputs the worktree starts at the pinned SHA, not the branch tip. The
deconflict helper still gets `--base "$BASE_REF"`; that is the same commit
except when both inputs are set, where it is the fetched `origin/<b>` tip. A
kept branch with commits past the pinned SHA is then refused by `KEPT_AHEAD`
rather than renamed, and nothing is reset. `BASE_REF` itself stays
`origin/<b>` for the JSON and the later PR steps.

### Checks after creation, and cleanup

After `git worktree add`, `verify-worktree` checks that the worktree's
toplevel is `WORKTREE_PATH`, that it shares the main repository's git common
dir, and that the origin URL has not changed. It prints no `RULE:` line; a
failure prints exactly one `ERROR:` line. Then:

| State (below) | Checked as | Rule | On failure |
| --- | --- | --- | --- |
| 3 — new branch | `created` | `HEAD` equals the verified SHA | ERROR; removes the worktree and deletes the branch this run created |
| 2 — branch existed, worktree attached now | `reused` | `HEAD` contains the verified SHA | ERROR; removes only the worktree |
| 1 — both existed | `reused` | `HEAD` contains the verified SHA | ERROR; touches nothing |

The step passes `--cleanup-created` to `verify-worktree` in states 2 and 3,
never in state 1. With that flag the tool removes the worktree, and deletes the
branch only in `created` mode, before it prints its ERROR line. A kept branch
that already contains the verified SHA is reused. A rerun whose kept branch
does not contain it is refused, not fast-forwarded:

```text
ERROR: reused worktree /home/ryan/src/amplihack-rs/worktrees/feat/issue-1957-pin-base HEAD 0b4e… does not contain verified base 5d1e…; delete branch feat/issue-1957-pin-base or rerun with existing_branch (issue #1957).
```

### Existing-branch path

`existing_branch` and `pr_number` ignore both inputs. If either is set the step
logs `INFO: base_branch/base_sha ignored on the existing-branch path`, and the
JSON reports `"source_verified": "not-applicable"` and
`"source_rule": "existing-branch"`.

### Old bundles

If `workflow_worktree_source.sh` is missing from the tools directory, the step
fails closed when an input is set:

```text
ERROR: base_branch/base_sha given but workflow_worktree_source.sh is missing from <tools-dir>; refusing an unverified worktree source (issue #1957).
```

With no input set it runs a minimal inline legacy fallback, so older bundles
still set up a worktree.

---

## Remote Base Branch Resolution

With neither `base_branch` nor `base_sha` set, `workflow-worktree` resolves
the base ref as before and logs:

```text
WARNING: no base_branch/base_sha given; legacy base resolution (<ref>), source not verified
```

`<ref>` is the ref actually resolved by the chain below. The tool also prints
exactly one `RULE: legacy-unverified` line. The JSON reports
`"source_verified": "legacy"` and `"source_rule": "legacy-unverified"`. The
legacy `base_branch` is never used by `verified-base`, so the later PR steps
keep their existing base chain and the PR base does not change. Resolution is fixed and deterministic:

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
- [Worktree Source Resolution](../../amplifier-bundle/tools/workflow_worktree_source.md) — `base_branch` / `base_sha` contract
- [worktree_setup Context Propagation](worktree-setup-propagation.md) — JSON fields read by later steps
