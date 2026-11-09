# Caller Branch Continuation and Explicit Push (Issue #1969)

A `default-workflow` run started from a linked worktree on a feature branch
continues that branch instead of starting an unrelated one. Step 15 pushes to
`origin` with an explicit refspec, so a branch's configured upstream can no
longer send the work to another branch or block the push.

This page summarises what changed and what you see. The details are on the
pages linked from each section.

**Affects:** `workflow-worktree.yaml` step 4, `workflow-publish.yaml` steps 15
and 16b, `consensus-issue-worktree.yaml` step 3, and two new read-only helpers
in `amplifier-bundle/tools/`: `workflow_caller_branch.sh` and
`workflow_push_plan.sh`.

**Related issues:** #1762 (alias worktrees), #1930 and #1942 (step 4's setup
push), #1134 (nested runs), #858 (default-branch refusal).

---

## Contents

- [The problem](#the-problem)
- [What changed](#what-changed)
- [Choosing the branch a run uses](#choosing-the-branch-a-run-uses)
- [Step 15 refusals](#step-15-refusals)
- [Limitation: nested runs are recognised by depth](#limitation-nested-runs-are-recognised-by-depth)
- [Other push sites](#other-push-sites)
- [Related](#related)

---

## The problem

A run was started from inside a linked worktree on the branch
`claude/identity-delegation-receiver-model-2254`:

```bash
W=/repo/worktrees/claude/identity-delegation-receiver-model-2254
cd "$W" && amplihack recipe run smart-orchestrator \
  -c repo_path="$W" -c issue_number=197 -c force_single_workstream=true \
  --context-from-file task_description=task.md
```

Two things went wrong:

1. Step 4 ignored the caller's branch without saying so. It cut a new branch,
   `feat/issue-197-implement-the-delegation`, from `origin/dev`. With git's
   default `branch.autoSetupMerge=true`, that made `origin/dev` the new
   branch's upstream.
2. Step 4's best-effort setup push did not happen, so the upstream stayed
   `origin/dev`. Step 15's bare `git push` then failed under
   `push.default=simple` with exit 128, and nothing reached `origin`.

---

## What changed

| Where | Before | Now |
|---|---|---|
| Step 4 and consensus step 3, new branch | `git worktree add -b <b> -- <wt> origin/<base>`; upstream `origin/<base>` | `git worktree add --no-track -b <b> -- <wt> origin/<base>`; no upstream until the branch is pushed |
| Step 4, `repo_path` is a linked worktree on a feature branch | A new branch from the base, silently | The caller's branch is continued through the #1762 alias path, with an `INFO` line, or step 4 stops before creating anything |
| Step 15, destination | `@{u}`, through a bare `git push` | `git push -u origin "HEAD:refs/heads/<TARGET>"` |
| Step 15, comparison | Against `@{u}` | Against `refs/remotes/origin/<TARGET>`, classified by `workflow_push_plan.sh` after a fetch |
| Step 15, no upstream | Skipped with `reason="no-upstream"`; nothing pushed | A first push with `reason="pushed"` |
| Step 16b retry | `git pull --rebase && git push`, following `@{u}` except in an alias worktree | `rb` from the resolver, with `[ -n "$rb" ] \|\| exit 1`, then `git pull --rebase origin "$rb" && git push -u origin "HEAD:refs/heads/$rb"` |

`<TARGET>` is the branch on `origin`: the current branch, or `<B>` in a #1762
alias worktree `<B>--wt-<n>`.

Details:

- [Caller-branch continuation](issue-1762-caller-branch-worktree-alias.md#caller-branch-continuation-1969):
  when it applies, the messages, and when it stops.
- [New branches have no upstream until pushed](issue-1762-caller-branch-worktree-alias.md#new-branches-have-no-upstream-until-pushed-1969).
- [Detection reference: `workflow_caller_branch.sh`](issue-1762-caller-branch-worktree-alias.md#detection-reference-workflow_caller_branchsh).
- [Ancestry-aware step 15 publish](../features/workflow-publish-ancestry-aware-step15.md):
  the push target, the classification table and
  [`workflow_push_plan.sh`](../features/workflow-publish-ancestry-aware-step15.md#classification-helper-workflow_push_plansh).

---

## Choosing the branch a run uses

Step 4 picks the branch in this order. The first that applies wins.

| Precedence | Source | Result |
|---|---|---|
| 1 | `-c existing_branch=<B>` | `<B>` |
| 2 | `-c pr_number=<N>` | The PR's head branch |
| 3 | A `Branch: <name>` line in the task description (#1426) | `<name>`, existing or new |
| 4 | `repo_path` is a linked worktree on a non-default branch `<X>`, in a top-level run | `<X>`, continued through the alias path |
| 5 | Otherwise | A new branch from the base, created with `--no-track` |

Rule 4 is new. When it applies, step 4 prints:

```text
INFO: repo_path is a linked worktree on branch 'claude/identity-delegation-receiver-model-2254'; continuing caller branch 'claude/identity-delegation-receiver-model-2254' (#1969). The task worktree starts from origin/claude/identity-delegation-receiver-model-2254, so only commits pushed there are carried into it. To start a new branch instead, run from the main clone or name a branch with a 'Branch:' line.
```

The task worktree starts at `origin/<X>`. If `<X>` is not on `origin`, has
commits not on `origin/<X>`, or has uncommitted changes to tracked files, step
4 exits 1 before creating a branch or worktree, and names the fix:

```text
ERROR: step 4 was continuing the caller's branch 'claude/identity-delegation-receiver-model-2254' (#1969) and stopped before creating a branch or worktree. Fix the cause above and re-run, or re-run with -c existing_branch='claude/identity-delegation-receiver-model-2254' and -c repo_path=/repo.
```

To start a new branch from a linked worktree on a feature branch, run with
`-c repo_path=<main clone>`, or name a new branch with a `Branch:` line.

Rule 4 does not apply, and nothing changes, when `repo_path` is the main
clone, when HEAD is detached, or when the branch is a default branch (`main`,
`master`, the target of `refs/remotes/origin/HEAD`, or `develop` when that ref
does not resolve).

---

## Step 15 refusals

Step 15 checks the push target before it stages anything, in the order
below. The first check that fails ends the step with no commit and no push,
so it alone decides the exit status and the `reason`. The refusals that exit
1 write `commit_result` with `"pushed":"false"`:

| # | Cause | Message | Exit | `reason` |
|---|---|---|---|---|
| 1 | `workflow_remote_branch.sh` is missing or fails | Bash's `No such file or directory` with the resolver's path | `2` | (none) |
| 2 | Detached HEAD: there is no branch to push | `ERROR: cannot resolve the branch to push (detached HEAD?); refusing to push (#1969).` | `1` | `no-push-target` |
| 3 | The target differs from `worktree_setup.branch_name` | `ERROR: push target '<TARGET>' does not match worktree_setup.branch_name '<B>'; refusing to push (issue #1762).` | `1` | (none) |
| 4 | The target is the default branch | `ERROR: push target '<TARGET>' is the default branch; step 15 never pushes to it (#1969, #858).` | `1` | `default-branch-target` |
| 5 | `workflow_push_plan.sh` is missing | `ERROR: workflow push plan helper not found: <path> (#1969)` | `2` | (none) |

`no-push-target` and `default-branch-target` are the two new `reason` values.
The `no-upstream` reason no longer exists.

The default-branch refusal is a backstop. Step 4's detection uses local refs
only, so in a clone whose default branch is called `trunk` and whose
`refs/remotes/origin/HEAD` is unset, it cannot tell that `trunk` is the default
branch and may continue it. When the local ref is unset, and only then, step
15 asks `origin` for its `HEAD` with `git ls-remote --symref origin HEAD`, and
refuses the push. If that query fails, only `main` and `master` are
refused, and the step continues. Step 15's rule has no
`develop` fallback, unlike step 4's; see
[Default-branch backstop](../features/workflow-publish-ancestry-aware-step15.md#default-branch-backstop)
for how the query is bounded. To avoid reaching step 15 at all, run
`git remote set-head origin --auto` in the clone. Protect the default branch on
the server as well: the check does not replace branch protection.

The full list of step 15 outcomes, including `diverged-upstream` and
`push-plan-failed`, is in
[Ancestry-aware step 15 publish](../features/workflow-publish-ancestry-aware-step15.md#behaviour).

---

## Limitation: nested runs are recognised by depth

An `amplihack recipe run` started by an agent step inside another run usually
starts in the parent's task worktree, which is a linked worktree on a feature
branch. If rule 4 applied, the nested run would continue the parent's branch
while the parent is still committing to it.

Step 4 therefore applies rule 4 only to top-level runs, and decides this from
`AMPLIHACK_SESSION_DEPTH`, which the runner exports to every step:

| `AMPLIHACK_SESSION_DEPTH` | Treated as | Result |
|---|---|---|
| Unset, empty, or not a whole number | Top-level | Rule 4 applies |
| `0` or `1` | Top-level | Rule 4 applies |
| `2` or more | Nested | A new branch from the base, as under #1134 |

A nested run prints:

```text
INFO: nested run (depth 2); not continuing caller branch 'feat/parent' (#1969, #1134). Pass -c existing_branch='feat/parent' to continue it.
```

The check is only as good as the depth value. A nested run whose environment
does not raise `AMPLIHACK_SESSION_DEPTH` to 2 or more, for example one started
through a wrapper that clears the environment, is treated as top-level. It
then continues the parent's branch through an alias worktree, or step 4 stops
because the parent's worktree has uncommitted or unpushed work. In neither
case does it silently start an unrelated branch. To be certain that a nested
run gets its own branch, give its task description a `Branch:` line.

The depth value is checked as text before any use: anything that is not a
plain whole number, such as `a[$(touch PWNED)]`, is treated as unset and is
never evaluated.

---

## Other push sites

These still run a bare `git push` when `@{u}` is set, and are not changed by
#1969:

- `step-18c-push-feedback-changes` (`workflow-pr-review.yaml`) and
  `step-20b-push-cleanup` (`workflow-finalize.yaml`). Step 15's `-u` sets the
  correct upstream, `origin/<TARGET>`, earlier in the same run, including on a
  branch created before #1969 whose upstream was `origin/<base>`.
- `consensus-publish.yaml` and `consensus-pr-feedback.yaml`.

---

## Related

- [Caller branch worktree alias (#1762, #1969)](issue-1762-caller-branch-worktree-alias.md): the alias path that continuation reuses, its stops, and the helper references
- [Ancestry-aware step 15 publish (#978, #1969)](../features/workflow-publish-ancestry-aware-step15.md): the full step 15 contract
- [Recipe commit hook repair](../reference/recipe-commit-hook-repair.md): how step 15's push is retried after a pre-push hook rejects it
- [Relative-path-safe, idempotent worktree setup (#1121)](issue-1121-relative-repo-path.md): `wt_add_idempotent` and its `create` mode with `--no-track`
