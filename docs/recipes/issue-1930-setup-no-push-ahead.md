---
title: Worktree setup does not push an existing branch (issue #1930)
type: reference
applies_to:
  - amplifier-bundle/recipes/workflow-worktree.yaml (step-04-setup-worktree)
  - amplifier-bundle/recipes/consensus-issue-worktree.yaml (step3-setup-worktree)
closes: "#1930"
---

# Worktree setup does not push an existing branch (issue #1930)

`step-04-setup-worktree` in `workflow-worktree.yaml` and `step3-setup-worktree`
in `consensus-issue-worktree.yaml` push a branch only when `origin` has no copy
of it yet. When `refs/remotes/origin/<branch>` exists, setup sets the upstream
to `origin/<branch>` and does not push. As a result, setup never runs the
repository's `pre-push` hook on a branch whose local head is ahead of its
remote, so a hook that rejects that head can no longer stop the run before any
work has been done.

**Affects:**
- `amplifier-bundle/recipes/workflow-worktree.yaml`: `step-04-setup-worktree`, on both the existing-branch path and the new, kept or attached branch path
- `amplifier-bundle/recipes/consensus-issue-worktree.yaml`: `step3-setup-worktree`, new-branch path

**Closes:** #1930

## Contents

- [Rule](#rule)
- [Behaviour by path](#behaviour-by-path)
- [Local-only existing branches](#local-only-existing-branches)
- [Messages](#messages)
- [Effect on the publish steps](#effect-on-the-publish-steps)
- [Hooks are never bypassed](#hooks-are-never-bypassed)
- [Testing](#testing)
- [Related](#related)

## Rule

Both recipes apply this rule:

| Remote-tracking ref `refs/remotes/origin/<B>` | Setup action |
|---|---|
| Exists | `git branch --set-upstream-to=origin/<B> <B>`, best effort (`\|\| true`). No push. |
| Absent, new-branch path | Plain `git push origin <B>` (workflow) or `git push -u origin <B>` (consensus), then set the upstream. Unchanged from before. |
| Absent, existing-branch path (workflow only) | No push. A warning goes to stderr. See [Local-only existing branches](#local-only-existing-branches). |

The check uses the full ref name with
`git show-ref --verify --quiet "refs/remotes/origin/<B>"`, so a tag or a short
SHA with the same name cannot match it. The branch name has already passed
`git check-ref-format --branch` before the check runs.

On the new-branch path the retained push is safe. The KEPT_AHEAD guard from
[issue #1565](issue-1565-rerun-preserves-worktree.md) runs before the push
and moves any branch with commits ahead of `BASE_REF` to a fresh branch, so
the head that setup pushes is the base commit. The pre-push hook therefore
checks only code that is already on the base branch.

The check reads the local remote-tracking ref and does not fetch. If
`origin/<B>` exists on the server but has not been fetched, setup treats it as
absent and pushes. A non-fast-forward push is rejected without force, and
`|| true` absorbs the rejection, as before.

## Behaviour by path

### `workflow-worktree.yaml` step-04, existing-branch path

This path runs when the caller sets `existing_branch` (for example, to resume
a PR branch). Before this change it ran
`git push origin <B>` unconditionally, which ran the pre-push hook on every
commit the local branch held beyond `origin/<B>`.

The push line is replaced by a guarded warning, and the upstream line is
unchanged, so the file keeps its line count:

```bash
[ -n "$LOCAL_BRANCH" ] || git -C "$WORKTREE_PATH" show-ref --verify --quiet "refs/remotes/origin/${BRANCH_NAME}" || echo "WARNING: branch '${BRANCH_NAME}' has no origin/${BRANCH_NAME}; setup does not push it (#1930), so publish will skip it until it is pushed." >&2
[ -n "$LOCAL_BRANCH" ] || git -C "$WORKTREE_PATH" branch --set-upstream-to="origin/${BRANCH_NAME}" "$BRANCH_NAME" > /dev/null 2>&1 || true
```

When `origin/<B>` exists, the first line prints nothing and the second sets
the upstream. When it is absent, the first line prints the warning and the
second fails quietly, so the branch has no upstream.

An alias worktree from [issue #1762](issue-1762-caller-branch-worktree-alias.md)
(`LOCAL_BRANCH` set) is skipped, as before; it already tracks `origin/<B>`.

The step output keeps `created=false`.

### `workflow-worktree.yaml` step-04, new, kept or attached branch path

The push is guarded by the remote-tracking ref:

```bash
git -C "${WORKTREE_PATH}" show-ref --verify --quiet "refs/remotes/origin/${BRANCH_NAME}" || git -C "${WORKTREE_PATH}" push origin "${BRANCH_NAME}" > /dev/null 2>&1 || true
git -C "${WORKTREE_PATH}" branch --set-upstream-to="origin/${BRANCH_NAME}" "${BRANCH_NAME}" > /dev/null 2>&1 || true
```

This covers the case where `origin/<B>` was left by an earlier run at an
older base commit, and `<B>` has since been recreated at the current base.
Setup sets the upstream and leaves `origin/<B>` where it is; the publish steps
reconcile the two later.

### `consensus-issue-worktree.yaml` step3

The push block has three branches:

```bash
if [ "$BOOTSTRAP" = "true" ]; then
  echo "BOOTSTRAP: Skipping push — no remote main branch available." >&2
elif git show-ref --verify --quiet "refs/remotes/origin/$BRANCH_NAME"; then
  git branch --set-upstream-to="origin/$BRANCH_NAME" "$BRANCH_NAME" >/dev/null 2>&1 || true
else
  git push -u origin "$BRANCH_NAME" > /dev/null 2>&1 || \
    echo "WARNING: Initial push of '$BRANCH_NAME' failed — will retry at PR creation." >&2
fi
```

The BOOTSTRAP line is unchanged. The existing-branch path of step3
([issue #342](../../amplifier-bundle/recipes/consensus-issue-worktree.yaml))
exits before this block and never pushed, so it is not affected.

The two recipes apply the same rule.

## Local-only existing branches

When `existing_branch=<B>` names a branch that exists locally but has no
`origin/<B>`, setup no longer pushes it. Such a branch holds commits that
`origin` does not have, which is the case issue #1930 describes: pushing it
would run the pre-push hook on unfinished work before the workflow has started.

Setup writes this line to stderr and continues:

```text
WARNING: branch 'feat/x' has no origin/feat/x; setup does not push it (#1930), so publish will skip it until it is pushed.
```

To publish such a branch through the workflow, push it once yourself before
the run, with hooks enabled:

```bash
git push -u origin feat/x
```

Then re-run with `-c existing_branch=feat/x`. Setup finds `origin/feat/x`,
sets the upstream and continues normally.

## Messages

| Message (stderr) | Recipe and path | Meaning |
|---|---|---|
| `WARNING: branch '<B>' has no origin/<B>; setup does not push it (#1930), so publish will skip it until it is pushed.` | workflow step-04, existing-branch path | Local-only existing branch. No push, no upstream. |
| `WARNING: Initial push of '<B>' failed — will retry at PR creation.` | consensus step3, new branch with no `origin/<B>` | Unchanged. The retained push failed. |
| `BOOTSTRAP: Skipping push — no remote main branch available.` | consensus step3 | Unchanged. |

Stdout still carries only the step's JSON object.

## Effect on the publish steps

`workflow-publish.yaml` step-15 and step-16 and `consensus-publish.yaml`
depend on the branch having an upstream (`@{u}`). No change was needed to
them:

- When `origin/<B>` exists, setup sets the upstream, so step-15 pushes the
  finished work normally and its pre-push hook checks that work.
- On the new-branch path with no `origin/<B>`, the retained push creates
  `origin/<B>` and the upstream.
- For a local-only existing branch, there is no upstream. Step-15 prints
  `WARNING: No upstream tracking branch configured — skipping push`, reports
  `reason="no-upstream"` and `pushed=false`, and step-16 opens no PR. Push the
  branch as shown in [Local-only existing branches](#local-only-existing-branches).

## Hooks are never bypassed

Neither recipe, the test nor CI uses `--no-verify`, `core.hooksPath`,
`HUSKY=0`, `SKIP=` or any other hook bypass, and no push is forced. Setup
avoids the hook by not pushing; every push that still happens runs the
repository's hooks.

## Testing

The regression test is
`amplifier-bundle/recipes/tests/test-issue-1930-setup-no-push-ahead.sh`. It
extracts the setup bash from each recipe and runs it against a scratch bare
`origin` and clone under `mktemp -d`. The clone's `.git/hooks/pre-push` touches
a marker file and exits with a chosen status. Git configuration, `HOME` and
`gh` are isolated, so a host-wide `core.hooksPath` cannot disable the hook.

| Case | Recipe and path | Set-up | Expected |
|---|---|---|---|
| A | workflow step-04, `existing_branch=feat/x` | `origin/feat/x` exists; local `feat/x` is one commit ahead; hook exits 1 | Exit 0, `created=false`, marker absent, `feat/x@{u}` is `origin/feat/x`, origin ref unchanged |
| B | workflow step-04, new-branch path | `origin/<B>` at an older base commit; `<B>` created at the current base; hook exits 1 | Exit 0, valid JSON, marker absent, upstream is `origin/<B>`, origin ref unchanged |
| C | consensus step3 | As B | Exit 0, `setup_complete: true`, marker absent, upstream set |
| D (control) | workflow step-04, new-branch path | No `origin/<B>`; hook exits 0 | Marker present and `git ls-remote origin <B>` is non-empty, which proves the retained push still runs |

Run it from the repository root:

```bash
bash amplifier-bundle/recipes/tests/test-issue-1930-setup-no-push-ahead.sh
```

`RECIPES_DIR` points the test at another copy of the recipes. To see cases A,
B and C fail against the recipes before this change:

```bash
D=$(mktemp -d)
git worktree add --detach "$D/pre-1930" <parent-commit>
RECIPES_DIR="$D/pre-1930/amplifier-bundle/recipes" \
  bash amplifier-bundle/recipes/tests/test-issue-1930-setup-no-push-ahead.sh
git worktree remove "$D/pre-1930"
rmdir "$D"
```

CI runs the test in `.github/workflows/ci.yml`, in the step after the
issue #1626 test.

## Related

- [Re-runs Never Destroy an Existing Worktree's Work (#1565, #1626)](issue-1565-rerun-preserves-worktree.md): the KEPT_AHEAD guard that keeps the retained push to the base commit
- [Caller Branch Worktree Alias (#1762)](issue-1762-caller-branch-worktree-alias.md): alias worktrees, which setup never pushes
- [Recipes index](README.md)
