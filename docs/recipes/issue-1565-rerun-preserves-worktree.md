# Re-runs Never Destroy an Existing Worktree's Work (Issue #1565)

When a task is re-run, `step-04-setup-worktree` no longer resets, cleans or
deletes a branch or worktree that already holds work. If the derived branch's
worktree has uncommitted changes, untracked files or commits ahead of
`BASE_REF`, the step **keeps** that branch and worktree exactly as they are and
gives the new run its own fresh branch and worktree.

**Affects:**
- `amplifier-bundle/recipes/workflow-worktree.yaml`: `step-04-setup-worktree`
- `amplifier-bundle/tools/workflow_worktree_deconflict.sh`: `resolve --base`

**Closes:** #1565

---

## Contents

- [Quick start](#quick-start)
- [The problem](#the-problem)
- [Behaviour by state](#behaviour-by-state)
- [What counts as work](#what-counts-as-work)
- [Messages](#messages)
- [Continuing an earlier branch on purpose](#continuing-an-earlier-branch-on-purpose)
- [Fail-loud guard](#fail-loud-guard)
- [Cleaning up kept worktrees](#cleaning-up-kept-worktrees)
- [Testing](#testing)
- [Related](#related)

---

## Quick start

Nothing needs configuring. Run the same task twice:

```bash
amplihack recipe run default-workflow \
  -c task_description="Fix issue #1565" \
  -c repo_path="$(pwd)"
```

If the first run left work in
`worktrees/feat/issue-1565-fix-issue-1565`, the second run prints:

```text
WARNING: keeping existing_branch=feat/issue-1565-fix-issue-1565 at /repo/worktrees/feat/issue-1565-fix-issue-1565 (uncommitted/untracked changes: yes, commits ahead of origin/main: 1); this run uses feat/issue-1565-fix-issue-1565-1760781234-1-48213-4821. To continue the kept branch, re-run with existing_branch=feat/issue-1565-fix-issue-1565.
```

and its step output names the new branch:

```json
{
  "worktree_path": "/repo/worktrees/feat/issue-1565-fix-issue-1565-1760781234-1-48213-4821",
  "branch_name": "feat/issue-1565-fix-issue-1565-1760781234-1-48213-4821",
  "base_ref": "origin/main",
  "base_branch": "main",
  "runtime_root": "/run/user/1000/amplihack-runtime/feat/issue-1565-fix-issue-1565-1760781234-1-48213-4821",
  "created": true
}
```

The first run's files, modifications and commits are untouched.

---

## The problem

Before this fix, the State 1 reuse path ran the following on any existing
worktree it found dirty or ahead of base:

```bash
git -C "${WORKTREE_PATH}" checkout -- .
git -C "${WORKTREE_PATH}" clean -fd
git -C "${WORKTREE_PATH}" reset --hard "${BASE_REF}"
```

The State 2 path (branch exists, worktree missing) ran `git branch -D` on a
branch with commits ahead of base. Together they deleted uncommitted edits and
untracked files and orphaned unpushed commits. The step cannot tell whether
that work came from an earlier run of the same task or from a person working
in the worktree, so it must not destroy it.

Issue #1565 offered two fixes: keep the work and move the new run to a new
branch (option 1), or archive the work under
`refs/amplihack-archive/<branch>/<timestamp>` before resetting (option 2).
Option 1 is implemented. It reuses the existing deconfliction helper, writes no
refs, and leaves the earlier work exactly where the user left it. Option 2
would still rewrite the user's worktree and need a recovery procedure.

---

## Behaviour by state

`BASE_REF` is the resolved base for this run (for example `origin/main`).

| State | Situation | Result | `created` |
|---|---|---|---|
| 1, clean | Branch and worktree exist at this run's path, no changes, 0 commits ahead | Reused unchanged, same branch and path. Not fast-forwarded even if behind `BASE_REF`. | `false` |
| 1, dirty or ahead | Branch and worktree exist at this run's path, with changes and/or commits ahead | Kept untouched. This run gets a fresh branch from `BASE_REF` in its own worktree. | `true` |
| 2, not ahead | Branch exists, no worktree, 0 commits ahead | Worktree attached to the existing branch. | `true` |
| 2, ahead | Branch exists, no worktree, commits ahead | Branch kept at the same SHA. This run gets a fresh branch. | `true` |
| 3 | Branch does not exist | Branch and worktree created from `BASE_REF`. | `true` |
| Foreign | Branch is checked out by a worktree at a different path | Unchanged from before: this run gets a fresh branch. | `true` |

Step-04 contains no `checkout -- .`, `clean -f`, `reset --hard` or
`branch -D`. The fresh name comes from
[`workflow_worktree_deconflict.sh resolve --base`](../../amplifier-bundle/tools/workflow_worktree_deconflict.md#the---base-flag):
`<branch>-<suffix>`, at most 80 characters, checked with
`git check-ref-format --branch`, and not already used by a local branch, a
local remote-tracking ref (`refs/remotes/origin/<name>`) or any worktree. The
helper does not fetch, so a branch pushed to `origin` since the last fetch is
not seen.

The kept branch is never pushed by the new run. The push at the end of step-04
runs after `BRANCH_NAME` has been reassigned, so it pushes only the new branch.

---

## What counts as work

| Signal | Check | Notes |
|---|---|---|
| Uncommitted or untracked changes | Any output from `git --no-optional-locks -C <worktree> status --porcelain` | Covers modified, staged and untracked files. Ignored files don't count. The old `clean -fd` never touched them either. `--no-optional-locks` avoids taking a live worktree's index lock (#1560). |
| Commits ahead | `git rev-list --count "${BASE_REF}..refs/heads/<branch>"` > 0 | Counted whether or not the commits are pushed. |
| Cannot tell | `status` or `rev-list` fails, the count is not a number, the worktree directory is missing, or `BASE_REF` does not resolve | Treated as work. The run moves to a fresh branch. |

When the check can't give an answer, the step assumes work is there: the cost
is one extra branch, never lost work.

---

## Messages

All messages go to stderr. They contain the branch, the path, `yes`/`no` for
changes, the ahead count and `BASE_REF`. They never contain file names, diffs,
commit messages or the task description.

| Level | When | Meaning |
|---|---|---|
| `WARNING: keeping existing_branch=<branch> …` | Printed by the helper when it moves this run off a branch holding work | Earlier work is kept at the path shown. This run uses the new branch named in the message. |
| `INFO: branch '<branch>' is owned by a foreign worktree …` | Printed by the helper when a different worktree holds the branch | Unchanged foreign-worktree deconfliction. |
| `INFO: using branch '<new>' instead of '<old>'.` | Printed by step-04 after either kind of rename | Step-04 can't tell the two renames apart; the helper line just above it says which it was. |
| `INFO: Branch '<branch>' and worktree '<path>' already exist and are clean — reusing.` | Clean State 1 | Same branch, `created=false`. |
| `ERROR: … refusing to reset existing_branch=<branch> …` | The [fail-loud guard](#fail-loud-guard) fired | Step exits 1. Nothing was changed. |
| `ERROR: worktree branch deconfliction failed for '<branch>' …` | The helper exited non-zero: retries exhausted, misconfigured, or an old copy that rejects `--base` | Step exits 1. Nothing was changed. |
| `ERROR: deconflict helper returned an invalid branch name` | The helper's output failed `git check-ref-format --branch` | Step exits 1 before any ref is created. |

---

## Continuing an earlier branch on purpose

A plain re-run never adopts a branch that holds work. To carry on with it, name
it explicitly through the issue #342 `existing_branch` path:

```bash
amplihack recipe run default-workflow \
  -c task_description="Fix issue #1565" \
  -c repo_path="$(pwd)" \
  -c existing_branch="feat/issue-1565-fix-issue-1565"
```

The `WARNING` and `ERROR: … refusing to reset …` messages both print the exact
`existing_branch=<branch>` value to use.

A task description that names the branch on its own line (`Branch: <ref>`,
issue #1426) does the same thing: when that branch already exists, step-04
sets `existing_branch` to it and takes the #342 path. Both routes run before
the deconflict helper and the state machine, so a branch you name is adopted
as it stands, work included.

---

## Fail-loud guard

A missing helper fails the step only when work is present. A clean State 1, a
State 2 with no commits ahead, and State 3 proceed as before without it.

The step re-checks for work just before its state machine runs, in case the
helper is missing or the worktree changed between the helper call and the
state machine. If State 1 is
dirty or ahead, or State 2 is ahead, at that point, step-04 prints:

```text
ERROR: refusing to reset existing_branch=feat/issue-1565-fix-issue-1565 at /repo/worktrees/feat/issue-1565-fix-issue-1565 (uncommitted/untracked changes: yes, commits ahead of origin/main: 1). Re-run with existing_branch=feat/issue-1565-fix-issue-1565 to continue it, or move the work aside.
```

and exits `1`. It never falls back to resetting.

An old helper that does not understand `--base` exits `1` on it as a bad
argument, so step-04 prints
`ERROR: worktree branch deconfliction failed for '<branch>' …` and exits `1`.
As a second line of defence, the helper's output is checked with
`git check-ref-format --branch`: a helper that echoes `--base` back as the
branch name makes step-04 print
`ERROR: deconflict helper returned an invalid branch name` and exit `1` before
any ref is created.

---

## Cleaning up kept worktrees

Kept branches and worktrees are not garbage-collected. List and remove the ones
you no longer need yourself:

```bash
git worktree list
git worktree remove worktrees/feat/issue-1565-fix-issue-1565
git branch -d feat/issue-1565-fix-issue-1565   # -d refuses if unmerged
```

The orphan sweep (`workflow_worktree_sweep.sh`, see
[#840](issue-840-leak-proof-worktree.md)) is unchanged and still skips dirty
worktrees.

---

## Testing

`amplifier-bundle/recipes/tests/test-issue-1565-rerun-preserves-dirty-worktree.sh`
extracts the real step-04 body and runs it against a scratch repo with a local
bare `origin`, under `env -i` with a scratch `HOME` and
`GIT_CONFIG_GLOBAL=/dev/null`. Scratch lives under `$TMPDIR`.

| Scenario | Checks |
|---|---|
| S1: dirty State 1 (untracked file, modified file, unpushed commit) | All three survive in the original worktree and branch. The run gets a new branch whose HEAD is `BASE_REF`, its worktree exists under `worktrees/`, `created=true`, and stderr names the kept branch. |
| S2: clean State 1 | Same branch and path, `created=false`. |
| S3: State 2, 1 commit ahead | Old branch still at the same SHA. The run gets a new branch. |
| S4: `resolve` without `--base` | Output unchanged from before. |
| S5: helper missing | Exits non-zero, stderr says `refusing to reset`, all work survives. |
| S6: old helper that prints `--base` | Exits non-zero, no ref created. |
| Static | Step-04 contains no `checkout -- .`, `clean -f`, `reset --hard` or `branch -D`. |

Run it locally:

```bash
bash amplifier-bundle/recipes/tests/test-issue-1565-rerun-preserves-dirty-worktree.sh
```

To confirm it catches the old behaviour, point it at another copy of the YAML:

```bash
git show origin/main:amplifier-bundle/recipes/workflow-worktree.yaml \
  > "$TMPDIR/workflow-worktree.main.yaml"
WORKTREE_YAML="$TMPDIR/workflow-worktree.main.yaml" \
  bash amplifier-bundle/recipes/tests/test-issue-1565-rerun-preserves-dirty-worktree.sh
# exits non-zero against a YAML that still resets
```

`WORKTREE_YAML` must be a regular file. CI runs the test against the in-repo
YAML in the step "Recipes — re-run never destroys an existing worktree's work
(issue #1565)".

---

## Related

- [Worktree branch deconfliction](../../amplifier-bundle/tools/workflow_worktree_deconflict.md): the helper and its `--base` flag
- [Relative-path-safe, idempotent worktree setup (#1121)](issue-1121-relative-repo-path.md): the three-state guard
- [Leak-proof worktree setup (#840)](issue-840-leak-proof-worktree.md): orphan sweep
