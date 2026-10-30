# Caller Branch Worktree Alias (Issue #1762)

When the branch a run targets is checked out in the caller's own clone,
`step-04-setup-worktree` no longer stops. It creates the task worktree on a
local alias branch, `<branch>--wt-<n>`, which starts at `origin/<branch>` and
has `origin/<branch>` as its upstream. Every later step pushes to `<branch>`
and looks the pull request up by `<branch>`, so the branch on `origin` is the
one updated. The caller's clone is not changed.

**Affects:**
- `amplifier-bundle/recipes/workflow-worktree.yaml`: `step-04-setup-worktree` (existing-branch path)
- `amplifier-bundle/recipes/workflow-publish.yaml`: `step-15-commit-push` and the `step-16b-outside-in-fix-loop` prompt
- `amplifier-bundle/recipes/workflow-pr-review.yaml`: `step-18c-push-feedback-changes`
- `amplifier-bundle/recipes/workflow-finalize.yaml`: `step-20b-push-cleanup`
- `amplifier-bundle/recipes/workflow-terminal-state.yaml`: the branch check
- `amplifier-bundle/tools/workflow_caller_branch_alias.sh` (new): the step-04 planner
- `amplifier-bundle/tools/workflow_remote_branch.sh` (new): the remote-branch resolver, and the ten tool call sites that use it

**Closes:** #1762

---

## Contents

- [Quick start](#quick-start)
- [The problem](#the-problem)
- [When the alias applies](#when-the-alias-applies)
- [Alias naming and reuse](#alias-naming-and-reuse)
- [Checks on the caller's checkout](#checks-on-the-callers-checkout)
- [Messages](#messages)
- [Step output](#step-output)
- [How later steps use the remote branch name](#how-later-steps-use-the-remote-branch-name)
- [Resolver reference: `workflow_remote_branch.sh`](#resolver-reference-workflow_remote_branchsh)
- [Planner reference: `workflow_caller_branch_alias.sh`](#planner-reference-workflow_caller_branch_aliassh)
- [Missing helpers](#missing-helpers)
- [Limitations](#limitations)
- [Cleaning up alias worktrees](#cleaning-up-alias-worktrees)
- [Testing](#testing)
- [Related](#related)

---

## Quick start

Nothing needs configuring. Suppose `/repo` is checked out on
`fix/142-band-edge-previous-slice`, the branch is pushed, and there are no
uncommitted changes to tracked files. Ask a run to continue that branch:

```bash
amplihack recipe run default-workflow \
  -c task_description="Address the review comments on PR #142" \
  -c repo_path="$(pwd)" \
  -c existing_branch="fix/142-band-edge-previous-slice"
```

Step 4's stderr includes these lines:

```text
INFO: Targeting existing branch 'fix/142-band-edge-previous-slice' (issue #342 path).
INFO: branch 'fix/142-band-edge-previous-slice' is checked out in the caller repository at /repo; the task worktree uses local branch 'fix/142-band-edge-previous-slice--wt-1' with upstream origin/fix/142-band-edge-previous-slice (issue #1762).
```

`recipe-runner-rs` does not keep the stderr of a bash step that succeeds, so
the run's log has the step output instead, which names both branches and gives
the reason:

```json
{"worktree_path": "/repo/worktrees/fix/142-band-edge-previous-slice--wt-1", "branch_name": "fix/142-band-edge-previous-slice", "base_ref": "", "base_branch": "", "created": false, "local_branch_reason": "branch_name is checked out in the caller repository (issue #1762)", "local_branch": "fix/142-band-edge-previous-slice--wt-1"}
```

Publish then runs `git push origin "HEAD:fix/142-band-edge-previous-slice"`
from the alias worktree. `/repo` keeps its HEAD, index, working tree and
branch configuration.

To confirm the mapping by hand:

```bash
git -C /repo/worktrees/fix/142-band-edge-previous-slice--wt-1 rev-parse --abbrev-ref '@{u}'
# origin/fix/142-band-edge-previous-slice

bash "${AMPLIHACK_HOME:-$HOME/.amplihack}/amplifier-bundle/tools/workflow_remote_branch.sh" \
  /repo/worktrees/fix/142-band-edge-previous-slice--wt-1
# fix/142-band-edge-previous-slice
```

---

## The problem

Before this change, step 4 stopped the run whenever the target branch was
checked out in the caller's clone:

```text
ERROR: existing branch 'fix/142-band-edge-previous-slice' is checked out in the caller repository — refusing to use the caller checkout as a recipe task worktree (issue #858).
ERROR: run the recipe from a clean base checkout or detach/switch the caller checkout before targeting this branch.
```

The #858 rule stays: the caller's checkout is never used as the task
worktree, because it may hold unrelated commits and uncommitted files that a
failed run would then have to salvage. The stop, however, left a person or a
worker to work around it by hand, and an agent is often sitting on the PR
branch that it then asks a recipe to continue.

Git does not allow one branch to be checked out in two worktrees, so the task
worktree needs a second local branch. The alias is that branch. It points at
the same remote branch, and every step that talks to `origin` or to the
provider uses the remote name.

---

## When the alias applies

All of the following must hold:

1. The run takes the existing-branch path (#342). That happens when
   `-c existing_branch=<B>` is set, when `-c pr_number=<N>` names a PR whose
   head is `<B>`, or when the task description names a branch that already
   exists (`Branch: <B>`, issue #1426).
2. `<B>` is checked out in the caller's checkout. The caller's checkout is the
   `repo_path` directory after symbolic links are resolved; it may be a linked
   worktree. Step 4 detects this either because `git worktree list` reports
   `<B>` at that directory (Gate B) or because the caller's `HEAD` is `<B>`
   (Gate C).
3. `<B>` is not the default branch.
4. The caller's checkout passes the [three checks](#checks-on-the-callers-checkout).

The default branch is `main`, `master`, the branch that
`refs/remotes/origin/HEAD` points at, and `develop` when
`refs/remotes/origin/HEAD` does not resolve.

These cases behave as before:

| Situation | Result |
|---|---|
| The caller is on another branch | The task worktree is `<main>/worktrees/<B>`, on `<B>` itself. |
| `<B>` is checked out in a worktree other than the caller's | That worktree is reused (`INFO: Reusing existing worktree at '<path>'.`). See [Limitations](#limitations) for runs started from an alias worktree. |
| Git reports a worktree for `<B>` at a path that is not accessible (Gate A) | The existing `git worktree prune` error, exit 1. |
| `<B>` is the default branch | The #858 refusal, exit 1. |
| The derived-slug path (no existing branch named) | Unchanged. See [#1565](issue-1565-rerun-preserves-worktree.md). |
| `consensus-issue-worktree.yaml` `step3-setup-worktree` | Keeps the #858 refusal. Only `default-workflow` step 4 creates aliases. |

---

## Alias naming and reuse

The alias is `<B>--wt-<n>`. `n` is a decimal number starting at 1, with no
leading zeros. The alias worktree is `<main>/worktrees/<B>--wt-<n>`, where
`<main>` is the main repository (issue #1134), also when the caller is a
linked worktree. A slash in `<B>` gives subdirectories, as for any task
worktree: `feat/issue-1762-fixture` becomes
`<main>/worktrees/feat/issue-1762-fixture--wt-1`.

Step 4 picks the alias in two passes.

**Reuse pass.** Among local branches `refs/heads/<B>--wt-<n>`, take the lowest
`n` for which all of these hold:

- `branch.<alias>.remote` is `origin`;
- `branch.<alias>.merge` is `refs/heads/<B>`;
- `git worktree list --porcelain` shows the alias checked out at
  `<main>/worktrees/<alias>`, compared as physical paths.

The worktree is reused as it stands, including any uncommitted changes or
commits an earlier run left in it. Two runs started one after the other from
the same caller therefore share one alias worktree. The reuse pass does not
check whether another run is still using it.

**Create pass.** If nothing qualifies for reuse, take the lowest `n` for which:

- `refs/heads/<alias>` does not exist;
- nothing exists at `<main>/worktrees/<alias>`: no file, directory or symbolic
  link;
- `git check-ref-format --branch <alias>` prints the alias unchanged.

Step 4 then runs the #1134 nesting check and creates the branch and worktree in
one command:

```bash
git worktree add --track -b "<B>--wt-<n>" -- "<main>/worktrees/<B>--wt-<n>" "origin/<B>"
```

The alias starts at `origin/<B>` and tracks it.

An alias-shaped branch that fails the reuse conditions is skipped and left
untouched: one with another upstream, one with no worktree, and one checked out
somewhere else. Because its ref exists, the create pass does not take its `n`
either. A branch on `origin` that happens to be called `<B>--wt-<n>` plays no
part; only local branches are considered.

---

## Checks on the caller's checkout

Before choosing an alias, step 4 checks the caller's checkout. The checks run
in this order, and only the first failure is reported:

| # | Check | Command | Stops when |
|---|---|---|---|
| 1 | Uncommitted changes to tracked files | `git -C <caller> --no-optional-locks status --porcelain --untracked-files=no` | The output is not empty, or the command fails |
| 2 | `<B>` exists on `origin` | `refs/remotes/origin/<B>` resolves to a commit | It does not resolve |
| 3 | No local commits missing from `origin/<B>` | `git rev-list --count refs/remotes/origin/<B>..refs/heads/<B>` | The count is above 0, the command fails, or the output is not a number |

Each stop exists because the task worktree starts from `origin/<B>` and would
not see the change in question.

- Untracked and ignored files are not checked. They stay in the caller's
  checkout, and the task worktree does not see them.
- `--no-optional-locks` keeps the check from refreshing the caller's index.
- A caller that is strictly behind `origin/<B>` passes. The alias starts at
  `origin/<B>`, and the caller's branch is not moved.
- A caller that has diverged from `origin/<B>` fails check 3. A plain
  `git push origin <B>` is then rejected as a non-fast-forward, so the caller
  has to integrate `origin/<B>` first, for example with `git pull --rebase`,
  and then push.

Before these checks, step 4 makes its usual best-effort fetch of
`origin/<B>`. The checks themselves never fetch. If that fetch fails, they use
the last fetched `origin/<B>`. Should `origin` have moved on since then, the
alias starts behind it, and the step-15 push is rejected as a non-fast-forward.
Nothing on `origin` is overwritten.

---

## Messages

All messages go to stderr. The `INFO` line below and the planner's three stops
name the branch and the caller's path and end with `(issue #1762).` Every other
message that #1762 adds mentions `issue #1762` but need not name the caller's
path. No message contains a remote URL, the names of changed files, a diff or
a commit message. When the caller is a linked worktree, `<path>` is that linked
worktree, not the main repository.

### Alias in use

```text
INFO: branch '<B>' is checked out in the caller repository at <path>; the task worktree uses local branch '<alias>' with upstream origin/<B> (issue #1762).
```

The same line is printed when an existing alias worktree is reused.

### Stops

Each stop prints one of these lines, then
`ERROR: cannot set up an alias worktree for '<B>' (planner exit 1; issue #1762).`,
and exits 1 with nothing on stdout. Nothing is created.

| Cause | Message | Remedy |
|---|---|---|
| Uncommitted changes to tracked files | `ERROR: branch '<B>' is checked out in the caller repository at <path> and has uncommitted changes to tracked files. The task worktree starts from origin/<B> and would not see those changes. Commit and push them, or discard them, then re-run (issue #1762).` | Commit and push, or discard, in the caller's checkout. |
| `<B>` not on `origin` | `ERROR: branch '<B>' is checked out in the caller repository at <path> but origin/<B> does not exist. The task worktree tracks origin/<B> and would not see the branch's commits. Push the branch with 'git push -u origin <B>', or work from a checkout that is not on '<B>', then re-run (issue #1762).` | `git push -u origin <B>`, or run from a checkout on another branch. |
| Commits not on `origin/<B>` | `ERROR: branch '<B>' is checked out in the caller repository at <path> and has <N> commit(s) that are not on origin/<B>. The task worktree starts from origin/<B> and would not see those commits. Push them with 'git push origin <B>' (run 'git pull --rebase' first if origin/<B> has moved on), or discard them, then re-run (issue #1762).` | `git push origin <B>`. If the branch has diverged from `origin/<B>`, integrate it first with `git pull --rebase`. Or discard the commits. |

The planner also stops, with exit 1 and its own `ERROR` line, when
`git status` itself fails, when `git rev-list` fails or prints something other
than a number, and when a candidate alias does not survive
`git check-ref-format --branch` unchanged. In each case it cannot prove the
caller's checkout is safe, so it does not go on.

Step 4 also stops when the alias branch exists but is not checked out at
`<main>/worktrees/<B>--wt-<n>`:

```text
ERROR: alias '<alias>' exists but is not checked out at <main>/worktrees/<alias>; another run may be creating it. Wait for that run to finish, then re-run (issue #1762).
```

The planner never chooses an alias in that state, so this happens only while
another run is creating that alias. Step 4 exits 1 with nothing on stdout and
removes nothing: the other run's branch, directory and worktree stay as they
are. Wait for the other run to finish, then re-run. This stop comes from step
4, not the planner, so the `cannot set up an alias worktree` line does not
follow it.

### Default branch

When `<B>` is the default branch, the planner prints an `INFO` line saying that
`<B>` is the default branch and gets no alias, and step 4 prints the two #858
`ERROR` lines shown under [The problem](#the-problem) and exits 1.

---

## Step output

On the alias path, step 4 prints one JSON line on stdout:

| Key | Value |
|---|---|
| `worktree_path` | The physical path of the alias worktree |
| `branch_name` | `<B>`, the remote branch |
| `base_ref`, `base_branch` | `""`, as on every existing-branch run |
| `created` | `false`. It describes `branch_name`: `<B>` existed before the run, even though the alias branch and its worktree may be new. |
| `local_branch_reason` | `branch_name is checked out in the caller repository (issue #1762)`. Present only on the alias path. It repeats the reason from the `INFO` line, because the recipe runner does not keep a successful step's stderr. |
| `local_branch` | The alias. The last key, and present only on the alias path. |

Later steps read `{{worktree_setup.branch_name}}` and
`{{worktree_setup.worktree_path}}` as before, so they get `<B>` and the alias
worktree.

On the alias path step 4 also skips its best-effort
`git push origin <branch>` and `git branch --set-upstream-to`, so it never
pushes the alias as a branch, and the alias's upstream stays `origin/<B>`.

---

## How later steps use the remote branch name

Every step or tool that pushes, or looks up a pull request by branch, asks the
resolver for the remote branch name instead of reading
`git branch --show-current`. In an alias worktree the resolver prints `<B>`.
Anywhere else it prints the local branch name, so those runs behave exactly as
before.

The alias is in effect when the resolver's output is not empty and differs from
`git branch --show-current`.

| Step or tool | Purpose | On the alias path |
|---|---|---|
| `workflow-publish.yaml` `step-15-commit-push` | Publish | Pushes with `git push origin "HEAD:<B>"`, passes `<B>` to `verify_pushed_ref.sh`, and refuses to push unless the resolved name equals `worktree_setup.branch_name` |
| `workflow-publish.yaml` `step-16b-outside-in-fix-loop` prompt | Push fixes found by outside-in testing | The prompt's code block resolves `<B>` when it runs and pushes with `git push origin "HEAD:<B>"` |
| `workflow-pr-review.yaml` `step-18c-push-feedback-changes` | Push review fixes | `git push origin "HEAD:<B>"`; the `-u` fallback is `git push -u origin "<alias>:<B>"` |
| `workflow-finalize.yaml` `step-20b-push-cleanup` | Push the cleanup commit | Same as step 18c |
| `workflow-terminal-state.yaml` | Check the checkout is on `branch_name` | Compares `<B>` with `branch_name` |
| `workflow_pr_head_sync.sh` | Sync the worktree to the PR head | Fetches `origin/<B>`; sets the alias's upstream only when it has none |
| `workflow_publish_pr.sh` | Create the pull request | `gh pr create --draft --head=<B>` |
| `workflow_final_status.sh` (two sites), `workflow_pr_ready.sh`, `workflow_terminal_evidence.sh` | Look up the pull request by branch | Look up `<B>` |
| `workflow_finalization_open_pr.sh`, `workflow_finalization_pr_signal.sh`, `workflow_finalization_collect.sh` | Finalization's open-PR check | Look up `<B>` |

### Push safety

- The push steps (`step-15-commit-push`, `step-16b-outside-in-fix-loop`,
  `step-18c-push-feedback-changes` and `step-20b-push-cleanup`) write at most
  `refs/heads/<B>` on `origin`, as a fast-forward, and never push the alias as
  a branch. No push uses `--force`, `--force-with-lease` or a `+` refspec.
- The orphan sweep is the only part of a run that can push a ref named after
  the alias: `refs/amplihack-archive/<alias>`, for a stale alias worktree with
  unmerged commits. See [Cleaning up alias worktrees](#cleaning-up-alias-worktrees).
- The `-u` fallbacks in steps 18c and 20b name `<B>` as the destination, so a
  missing `origin/<B>` tracking ref cannot publish the alias name or point the
  alias at a new upstream.
- Step 15 refuses to push from an alias worktree unless the resolved remote
  branch equals `worktree_setup.branch_name`. It prints
  `ERROR: push target '<resolved>' does not match worktree_setup.branch_name '<B>'; refusing to push (issue #1762).`
  and exits 1.
- No branch name reaches shell text through `{{...}}` templating. Step 16b
  gets `<B>` from the resolver when its code block runs.

`commit_result.branch` in the step-15 output still reports the local branch,
which is the alias. Nothing reads it to push or to look up a pull request.

---

## Resolver reference: `workflow_remote_branch.sh`

```text
usage: workflow_remote_branch.sh [DIR]
```

`DIR` defaults to `.`. The resolver runs only `git symbolic-ref`,
`git rev-parse --verify` (to test whether `refs/remotes/origin/HEAD`
resolves) and `git config --get`, and changes nothing.

| Checkout at `DIR` | Output |
|---|---|
| Detached HEAD, not a repository, or `DIR` missing | Nothing |
| Branch name without `--wt-` | The name, unchanged |
| `<P>--wt-<n>` where `n` matches `^[1-9][0-9]*$`, `<P>` is not empty and not the default branch, `branch.<name>.remote` is `origin`, and `branch.<name>.merge` is `refs/heads/<P>` | `<P>` |
| Any other `--wt-` name | The name, unchanged |

Only the last `--wt-` suffix is parsed: `a--wt-1--wt-2` can map only to
`a--wt-1`. The default branch is defined as in
[When the alias applies](#when-the-alias-applies).

The resolver reads the branch configuration rather than `@{u}`, so it still
answers when the remote-tracking ref is missing. It unsets `GIT_DIR`,
`GIT_WORK_TREE` and `GIT_INDEX_FILE` first, so an inherited environment cannot
point it at another repository.

| Exit | Meaning |
|---|---|
| 0 | Any of the outputs above |
| 2 | More than one argument. Prints the usage line on stderr. |

Examples, from a checkout of the alias created in the
[quick start](#quick-start):

```bash
R="${AMPLIHACK_HOME:-$HOME/.amplihack}/amplifier-bundle/tools/workflow_remote_branch.sh"

bash "$R" /repo/worktrees/fix/142-band-edge-previous-slice--wt-1
# fix/142-band-edge-previous-slice

bash "$R" /repo
# fix/142-band-edge-previous-slice   (the caller's own branch, unchanged)

bash "$R" /
# (no output, exit 0: not a repository)
```

Callers treat a resolver that is missing or fails as an error. They do not
fall back to the local branch name, which from an alias worktree is
`<B>--wt-<n>`. Bash's own message names the missing file:

```bash
rb="$(bash "$R" "$dir")" || exit 2
```

---

## Planner reference: `workflow_caller_branch_alias.sh`

```text
usage: workflow_caller_branch_alias.sh <caller_path> <main_repo_path> <branch>
```

Step 4 calls it as:

```bash
bash "$TOOLS_DIR/workflow_caller_branch_alias.sh" "$REPO_REAL" "$MAIN_REPO_PATH" "$BRANCH_NAME"
```

It runs the [checks](#checks-on-the-callers-checkout) against `caller_path`
and then [chooses the alias](#alias-naming-and-reuse).

| Exit | stdout | stderr | Meaning |
|---|---|---|---|
| 0 | The alias, one line | The `INFO` line | Use this alias |
| 1 | Nothing | One `ERROR` line | A check failed; stop |
| 2 | Nothing | The usage line | Wrong argument count, a missing directory, or a `<branch>` that `git check-ref-format --branch` does not print back unchanged (this rejects `@{-1}`) |
| 3 | Nothing | An `INFO` line | `<branch>` is the default branch; no alias |

Step 4 accepts the result only when the exit is 0 and the output starts with
`<branch>--wt-`. Anything else ends the step with
`ERROR: cannot set up an alias worktree for '<branch>' (planner exit <rc>; issue #1762).`
and exit 1, except exit 3, which gives the #858 refusal.

The planner only reads. It never fetches, pushes, or creates branches,
worktrees, configuration or refs, and it does not contain `update-ref`,
`reset`, `checkout`, `stash`, `clean`, `branch -D`, `branch -m`,
`worktree add`, `worktree remove` or any `git config` call without `--get`.
Like the resolver, it unsets `GIT_DIR`, `GIT_WORK_TREE` and `GIT_INDEX_FILE`.

On the alias path, step 4 itself writes these and nothing else:

- `refs/remotes/origin/<B>`, through the best-effort fetch it already made
  before #1762;
- `refs/heads/<alias>`, `branch.<alias>.remote` and `branch.<alias>.merge`;
- the worktree at `<main>/worktrees/<alias>`.

The orphan sweep that step 4 runs first on every run may also remove or archive
stale task worktrees, as described under
[Cleaning up alias worktrees](#cleaning-up-alias-worktrees). The alias path
never touches the caller's HEAD, index, working tree or `branch.<B>.*`, and
never stashes, moves or discards the caller's work.

Both helpers are bash scripts compatible with bash 3.2, the system bash on
macOS.

---

## Missing helpers

| Missing | Effect |
|---|---|
| Planner | Step 4 prints `WARNING: <TOOLS_DIR>/workflow_caller_branch_alias.sh not found; cannot create an alias worktree for '<B>' (issue #1762).`, then the two #858 `ERROR` lines, and exits 1 with nothing on stdout. This is the behaviour from before #1762. |
| Resolver | Bash prints `No such file or directory` with the resolver's path, and no caller uses the local branch name instead. Steps 15, 18c and 20b, the step 16b retry command and `workflow_publish_pr.sh` stop before pushing or opening a PR. `workflow_pr_ready.sh`, `workflow-terminal-state`, finalization's evidence collection and the PR scope check in `workflow_final_status.sh` stop with an `ERROR`. The other lookups fail closed: the open-PR check confirms no PR, the branch PR lookup in `workflow_final_status.sh` is skipped, the terminal evidence verdict is `UNCERTAIN`, and `workflow_pr_head_sync.sh` syncs nothing. |

Both helpers ship in `amplifier-bundle/tools/`, so an install that lacks either
one is damaged. Without the stop, a run in an alias worktree would push, or
look up the PR, under the alias name. Reinstall the bundle with
`amplihack install`.

---

## Limitations

- **Explicit `branch_name` equal to the alias.** `workflow-terminal-state`
  compares the resolved name, `<B>`, with `branch_name`. Passing
  `branch_name=<B>--wt-<n>` from an alias worktree gives
  `FAILED_WRONG_BRANCH`. Pass `<B>`, or leave `branch_name` empty.
- **Hand-made branches of the alias shape.** A local branch named
  `<B>--wt-<n>` with `branch.<name>.remote=origin` and
  `branch.<name>.merge=refs/heads/<B>` is treated as an alias by the resolver
  wherever it is checked out. Pushes from it go to `<B>`.
- **Hidden changes.** Files marked `assume-unchanged` or `skip-worktree` are
  not seen by the uncommitted-changes check.
- **Default branch detection is local.** `refs/remotes/origin/HEAD` can be
  missing or stale, so a renamed default branch may not be recognised.
  Refresh it with `git remote set-head origin --auto`, and protect the default
  branch on the server. The local check does not replace branch protection.
- **Concurrent runs.** The reuse pass does not check whether another run is
  using an alias worktree, so two runs for `<B>` from the same caller can work
  in the same worktree at once. Two runs that reach the create pass at the same
  moment choose the same `n`. What the second run's step 4 does depends on how
  far the first run has got:
  - the first has not yet created the alias ref: both try
    `git worktree add -b`, the second fails on git's ref lock, and its step 4
    exits 1;
  - the first has created the alias ref but its worktree is not yet on the
    alias: the second run's step 4 stops with the message under
    [Stops](#stops);
  - the first run's worktree is already on the alias: the second run reuses it.

  None of these removes anything. A re-run does not take the next `n`: the
  first run's alias now meets the reuse conditions, so the re-run reuses it and
  shares the worktree with the first run. Wait for the first run to finish
  before re-running.
- **Auto-drive rounds.** Auto-drive does not yet support alias worktrees.
  Before a crusty or merge-ready round stages anything, it runs
  `autodrive_worktree_guard.sh commit-dir`, which requires
  `git branch --show-current` to equal the PR's `headRefName` and otherwise
  stops with "this worktree is not on the PR head branch". An alias worktree
  is on `<B>--wt-<n>`, so a round run there stops and commits and pushes
  nothing. To use auto-drive on `<B>`, start it from a checkout that is not on
  `<B>`.
- **Nested runs from an alias worktree.** A run started from inside an alias
  worktree of `<B>` and targeting `<B>` finds `<B>` checked out in the original
  caller's checkout. That checkout is not the new run's caller, so the existing
  rule for a worktree other than the caller's applies, and the nested run uses
  the original caller's checkout as its task worktree. This goes against the
  intent of #858. The same rule applies to a run that targets `<B>` from any
  third checkout while the original caller is on `<B>`; that behaviour predates
  #1762.
- **The caller later leaves `<B>`.** The next run takes the ordinary path, on
  `<main>/worktrees/<B>`. The old alias worktree stays until the sweep removes
  it.

---

## Cleaning up alias worktrees

Alias worktrees live under `<main>/worktrees/`, so the orphan sweep
(`workflow_worktree_sweep.sh`, see [#840](issue-840-leak-proof-worktree.md))
handles them like any task worktree:

| Alias worktree | Sweep result |
|---|---|
| Newer than `AMPLIHACK_WORKTREE_STALE_SECS` (default 86400) | Left alone |
| Stale, clean, and its changes already in the base | Removed together with the alias branch |
| Stale and holding commits not in the base | Archived to `refs/amplihack-archive/<alias>` and kept. The alias is never on `origin`, so the best-effort archive push writes `refs/amplihack-archive/<alias>` on `origin`, which branch listings do not show. |
| Uncommitted or untracked files | Left alone |

To remove one by hand:

```bash
git worktree remove worktrees/fix/142-band-edge-previous-slice--wt-1
git branch -d fix/142-band-edge-previous-slice--wt-1   # refuses if it has commits not on origin/<B>
```

---

## Testing

`tests/integration/issue_1762_caller_branch_alias_test.rs` extracts the recipe
step bodies from the YAML and runs them against a fixture repository whose `origin`
is a local bare repository. Each child runs through `isolated("bash")`, with a
fixture-owned `HOME`, `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL=/dev/null`,
`GIT_TERMINAL_PROMPT=0`, `GIT_ALLOW_PROTOCOL=file` and stdin from `/dev/null`.
`AMPLIHACK_HOME` points at the workspace, so the helpers are read from the
source tree. A fake `gh` logs its arguments and rejects unknown subcommands.
The fixture branch is `feat/issue-1762-fixture`, so its alias directory is
`worktrees/feat/issue-1762-fixture--wt-1`.

| Group | Tests |
|---|---|
| Step 4 | `alias_created_when_caller_on_pushed_clean_branch` (with a snapshot showing the caller is untouched), `rerun_reuses_alias_worktree`, `alias_shaped_branch_with_other_upstream_is_skipped`, `caller_behind_origin_alias_starts_at_origin`, `caller_not_on_branch_is_unchanged`, `default_branch_keeps_858_refusal`, `missing_planner_warns_then_refuses`, `concurrent_create_does_not_remove_existing_worktree`, `alias_ref_made_by_other_run_is_not_removed` |
| Stops | `stops_on_uncommitted_tracked_change`, `stops_when_branch_not_on_origin`, `stops_on_unpushed_commit`, `reports_only_first_cause` |
| Consumers | `publish_pushes_alias_head_to_original_remote_branch`, `open_pr_check_looks_up_remote_branch`, `final_status_branch_lookup_uses_remote_branch`, `terminal_state_probe_accepts_alias` (with a `FAILED_WRONG_BRANCH` control on an alias-shaped branch with another upstream), `remote_branch_resolver_contract`, `planner_and_resolver_agree_on_default_branches` |
| Security | `step16b_prompt_does_not_template_branch_name`, `planner_writes_nothing`, `planner_rejects_previous_branch_syntax`, `planner_status_unreadable_fails_closed`, `planner_ignores_inherited_git_dir`, `push_fallback_keeps_alias_mapping`, `step15_refuses_unbound_push_target`, `helpers_contain_no_write_commands`, `missing_resolver_stops_push_steps`, `resolver_callers_do_not_fall_back_to_local_branch` |

`tests/integration/workflow_worktree_caller_refusal_test.rs` keeps the #858
cases. Its Gate B, Gate B symlink and Gate C cases now expect the #1762 stop:
the fixture branch is not on `origin`, so the run exits 1 with an empty
stdout, no `worktrees/` directory, the caller unchanged, and stderr naming
`#1762` and `origin/`. In `tests/issue_1426_branch_name_not_prose.sh`, case E2
expects the same stop for `origin/fix/142-band-edge-previous-slice`, and E2b
still checks that no competing branch is derived from the task's prose.

Run them locally:

```bash
cargo test --test integration issue_1762
cargo test --test integration workflow_worktree_caller_refusal
bash tests/issue_1426_branch_name_not_prose.sh
shellcheck -S warning amplifier-bundle/tools/workflow_remote_branch.sh \
  amplifier-bundle/tools/workflow_caller_branch_alias.sh
```

---

## Related

- [Branch name generation (#1426)](../features/branch-name-generation.md#item-6-caller-checkout): how the #1426 request to use the `-w` checkout relates to #858 and the alias
- [Re-runs never destroy an existing worktree's work (#1565)](issue-1565-rerun-preserves-worktree.md): the derived-slug path, and `consensus-issue-worktree.yaml` step 3
- [Relative-path-safe, idempotent worktree setup (#1121)](issue-1121-relative-repo-path.md): `wt_add_idempotent` and the three-state guard
- [Leak-proof worktree setup (#840)](issue-840-leak-proof-worktree.md): the orphan sweep and the archive namespace
