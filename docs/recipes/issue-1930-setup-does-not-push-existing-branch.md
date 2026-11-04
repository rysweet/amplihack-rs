# Worktree Setup Does Not Push an Existing Branch (Issue #1930)

When a run targets an existing branch, through `existing_branch` or
`pr_number`, `step-04-setup-worktree` attaches the task worktree to that branch
and sets its upstream to `origin/<branch>`. It does not push. The repository's
pre-push hooks therefore do not run during setup, and a local head that fails
them no longer holds up the run before any work starts.

**Affects:**
- `amplifier-bundle/recipes/workflow-worktree.yaml`: `step-04-setup-worktree` (existing-branch path)
- `amplifier-bundle/recipes/consensus-issue-worktree.yaml`: `step3-setup-worktree` (MIRROR comment only; this path has never pushed)
- `amplifier-bundle/recipes/tests/test-issue-1930-setup-does-not-push-ahead-branch.sh` (new)
- `.github/workflows/ci.yml`: one step that runs the new test

**Closes:** #1930

---

## Contents

- [Quick start](#quick-start)
- [The problem](#the-problem)
- [What the existing-branch path runs](#what-the-existing-branch-path-runs)
- [Where commits are pushed instead](#where-commits-are-pushed-instead)
- [Known limitation: a branch that exists only locally](#known-limitation-a-branch-that-exists-only-locally)
- [What is unchanged](#what-is-unchanged)
- [Hooks are never bypassed](#hooks-are-never-bypassed)
- [Testing](#testing)
- [Related](#related)

---

## Quick start

Nothing needs configuring. Resume work on a branch that already has a local
commit:

```bash
amplihack recipe run default-workflow \
  -c task_description="Fix the failing recipe test on PR #1896" \
  -c existing_branch=feat/issue-1887-split-recipe \
  -c repo_path="$(pwd)"
```

Step 4 finishes in seconds and prints:

```text
INFO: Targeting existing branch 'feat/issue-1887-split-recipe' (issue #342 path).

=== Worktree Setup Complete (existing-branch path) ===
```

No pre-push hook runs, and `origin/feat/issue-1887-split-recipe` is unchanged.
The worktree's branch tracks it:

```bash
git -C worktrees/feat/issue-1887-split-recipe rev-parse --abbrev-ref '@{u}'
# Output: origin/feat/issue-1887-split-recipe
```

The same applies when the run is started with `-c pr_number=1896`.

---

## The problem

Before #1930 the existing-branch path ended with:

```bash
[ -n "$LOCAL_BRANCH" ] || git -C "$WORKTREE_PATH" push origin "$BRANCH_NAME" > /dev/null 2>&1 || true
```

A push runs the repository's pre-push hooks. In amplihack-rs these run clippy
and `cargo test --workspace`. A resumed run usually targets a branch whose
local head fails those hooks, because that failure is why the run was resumed.
The push therefore took a cargo build slot, often for more than an hour under
load, and its result was discarded by `> /dev/null 2>&1 || true`. In one case
a resumed run for PR #1896 waited 1 h 20 min in this push before it was
stopped by hand. The implementation steps meant to fix the failure could not
start until it finished.

---

## What the existing-branch path runs

After the branch name is resolved and passes `git check-ref-format --branch`,
the path runs only these git commands, in this order:

| Order | Command | Purpose |
|---|---|---|
| 1 | `git fetch origin "+refs/heads/<B>:refs/remotes/origin/<B>"` | Creates or refreshes `origin/<B>` so a remote-only branch resolves. Best-effort. |
| 2 | `git worktree list --porcelain` | Finds a worktree already on `<B>` (reused) or the caller checkout (alias route, #1762). |
| 3 | `git worktree add` (through `wt_add_idempotent attach` or `track`, or the #1762 alias) | Attaches the task worktree. |
| 4 | `git branch --set-upstream-to="origin/<B>" "<B>"` | Sets the upstream. Skipped for an alias, which already tracks `origin/<B>`. Best-effort. |

The step's code reads:

```bash
# Upstream only, never a push (#1930): a push here would run the hooks on the local head. Never for an alias.
[ -n "$LOCAL_BRANCH" ] || git -C "$WORKTREE_PATH" branch --set-upstream-to="origin/${BRANCH_NAME}" "$BRANCH_NAME" > /dev/null 2>&1 || true
```

Neither command contacts `origin` for writing, so the path needs no push
credentials. Local commits on `<B>` stay local until a later step pushes them.

The output contract is unchanged: `created=false`, the physical worktree path,
and the `=== Worktree Setup Complete (existing-branch path) ===` marker on
stderr.

---

## Where commits are pushed instead

Commits leave the worktree only through the publish and review steps, which
run the hooks in the open and report a failure rather than discarding it:

| Step | Recipe | Push |
|---|---|---|
| `step-15-commit-push` | `workflow-publish.yaml` | Pushes to the upstream once the run has committed its work. Fails closed on divergence (#978). |
| `step-18c-push-feedback-changes` | `workflow-pr-review.yaml` | Pushes review fixes with `-u`. |
| `step-20b-push-cleanup` | `workflow-finalize.yaml` | Pushes cleanup commits with `-u`. |

By the time step 15 pushes, the implementation steps have had the chance to fix
the commit that would have failed the hooks during setup.

---

## Known limitation: a branch that exists only locally

If `<B>` exists locally but not on `origin`, the fetch finds nothing,
`origin/<B>` does not exist and the set-upstream command fails without a
message. Setup still completes. Later:

- `step-15-commit-push` prints
  `WARNING: No upstream tracking branch configured — skipping push` and records
  `reason="no-upstream"`;
- `step-16` then has no remote branch and does not create a pull request;
- only `step-18c` and `step-20b`, which push with `-u`, would create the remote
  branch.

Setup does not push in this case either. A conditional push for local-only
branches would run the hooks on the same local commits, which is the defect
#1930 removes. To publish a local-only branch, push it once yourself before
the run, with hooks enabled:

```bash
git push -u origin feat/issue-1887-split-recipe
```

---

## What is unchanged

- **New-branch path.** When the run derives a fresh branch, step 4 still runs
  `git push origin "<B>"` and sets the upstream (consensus step3 runs
  `git push -u`). The fresh branch starts at `BASE_REF` and holds no commits
  of its own, and a deconflicted run pushes only its own new branch (#1565), so
  this push never carries work that is ahead of the base.
- **Alias route (#1762).** When the caller checkout is on `<B>`, the task
  worktree uses `<B>--wt-<n>`, which already tracks `origin/<B>`. Neither the
  push (before) nor the set-upstream (now) ran on this route.
- **consensus-issue-worktree.yaml step3.** Its existing-branch path has never
  pushed. Its MIRROR comment now names
  `workflow-worktree.yaml step-04-setup-worktree` and records that this path
  does not push. The MIRROR comment in step-04 names
  `consensus-issue-worktree.yaml step3-setup-worktree`.
- **Validation.** The `git check-ref-format --branch` gate, the #858 refusals
  and the `pr_number` integer check are unchanged.
- **Line budget.** `workflow-worktree.yaml` is 398 lines and
  `consensus-issue-worktree.yaml` is 396, both under the 400-line brick limit
  checked by `scripts/check-brick-budget.sh` and
  `default_workflow_decomposition_test`.

---

## Hooks are never bypassed

The fix removes a push; it does not weaken one. No recipe, tool or test uses
`--no-verify`, `core.hooksPath`, `HUSKY=0`, `SKIP=` or any other way to skip
hooks, and no push is forced. Every push that still happens runs the
repository's pre-push hooks.

---

## Testing

`amplifier-bundle/recipes/tests/test-issue-1930-setup-does-not-push-ahead-branch.sh`
extracts the real body of `step-04-setup-worktree` from the YAML and runs it
under `env -i` against a scratch repository. Git configuration is isolated with
`GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1` and
`GIT_TERMINAL_PROMPT=0`.

**Fixture.** A bare `origin.git` under the scratch directory has `main` at
commit C0 and `feat/x` at commit A, one commit after C0. In the clone, the
caller stays on `main` and the local `feat/x` is at B, one commit ahead of A,
and not checked out anywhere. The clone's `.git/hooks/pre-push` writes
`$SCRATCH/pre-push-ran` and exits 1.

| Case | Input | Assertions |
|---|---|---|
| 1 | `EXISTING_BRANCH=feat/x` | Step exits 0; stderr has the existing-branch completion marker; `pre-push-ran` does not exist; `feat/x@{u}` is `origin/feat/x`; local `feat/x` is still B; `origin`'s `refs/heads/feat/x` is still A. |
| 2 | `PR_NUMBER=7`, with a stub `gh` that answers only `pr view 7 --json headRefName -q .headRefName` with `feat/x` | The same six assertions, on a fresh fixture. |

On the recipe before #1930 the test fails at the third assertion: the push ran
the hook, which wrote the marker. The step still exited 0 because of
`|| true`, so the exit code alone did not show the defect.

Run it locally:

```bash
bash amplifier-bundle/recipes/tests/test-issue-1930-setup-does-not-push-ahead-branch.sh
```

The exit code is 0 for a pass, 1 for a failed assertion and 2 for a harness
error. CI runs the same command in `.github/workflows/ci.yml`, after the #1565
and #1626 recipe tests.

---

## Related

- [Re-runs Never Destroy an Existing Worktree's Work (#1565, #1626)](issue-1565-rerun-preserves-worktree.md): the new-branch push and deconfliction
- [Caller Branch Worktree Alias (#1762)](issue-1762-caller-branch-worktree-alias.md): the alias route on the existing-branch path
- [Recover an Existing PR with default-workflow](../howto/recover-existing-pr-with-default-workflow.md): starting a run with `pr_number`
- [Recipe Runner](README.md): index of recipe documentation
