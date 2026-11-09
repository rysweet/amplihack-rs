# Caller Branch Continuation and Explicit Push (Issue #1969)

A `default-workflow` run started from a linked worktree on a feature branch
continues that branch instead of starting an unrelated one. Step 15 pushes to
`origin` with an explicit refspec, so a branch's configured upstream can no
longer send the work to another branch or block the push.

This page is the reference for caller-branch continuation, the no-upstream
rule for new branches and the shared default-branch rule. The alias worktree
that continuation uses is described in
[Caller branch worktree alias (#1762)](issue-1762-caller-branch-worktree-alias.md),
and the full step 15 contract in
[Ancestry-aware step 15 publish](../features/workflow-publish-ancestry-aware-step15.md).

**Affects:** `workflow-worktree.yaml` step 4, `workflow-publish.yaml` steps 15
and 16b, `consensus-issue-worktree.yaml` step 3, and three read-only helpers
in `amplifier-bundle/tools/`: `workflow_caller_branch.sh`,
`workflow_push_plan.sh` and `workflow_default_branch.sh`.

**Related issues:** #1762 (alias worktrees), #1930 and #1942 (step 4's setup
push), #1134 (nested runs), #858 (the caller checkout is never the task
worktree), #1565 (continuing a kept worktree), #1833 (Gate B reuse of a
checkout outside `<main>/worktrees/`).

---

## Contents

- [The problem](#the-problem)
- [What changed](#what-changed)
- [Choosing the branch a run uses](#choosing-the-branch-a-run-uses)
- [Caller-branch continuation](#caller-branch-continuation)
  - [Example](#example)
  - [When step 4 continues the caller's branch](#when-step-4-continues-the-callers-branch)
  - [What reaches the task worktree](#what-reaches-the-task-worktree)
  - [When continuation stops](#when-continuation-stops)
  - [Why the stop hint does not suggest existing_branch](#why-the-stop-hint-does-not-suggest-existing_branch)
  - [Runs from an alias worktree](#runs-from-an-alias-worktree)
  - [Starting a new branch instead](#starting-a-new-branch-instead)
- [Run depth](#run-depth)
- [New branches have no upstream until pushed](#new-branches-have-no-upstream-until-pushed)
- [The default-branch rule: `workflow_default_branch.sh`](#the-default-branch-rule-workflow_default_branchsh)
- [Step 15 refusals](#step-15-refusals)
- [Messages](#messages)
- [Detection reference: `workflow_caller_branch.sh`](#detection-reference-workflow_caller_branchsh)
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
| Default-branch rule | Copied into three helpers, with a fourth variant in step 15 | One helper, `workflow_default_branch.sh`, run by all four |

`<TARGET>` is the branch on `origin`: the current branch, or `<B>` in a #1762
alias worktree `<B>--wt-<n>`.

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

Rule 4 is new and is described in the next section.

---

## Caller-branch continuation

A top-level run whose `repo_path` is a linked worktree on its own feature
branch continues that branch. Step 4 treats the caller's branch as if
`-c existing_branch=<X>` had been given and takes the
[alias path](issue-1762-caller-branch-worktree-alias.md#when-the-alias-applies).
The work is committed on `<X>--wt-<n>` and pushed to `origin/<X>`.

### Example

Suppose `/repo/worktrees/claude/identity-delegation-receiver-model-2254` is a
linked worktree of `/repo` on the branch
`claude/identity-delegation-receiver-model-2254`. The branch is pushed, and no
tracked file has uncommitted changes. The command under
[The problem](#the-problem) now prints, on step 4's stderr:

```text
INFO: repo_path is a linked worktree on branch 'claude/identity-delegation-receiver-model-2254'; continuing caller branch 'claude/identity-delegation-receiver-model-2254' (#1969). The task worktree starts from origin/claude/identity-delegation-receiver-model-2254, so only commits pushed there are carried into it. To start a new branch instead, run from the main clone or name a branch with a 'Branch:' line.
INFO: Targeting existing branch 'claude/identity-delegation-receiver-model-2254' (issue #342 path).
INFO: branch 'claude/identity-delegation-receiver-model-2254' is checked out in the caller repository at /repo/worktrees/claude/identity-delegation-receiver-model-2254; the task worktree uses local branch 'claude/identity-delegation-receiver-model-2254--wt-1' with upstream origin/claude/identity-delegation-receiver-model-2254 (issue #1762).
```

The step output has the same keys as any alias run:

```json
{"worktree_path": "/repo/worktrees/claude/identity-delegation-receiver-model-2254--wt-1", "branch_name": "claude/identity-delegation-receiver-model-2254", "base_ref": "", "base_branch": "", "created": false, "local_branch_reason": "branch_name is checked out in the caller repository (issue #1762)", "local_branch": "claude/identity-delegation-receiver-model-2254--wt-1"}
```

Step 15 pushes the run's commits with
`git push -u origin "HEAD:refs/heads/claude/identity-delegation-receiver-model-2254"`.
The caller's worktree keeps its HEAD, index and working tree. To bring the
run's commits into it, run `git pull --ff-only` there after the run.

### When step 4 continues the caller's branch

All of the following must hold:

1. The run names no branch: `existing_branch` and `pr_number` are empty, and
   the task description has no `Branch:` line (#1426). A `Branch:` line naming
   a branch that does not exist yet also counts as naming a branch.
2. `repo_path` is inside a linked worktree, not the main clone.
3. That worktree's HEAD is on a branch, not detached.
4. The branch is not a default branch, as
   [`workflow_default_branch.sh`](#the-default-branch-rule-workflow_default_branchsh)
   decides.
5. The run is a top-level run. See [Run depth](#run-depth).
6. The branch does not have the #1762 alias form `<B>--wt-<n>`. A top-level
   run from an alias worktree stops instead; see
   [Runs from an alias worktree](#runs-from-an-alias-worktree).

In every other case step 4 behaves as before:

| Situation | Result |
|---|---|
| `existing_branch`, `pr_number` or a `Branch:` line is given | That branch is used. Detection does not run. |
| `repo_path` is the main clone, on any branch | A new branch from the base. |
| `repo_path` is a linked worktree on the default branch | A new branch from the base. |
| `repo_path` is a linked worktree with a detached HEAD | A new branch from the base. |
| Nested run (depth 2 or more) | A new branch from the base, and the nested-run `INFO` line. |
| Top-level run in a #1762 alias worktree (`<B>--wt-<n>`) | Step 4 stops before creating anything. See [Runs from an alias worktree](#runs-from-an-alias-worktree). |

### What reaches the task worktree

The alias worktree starts at `origin/<X>`. The planner's
[three checks](issue-1762-caller-branch-worktree-alias.md#checks-on-the-callers-checkout)
apply to the caller's worktree as usual:

- Uncommitted changes to tracked files stop the run.
- Untracked files do not stop the run. They stay in the caller's worktree and
  the task worktree does not see them.
- Commits on `<X>` that are not on `origin/<X>` stop the run.
- A caller that is behind `origin/<X>` is accepted. The alias starts at
  `origin/<X>`, which already has the caller's commits.

### When continuation stops

When step 4 has chosen to continue the caller's branch and anything fails
before the task worktree exists, step 4 exits 1 with nothing on stdout and
creates no branch and no worktree. It never falls back to a new branch. After
the cause, which is usually one of the planner's
[stops](issue-1762-caller-branch-worktree-alias.md#stops), it prints:

```text
ERROR: step 4 was continuing the caller's branch '<X>' (#1969) and stopped before creating a branch or worktree. Fix the cause above in that worktree and re-run from it, or start a new branch by re-running with -c repo_path=<main clone>. Do not name the branch with -c existing_branch instead: that reuses the caller's worktree as it stands, and step 15 commits and pushes whatever is in it (#858).
```

`<main clone>` is the path of the main repository, quoted for the shell when
it needs quoting. The same line follows every stop reason. For a branch that
was never pushed, the full stderr is:

```text
INFO: repo_path is a linked worktree on branch 'claude/identity-delegation-receiver-model-2254'; continuing caller branch 'claude/identity-delegation-receiver-model-2254' (#1969). ...
INFO: Targeting existing branch 'claude/identity-delegation-receiver-model-2254' (issue #342 path).
ERROR: branch 'claude/identity-delegation-receiver-model-2254' is checked out in the caller repository at /repo/worktrees/claude/identity-delegation-receiver-model-2254 but origin/claude/identity-delegation-receiver-model-2254 does not exist. The task worktree tracks origin/claude/identity-delegation-receiver-model-2254 and would not see the branch's commits. Push the branch with 'git push -u origin claude/identity-delegation-receiver-model-2254', or work from a checkout that is not on 'claude/identity-delegation-receiver-model-2254', then re-run (issue #1762).
ERROR: cannot set up an alias worktree for 'claude/identity-delegation-receiver-model-2254' (planner exit 1; issue #1762).
ERROR: step 4 was continuing the caller's branch 'claude/identity-delegation-receiver-model-2254' (#1969) and stopped before creating a branch or worktree. Fix the cause above in that worktree and re-run from it, or start a new branch by re-running with -c repo_path=/repo. Do not name the branch with -c existing_branch instead: that reuses the caller's worktree as it stands, and step 15 commits and pushes whatever is in it (#858).
```

The two routes the hint offers are both safe to follow as printed:

- **Fix the cause and re-run from the same worktree.** Commit and push, push,
  or discard, as the planner's message says. The re-run passes the planner's
  checks again and gets an alias worktree.
- **Start a new branch.** Re-run with `-c repo_path=<main clone>`. Detection
  does not apply to the main clone, so step 4 creates a new branch from the
  base in a new worktree. The caller's worktree, its branch and `origin/<X>`
  are not touched.

Step 4 also exits 1 before creating anything when the detection helper itself
fails (exit 2, or 4 or more), for example on a branch name that
`git check-ref-format --branch` does not print back unchanged:

```text
ERROR: caller-branch detection failed (workflow_caller_branch.sh exit 2; see above) (#1969).
```

### Why the stop hint does not suggest existing_branch

Running from the main clone with `-c existing_branch='<X>'` while the
caller's worktree is on `<X>` skips the planner's checks. The caller's
worktree is then not the run's caller, so Gate B in step 4 reuses it as the
task worktree in place (`INFO: Reusing existing worktree at '<path>'.`).
Step 15 then runs `stage_workflow_changes`, which is `git add -A` without
build artefacts, in that worktree. Uncommitted changes and untracked files in
the caller's worktree would be committed and pushed to `origin/<X>`, and the
run's agents would edit files in a checkout another session may still be
using.

Gate B is not changed to refuse a dirty worktree, because reusing a worktree
in place with its changes is how a kept worktree is continued on purpose:
[#1565](issue-1565-rerun-preserves-worktree.md) tells the user to re-run with
`existing_branch` for exactly that. Refusing to reuse a checkout of `<X>`
outside `<main>/worktrees/` is tracked in
[#1833](https://github.com/rysweet/amplihack-rs/issues/1833). Until then the
step 4 stop hint does not offer the route, and the
`existing_branch_from_main_clone_reuses_the_caller_worktree_in_place` test
records why.

### Runs from an alias worktree

#1762 keeps alias worktrees at `<main>/worktrees/<B>--wt-<n>`, on local
branch `<B>--wt-<n>` tracking `origin/<B>`. An alias publishes to
`origin/<B>` and is never pushed as a branch of its own. A user who `cd`s into
a kept alias task worktree and starts a run there is in a linked worktree on
a feature branch, so rules 1 to 5 would continue `<B>--wt-<n>` as a separate
branch. The planner would then ask for `origin/<B>--wt-<n>` and print a hint
to push the alias, and following it would put a second branch, and later a
second PR, on origin for the same work.

Continuing `<B>` from inside the alias is not done either. The planner's
reuse pass would pick this same alias worktree as the task worktree, which is
the #858 hazard: the run would edit and commit in the checkout it was started
from.

So a top-level run whose branch has the alias form stops before creating
anything. The form is the one
[`workflow_remote_branch.sh`](issue-1762-caller-branch-worktree-alias.md)
maps back to `<B>`: the last `--wt-`, then a number matching
`^[1-9][0-9]*$`, after a non-empty `<B>`. The upstream configuration is not
needed, so an alias whose upstream was removed stops too. Names such as
`feat/x--wt-0`, `feat/x--wt-01` or `feat/x--wt-x` are ordinary branches and
are continued. Step 4 prints, for an alias of a branch checked out in another
linked worktree:

```text
ERROR: repo_path is a #1762 alias worktree: branch 'feat/x--wt-1' is the alias form <branch>--wt-<n> of 'feat/x'. An alias worktree publishes to origin/feat/x and is never pushed or continued as a branch of its own, so step 4 is not continuing it and creates nothing (#1969).
ERROR: To continue 'feat/x', re-run from '/repo/worktrees/feat/x', which has 'feat/x' checked out. To start a new branch, re-run with -c repo_path='/repo'. Commits made here that are not on origin/feat/x belong there: "git push origin 'HEAD:refs/heads/feat/x'" (#1969, #1762).
```

The route to continuing `<B>` depends on where `<B>` is checked out:

| `<B>` is checked out in | The hint says | Why it is safe |
|---|---|---|
| Another linked worktree | `re-run from '<path>', which has '<B>' checked out` | That worktree is then the caller, so detection continues `<B>` through the planner's checks, which reuse or create an alias. |
| The main clone | `re-run from the main clone with -c repo_path='<main>' -c existing_branch='<B>'` | The main clone is then the caller and is on `<B>`, so Gate B sends the run to the planner. This is #1762's own route; Gate B reuses a worktree in place only when it is not the caller's. |
| No worktree | `check '<B>' out in a linked worktree and re-run from there` | As the first row. |

No line names the alias as something to push. Unpushed commits made in the
alias are published to `origin/<B>` with the printed
`git push origin 'HEAD:refs/heads/<B>'`; until then the planner's reuse
check refuses to reuse the alias, so they are not lost. Branch names and
paths in both lines are single-quoted for the shell.

The stop comes after the depth check. A nested run started in a parent's
alias task worktree keeps the #1134 behaviour and starts a new branch.

### Starting a new branch instead

To start a new branch from the base when you are in a linked worktree on a
feature branch, do one of the following:

- run with `-c repo_path=<main clone>`; step 4 anchors its worktrees at the
  main clone in either case (#1134), so nothing else changes;
- put a `Branch: <new-branch>` line in the task description (#1426).

---

## Run depth

An `amplihack recipe run` started by an agent step inside another run usually
starts in the parent's task worktree, which is a linked worktree on a feature
branch. If rule 4 applied, the nested run would continue the parent's branch
while the parent is still committing to it.

Step 4 therefore applies rule 4 only to top-level runs. The recipe runner
exports `AMPLIHACK_SESSION_DEPTH` to every step. A run started from a shell
sees `1` in its steps. A recipe that another recipe runs in the same process,
as `smart-orchestrator` runs `default-workflow`, also sees `1`. An
`amplihack recipe run` started by an agent step inside another run sees `2` or
more.

| `AMPLIHACK_SESSION_DEPTH` | Treated as | Result |
|---|---|---|
| Unset, empty, or not a whole number | Top-level | Rule 4 applies |
| `0` or `1` | Top-level | Rule 4 applies |
| `2` or more | Nested | A new branch from the base, as under #1134 |

A nested run prints:

```text
INFO: nested run (depth 2); not continuing caller branch 'feat/parent' (#1969, #1134). Pass -c existing_branch='feat/parent' to continue it.
```

That hint is run from the same `repo_path`, the parent's worktree, which is
then the run's caller, so the re-run goes through the planner's checks and an
alias worktree. It is not the main-clone route described above.

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

## New branches have no upstream until pushed

When step 4 creates a branch from the base, it uses `--no-track`:

```bash
git worktree add --no-track -b "<branch>" -- "<wt>" "origin/<base>"
```

`consensus-issue-worktree.yaml` `step3-setup-worktree` does the same. Without
`--no-track`, git's default `branch.autoSetupMerge=true` made `origin/<base>`
the new branch's upstream, and step 15's bare `git push` then failed with exit
128 under `push.default=simple`.

The upstream is now set only by a push to the branch's own name:

| Step | Upstream afterwards |
|---|---|
| Step 4 creates the branch | None (`branch.<branch>.merge` is unset) |
| Step 4's best-effort setup push succeeds | `origin/<branch>` |
| Step 4's setup push fails or is skipped | None |
| Step 15 pushes | `origin/<branch>`, from `git push -u origin "HEAD:refs/heads/<branch>"` |

Step 15 does not depend on the upstream: it pushes to `origin/<TARGET>` with an
explicit refspec and compares against `refs/remotes/origin/<TARGET>`. A missing
`origin/<TARGET>` is a first push, not a skip.

Alias branches are unchanged. They are still created with
`git worktree add --track -b "<B>--wt-<n>" … "origin/<B>"`, because their
upstream is how the resolver maps them back to `<B>`.

---

## The default-branch rule: `workflow_default_branch.sh`

```text
usage: workflow_default_branch.sh [--ask-origin] <dir> <branch>
```

This is the only copy of the rule. Its four callers run it and carry no copy
of their own:

| Caller | Mode | A default branch means |
|---|---|---|
| `workflow_caller_branch.sh` (step 4 detection) | Read-only | The caller's branch is not continued |
| `workflow_caller_branch_alias.sh` (step 4 planner) | Read-only | No alias; step 4 gives the #858 refusal |
| `workflow_remote_branch.sh` (resolver) | Read-only | An alias-shaped name with that prefix is not an alias |
| `workflow-publish.yaml` step 15 | `--ask-origin` | The push is refused with `reason="default-branch-target"` |

`<branch>` is a default branch when any of these holds, checked in order:

1. it is `main` or `master`;
2. it is the target of `refs/remotes/origin/HEAD` in `<dir>`;
3. `refs/remotes/origin/HEAD` does not resolve to a commit, and
   - with `--ask-origin`, `origin` answers `git ls-remote --symref origin HEAD`
     with a valid branch name and `<branch>` is that name;
   - without `--ask-origin`, or when `origin` gives no valid answer,
     `<branch>` is `develop`, which step 4 can fall back to as the base when
     `origin/HEAD` is unknown.

The two modes differ only in where they learn `origin`'s HEAD when the local
ref does not resolve. The read-only mode never touches the network and
guesses `develop`; `--ask-origin` replaces the guess with `origin`'s answer.
A resolvable local `refs/remotes/origin/HEAD` is trusted in both modes, so
`origin` is not asked then.

| Exit | Meaning | Callers then |
|---|---|---|
| 0 | A default branch | As in the table above |
| 1 | Not a default branch | Carry on |
| 2 | Usage error: wrong arguments, a missing directory or an empty branch | Fail closed: the detection helper and the resolver exit 2, the planner stops with exit 1, step 15 exits 2 before staging |

A missing helper is treated like exit 2 by every caller, never as "not a
default branch". It only reads (`symbolic-ref`, `rev-parse`,
`check-ref-format`, and `ls-remote` with `--ask-origin`). The `ls-remote`
query runs with `GIT_TERMINAL_PROMPT=0` and its stderr is discarded, so a
remote URL is never printed. The `every_caller_agrees_on_default_branches`
test runs one table through the helper and all four callers, and
`default_branch_rule_has_one_copy` checks that no caller reads
`refs/remotes/origin/HEAD`, names `main|master` or guesses `develop` itself.

---

## Step 15 refusals

Step 15 checks the push target before it stages anything, in the order
below. The first check that fails ends the step with no commit and no push,
so it alone decides the exit status and the `reason`. The refusals that exit
1 write `commit_result` with `"pushed":"false"`:

| # | Cause | Message | Exit | `reason` |
|---|---|---|---|---|
| 1 | `workflow_remote_branch.sh` is missing or fails | Bash's `No such file or directory` with the resolver's path, or the resolver's own `ERROR` line | `2` | (none) |
| 2 | Detached HEAD: there is no branch to push | `ERROR: cannot resolve the branch to push (detached HEAD?); refusing to push (#1969).` | `1` | `no-push-target` |
| 3 | The target differs from `worktree_setup.branch_name` | `ERROR: push target '<TARGET>' does not match worktree_setup.branch_name '<B>'; refusing to push (issue #1762).` | `1` | (none) |
| 4 | The target is the default branch | `ERROR: push target '<TARGET>' is the default branch; step 15 never pushes to it (#1969, #858).` | `1` | `default-branch-target` |
| 4 | `workflow_default_branch.sh` is missing or fails | `ERROR: cannot decide whether push target '<TARGET>' is the default branch (workflow_default_branch.sh exit <rc>); refusing to push (#1969).` | `2` | (none) |
| 5 | `workflow_push_plan.sh` is missing | `ERROR: workflow push plan helper not found: <path> (#1969)` | `2` | (none) |

`no-push-target` and `default-branch-target` are the two new `reason` values.
The `no-upstream` reason no longer exists.

The default-branch refusal is a backstop. Step 4's detection uses local refs
only, so in a clone whose default branch is called `trunk` and whose
`refs/remotes/origin/HEAD` is unset, it cannot tell that `trunk` is the default
branch and may continue it. Step 15 runs the same rule with `--ask-origin`,
learns `trunk` from `origin` and refuses the push. To avoid reaching step 15
at all, run `git remote set-head origin --auto` in the clone. Protect the
default branch on the server as well: the check does not replace branch
protection. See
[Default-branch backstop](../features/workflow-publish-ancestry-aware-step15.md#default-branch-backstop)
for how the query is bounded.

The full list of step 15 outcomes, including `diverged-upstream` and
`push-plan-failed`, is in
[Ancestry-aware step 15 publish](../features/workflow-publish-ancestry-aware-step15.md#behaviour).

---

## Messages

These messages come from step 4's detection of the caller's branch. Each
names the branch and `#1969`, and goes to stderr. The nested-run hint
single-quotes the branch name, so it can be pasted into a shell as it stands.

| When | Message |
|---|---|
| Continuing the caller's branch | `INFO: repo_path is a linked worktree on branch '<X>'; continuing caller branch '<X>' (#1969). The task worktree starts from origin/<X>, so only commits pushed there are carried into it. To start a new branch instead, run from the main clone or name a branch with a 'Branch:' line.` |
| Nested run, branch not continued | `INFO: nested run (depth <N>); not continuing caller branch '<X>' (#1969, #1134). Pass -c existing_branch='<X>' to continue it.` |
| Continuation stopped before creating anything | `ERROR: step 4 was continuing the caller's branch '<X>' (#1969) and stopped before creating a branch or worktree. Fix the cause above in that worktree and re-run from it, or start a new branch by re-running with -c repo_path=<main clone>. Do not name the branch with -c existing_branch instead: that reuses the caller's worktree as it stands, and step 15 commits and pushes whatever is in it (#858).` |
| `repo_path` is a #1762 alias worktree | `ERROR: repo_path is a #1762 alias worktree: branch '<B>--wt-<n>' is the alias form <branch>--wt-<n> of '<B>'. ...`, then `ERROR: To continue '<B>', <route>. To start a new branch, re-run with -c repo_path='<main clone>'. Commits made here that are not on origin/<B> belong there: "git push origin 'HEAD:refs/heads/<B>'" (#1969, #1762).` See [Runs from an alias worktree](#runs-from-an-alias-worktree). |
| The detection helper failed | `ERROR: caller-branch detection failed (workflow_caller_branch.sh exit <rc>; see above) (#1969).` |
| The detection helper is missing | `WARNING: workflow_caller_branch.sh not found; caller-branch continuation (#1969) disabled. Pass -c existing_branch=<branch> to continue a caller branch.` |
| The default-branch rule cannot decide | `ERROR: cannot decide whether '<X>' is the default branch (workflow_default_branch.sh exit <rc>) (#1969).`, then the `caller-branch detection failed` line |

The planner's own stop messages are listed under
[Stops](issue-1762-caller-branch-worktree-alias.md#stops).

---

## Detection reference: `workflow_caller_branch.sh`

```text
usage: workflow_caller_branch.sh <repo_path>
```

Step 4 calls it after the #1426 check, and only when `existing_branch`,
`pr_number` and the `Branch:` line are all empty:

```bash
bash "$TOOLS_DIR/workflow_caller_branch.sh" "$REPO_PATH"
```

It runs these checks in order and stops at the first that decides:

| # | Check | Result |
|---|---|---|
| 0 | Not exactly one argument, or `repo_path` is not a directory inside a git repository (`git -C <repo_path> rev-parse` fails) | Exit 2, with an `ERROR` line on stderr and nothing on stdout |
| 1 | `repo_path` is the main clone: its absolute git directory equals the common git directory, compared after `cd -P` | Exit 1 |
| 2 | HEAD is detached | Exit 1 |
| 3 | `git check-ref-format --branch <X>` does not print `<X>` back unchanged | Exit 2 |
| 4 | `<X>` is a default branch (`workflow_default_branch.sh` exits 0) | Exit 1; the rule helper failing or missing is exit 2 |
| 5 | `AMPLIHACK_SESSION_DEPTH` is a whole number and is 2 or more | Exit 1, with the nested-run `INFO` line |
| 6 | `<X>` has the #1762 alias form `<B>--wt-<n>` | Exit 3, with the two [alias stop](#runs-from-an-alias-worktree) `ERROR` lines and nothing on stdout |
| 7 | Otherwise | Exit 0, `<X>` alone on stdout, the continuation `INFO` line on stderr |

Step 4 acts on the exit status:

| Exit | Step 4 then |
|---|---|
| 0 | Sets `EXISTING_BRANCH=<X>`, arms the [stop hint](#when-continuation-stops), and continues on the existing-branch path |
| 1 | Carries on as before: a new branch from the base |
| 3 | Exits 1 before creating anything; the helper has already said why and what to run |
| 2, or 4 or more | Prints the `caller-branch detection failed` line and exits 1 before creating anything |

Example, from the worktree in the [continuation example](#example):

```bash
C="${AMPLIHACK_HOME:-$HOME/.amplihack}/amplifier-bundle/tools/workflow_caller_branch.sh"

AMPLIHACK_SESSION_DEPTH=1 bash "$C" /repo/worktrees/claude/identity-delegation-receiver-model-2254
# claude/identity-delegation-receiver-model-2254   (exit 0; the INFO line is on stderr)

bash "$C" /repo
# (no output, exit 1: the main clone)
```

The helper only reads. It uses `git rev-parse`, `git symbolic-ref`,
`git check-ref-format`, `git worktree list` (for the alias stop's hint), and
the rule helper, and never runs `git status`, so a
`core.fsmonitor` hook in the caller's repository is not started. Like the
other helpers, it starts with `set -euo pipefail`, unsets `GIT_DIR`,
`GIT_WORK_TREE` and `GIT_INDEX_FILE`, is run with `bash` rather than sourced,
and works with bash 3.2. It needs git 2.31 or later for
`rev-parse --path-format=absolute`, which step 15 already requires.

The detection helper and `workflow_default_branch.sh` are listed in step 4's
`STEP_TOOLS`, so the #1808 rule that picks a complete tools directory takes
them into account.

---

## Other push sites

These still run a bare `git push` when `@{u}` is set, and are not changed by
#1969:

- `step-18c-push-feedback-changes` (`workflow-pr-review.yaml`) and
  `step-20b-push-cleanup` (`workflow-finalize.yaml`). Step 15's `-u` sets the
  correct upstream, `origin/<TARGET>`, earlier in the same run, including on a
  branch created before #1969 whose upstream was `origin/<base>`.
- `consensus-publish.yaml` and `consensus-pr-feedback.yaml`. These cannot
  recover from a failed setup push: consensus step 3 now creates branches with
  `--no-track`, so when its `git push -u` fails or is skipped, the branch has
  no upstream and the later bare `git pull --rebase && git push` fails too.
  Before #1969 the same retry failed on the name mismatch with `origin/<base>`.
  Moving these to an explicit refspec is tracked in
  [#2075](https://github.com/rysweet/amplihack-rs/issues/2075).

---

## Related

- [Caller branch worktree alias (#1762)](issue-1762-caller-branch-worktree-alias.md): the alias path that continuation reuses, the planner's checks and stops, and the resolver and planner references
- [Ancestry-aware step 15 publish (#978, #1969)](../features/workflow-publish-ancestry-aware-step15.md): the full step 15 contract
- [Re-runs never destroy an existing worktree's work (#1565)](issue-1565-rerun-preserves-worktree.md): continuing a kept worktree with `existing_branch`
- [Recipe commit hook repair](../reference/recipe-commit-hook-repair.md): how step 15's push is retried after a pre-push hook rejects it
- [Relative-path-safe, idempotent worktree setup (#1121)](issue-1121-relative-repo-path.md): `wt_add_idempotent` and its `create` mode with `--no-track`
