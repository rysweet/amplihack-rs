# Ancestry-Aware Step 15 Publish

`workflow-publish` step 15 pushes to `origin/<target>` with an explicit
refspec. The step fetches first, then decides by ancestry against
`origin/<target>`: it fast-forwards when possible, fails closed when the
histories have diverged, and never rebases commits that are already
integrated. It never reads the branch's configured upstream (`@{u}`).

> [Home](../index.md) > [Features](README.md) > Ancestry-Aware Step 15 Publish

## Contents

- [What this feature does](#what-this-feature-does)
- [Push target](#push-target)
  - [Default-branch backstop](#default-branch-backstop)
- [Behaviour](#behaviour)
- [Classification helper: `workflow_push_plan.sh`](#classification-helper-workflow_push_plansh)
- [Structured divergence evidence](#structured-divergence-evidence)
- [Other push sites in the publish phase](#other-push-sites-in-the-publish-phase)
- [Brick-rule compliance](#brick-rule-compliance)
- [Verification](#verification)
- [Related docs](#related-docs)

---

## What this feature does

`step-15-commit-push` in `amplifier-bundle/recipes/workflow-publish.yaml` is the
publish phase's commit-and-push step. Two earlier behaviours caused failures:

- **Blind rebase (#978).** When a branch tracked an unrelated or stale
  upstream, the step ran `git pull --rebase` before pushing. That replayed
  commits that were already integrated, which produced add/add conflicts and
  could rewrite tested commit identities.
- **Bare push (#1969).** The step ran a bare `git push`, so the destination
  was whatever `@{u}` named. A branch cut from `origin/dev` with git's default
  `branch.autoSetupMerge=true` had `origin/dev` as its upstream. Under
  `push.default=simple` the push then failed with exit 128 ("The upstream
  branch of your current branch does not match the name of your current
  branch"), and a branch with no upstream was skipped with
  `reason="no-upstream"`, so nothing reached the remote.

Step 15 now does the following:

1. It resolves the destination branch, `TARGET`, without reading `@{u}`.
2. It fetches `origin`.
3. It classifies `HEAD` against `refs/remotes/origin/$TARGET`.
4. It pushes with `git push -u origin "HEAD:refs/heads/$TARGET"`, which also
   sets the upstream to `origin/$TARGET` for later steps.

A wrong or missing upstream can no longer send the work to another branch or
block it. Step 4 also creates new branches with `--no-track`, so a branch cut
from `origin/<base>` no longer gets `origin/<base>` as its upstream. See
[New branches have no upstream until pushed](../recipes/issue-1762-caller-branch-worktree-alias.md#new-branches-have-no-upstream-until-pushed-1969).

---

## Push target

`TARGET` is the output of
[`workflow_remote_branch.sh`](../recipes/issue-1762-caller-branch-worktree-alias.md#resolver-reference-workflow_remote_branchsh)
for the task worktree:

| Task worktree branch | `TARGET` |
|---|---|
| An ordinary branch, for example `feat/issue-197-implement-the-delegation` | `feat/issue-197-implement-the-delegation` |
| A #1762 alias, `<B>--wt-<n>` | `<B>` |
| Detached HEAD | Empty. Step 15 refuses to push. |

The checks below run before anything is staged, in this order. The first
that fails ends the step with no commit and no push, so when two checks would
fail, the earlier one decides the exit status and the `reason`:

| # | Check | Failure output | Exit | `reason` |
|---|---|---|---|---|
| 1 | The resolver is present and exits 0 | Bash's `No such file or directory` with the resolver's path | `2` | (none) |
| 2 | `TARGET` is not empty | `ERROR: cannot resolve the branch to push (detached HEAD?); refusing to push (#1969).` | `1` | `no-push-target` |
| 3 | `TARGET` equals `worktree_setup.branch_name` when that is set (#1762) | `ERROR: push target '<TARGET>' does not match worktree_setup.branch_name '<B>'; refusing to push (issue #1762).` | `1` | (none) |
| 4 | `TARGET` is not a default branch | `ERROR: push target '<TARGET>' is the default branch; step 15 never pushes to it (#1969, #858).` | `1` | `default-branch-target` |
| 5 | `workflow_push_plan.sh` is present | `ERROR: workflow push plan helper not found: <path> (#1969)` | `2` | (none) |

For example, a detached HEAD in an install that lacks `workflow_push_plan.sh`
exits 1 with `reason="no-push-target"`, not 2.

The #1762 guard used to apply only in alias worktrees. It now applies to
every branch. A task worktree whose agent switched to another branch is
therefore refused instead of pushing that branch.

### Default-branch backstop

Check 4 treats `TARGET` as a default branch when any of the following holds:

- it is `main` or `master`;
- the local `refs/remotes/origin/HEAD` resolves and points at
  `refs/remotes/origin/<TARGET>`;
- the local `refs/remotes/origin/HEAD` does not resolve, and `origin`'s own
  `HEAD`, as reported by `git ls-remote --symref origin HEAD`, points at
  `refs/heads/<TARGET>`.

The `ls-remote` query is the only network access before staging. It is
bounded as follows:

- **It runs only when the local ref is missing.** When
  `refs/remotes/origin/HEAD` resolves, step 15 uses it and does not ask
  `origin`, even if the local ref is stale.
- **It cannot prompt or leak.** It runs with `GIT_TERMINAL_PROMPT=0`. Its
  stderr is discarded, so a credential in an error message is never printed.
- **Its output is parsed strictly.** Step 15 uses only a line of the exact
  form `ref: refs/heads/<name>`, a tab, `HEAD`. Every other line is ignored.
  `<name>` is used only if `git check-ref-format --branch <name>` prints it
  back unchanged, and it is only ever compared with `TARGET`, never run.
- **It fails open.** If the query fails, prints no matching line, or the name
  is rejected, only `main` and `master` are refused, and the step
  continues. A remote that cannot answer `ls-remote` will most likely reject
  the push that follows as well, so this risk is accepted.

This rule differs on purpose from the one copied into
`workflow_caller_branch.sh`, `workflow_caller_branch_alias.sh` and
`workflow_remote_branch.sh`:

| | The three helpers | Step 15 |
|---|---|---|
| `main`, `master` | Default | Default |
| Local `refs/remotes/origin/HEAD` target | Default | Default |
| `develop` when the local ref does not resolve | Default | Not a default branch |
| `origin`'s `HEAD`, asked with `ls-remote` | Not consulted | Default, when the local ref does not resolve |

The `detect_and_planner_agree_on_default_branches` test covers the three
helpers only. Step 15's rule is covered by its own tests; see
[Verification](#verification).

Since #858, no run legitimately commits on the default branch, so this check
is a backstop. It matters for a clone whose default branch has another name,
for example `trunk`, and whose `refs/remotes/origin/HEAD` is unset. Step 4's
[caller-branch detection](../recipes/issue-1762-caller-branch-worktree-alias.md#caller-branch-continuation-1969)
uses local refs only, so it cannot recognise `trunk` as the default branch and
may continue it. Step 15 asks `origin` and refuses the push. Refresh the
local ref with `git remote set-head origin --auto`, and protect the default
branch on the server; this check does not replace branch protection.

---

## Behaviour

After the checks above, step 15 either commits the staged changes or takes
the nothing-staged branch. Only then does it run the following, once each and
in this order:

1. `git fetch origin`. A failed fetch prints
   `WARNING: git fetch origin failed before ancestry check; using cached upstream state`
   and the step continues with the refs it already has.
2. `workflow_push_plan.sh "$TARGET"`.
3. The [nothing-staged report](#nothing-staged) (#282), when nothing was
   committed. It uses the `ahead=` field of the plan from step 2.
4. The dispatch on the first word of the plan's output, as in the table below.

The plan must be computed after the commit. A plan computed before it would
classify a branch that was already pushed, and has just gained a commit, as
`already-pushed`, and the new commit would not be pushed.

| Classification | Condition | Message (stderr) | Action | Exit | `commit_result` |
|---|---|---|---|---|---|
| `first-push` | `refs/remotes/origin/<TARGET>` does not exist | `INFO: first push of '<TARGET>' to origin (#1969)` | Push with `-u` | `0` | `"pushed":"true"`, `reason="pushed"` |
| `already-pushed` | `ahead == 0` | `INFO: HEAD is not ahead of origin/<TARGET> after fetch; nothing to push` | None | `0` | `"pushed":"true"`, `reason="already-pushed"` |
| `diverged` | `ahead > 0` and `behind > 0` | The [#978 refusal](#structured-divergence-evidence) | None. No rebase. | `1` | `"pushed":"false"`, `reason="diverged-upstream: …"` |
| `fast-forward` | `ahead > 0` and `behind == 0` | `INFO: origin/<TARGET> is an ancestor of HEAD; fast-forward push, no rebase (#978)` | Push with `-u` | `0` | `"pushed":"true"`, `reason="pushed"` |
| Anything else, or a helper exit other than 0 | The helper's own `ERROR` line | None | `2` | `"pushed":"false"`, `reason="push-plan-failed"` |

The `ahead == 0` rule is checked before the `behind` rule. A branch that is
strictly behind `origin/<TARGET>` is therefore `already-pushed`.

A first push happens even when `HEAD` has no commits beyond the base. Step 4's
setup push normally publishes the branch when it is created, so in practice
this changes nothing on `origin`. Step 16 and the terminal-state checks still
classify a run whose branch carries no real work.

The push is:

```bash
hook_repair_run push step-15-commit-push \
  --fix-message "fix: address {hook_ids} pre-push rejection${ISSUE_NUMBER:+ (#$ISSUE_NUMBER)}" \
  --output "$push_stderr_file" \
  -- git push -u origin "HEAD:refs/heads/$TARGET"
```

After a successful push, `verify_pushed_ref.sh origin "$TARGET" <HEAD sha>`
(#1269) confirms that the published commit is the tested one. If that check
fails, the step exits 1 with `"pushed":"false"` and
`reason="push-verify-failed"`. If the helper cannot be found, the step prints
`WARNING: verify_pushed_ref.sh not reachable; published commit unverified (#1269)`
and continues. A push that fails reports `reason="push-failed: <redacted stderr>"`, or the hook-repair
reason described in
[Recipe commit hook repair](../reference/recipe-commit-hook-repair.md).

Key invariants:

- **The destination is always `refs/heads/<TARGET>` on `origin`.** A stale,
  wrong or missing `@{u}` affects neither where the work goes nor the
  ancestry decision.
- **Commit identity is preserved.** `HEAD` is the same before and after the
  push, and `origin/<TARGET>` ends at the local `HEAD`. Step 15 never runs
  `git rebase` or `git pull`.
- **Divergence fails closed.** The working tree and `HEAD` are left as they
  were. There is no rewrite, no conflict marker and no partial push.
- **No force.** No push uses `--force`, `--force-with-lease`, `--no-verify` or
  a `+` refspec, so `origin` accepts only a fast-forward.
- **Upstream repair.** After a successful push the branch tracks
  `origin/<TARGET>`. This includes branches created before #1969 whose
  upstream was `origin/<base>`, so steps 18c and 20b later in the same run see
  the correct `@{u}`.
- **Output is redacted.** Fetch and push output passes through
  `redact_sensitive_file` before it is printed, and `commit_result` is built
  with `jq --arg`.

### Nothing staged

When there is nothing to commit, step 15 reports how far `HEAD` is ahead
(#282) after the fetch and the push plan, and before the dispatch. The count
comes from the `ahead=` field of the plan, and the line names what it was
counted against:

| Classification | Count | Line ends with |
|---|---|---|
| `first-push` | Commits not on any `origin` branch | `ahead of origin.` |
| Any other | Commits not on `origin/<TARGET>` | `ahead of origin/<TARGET>.` |

The listing shows at most 50 commits. When the count is 0, step 15 prints
`INFO: Nothing staged to commit; continuing so already-pushed meaningful work can be published or classified.`
instead.

From the task worktree of the #1969 report, after its two wip commits and
before the first push, the branch is not yet on `origin`:

```text
INFO: Nothing staged in /repo/worktrees/feat/issue-197-implement-the-delegation, but the
current branch 'feat/issue-197-implement-the-delegation' is 2 commit(s) ahead of origin.
9581b38 wip: delegation receiver model
4f2c0a1 wip: delegation receiver tests
```

On a later run, after a third commit and with `origin/<TARGET>` present, the
second line is:

```text
current branch 'feat/issue-197-implement-the-delegation' is 1 commit(s) ahead of origin/feat/issue-197-implement-the-delegation.
```

---

## Classification helper: `workflow_push_plan.sh`

```text
usage: workflow_push_plan.sh <target>
```

The helper is in `amplifier-bundle/tools/`. It runs in the current directory
and prints one line on stdout:

| Output | Meaning |
|---|---|
| `first-push ahead=<N>` | `refs/remotes/origin/<target>` does not exist. `N` is `git rev-list --count HEAD --not --remotes=origin`. |
| `already-pushed ahead=0 behind=<B>` | `HEAD` has nothing that `origin/<target>` lacks. |
| `diverged ahead=<A> behind=<B> merge_base=<sha or none> upstream=origin/<target>` | Both sides have commits the other lacks. |
| `fast-forward ahead=<A> behind=0` | `origin/<target>` is an ancestor of `HEAD`. |

| Exit | Meaning |
|---|---|
| `0` | One of the lines above was printed. |
| `2` | Usage error; a `<target>` that `git check-ref-format --branch` does not print back unchanged; or a `rev-list` or `merge-base` that failed or printed something other than a number or commit. Nothing is printed on stdout. |

Properties:

- It only reads. It never fetches, pushes, or changes refs, configuration or
  the working tree. Step 15 runs the fetch.
- It never reads `@{u}`.
- Every ref is fully qualified (`refs/remotes/origin/<target>^{commit}`,
  `refs/remotes/origin/<target>..HEAD`, `HEAD..refs/remotes/origin/<target>`,
  `merge-base HEAD refs/remotes/origin/<target>`). A tag or local branch
  called `origin/<target>` therefore cannot make a branch look pushed or
  change the base it is compared with.
- It starts with `set -euo pipefail`, unsets `GIT_DIR`, `GIT_WORK_TREE` and
  `GIT_INDEX_FILE`, and is compatible with bash 3.2.

Example, from the task worktree of the #1969 report after its two wip commits
and before step 15's first push:

```bash
P="${AMPLIHACK_HOME:-$HOME/.amplihack}/amplifier-bundle/tools/workflow_push_plan.sh"
cd /repo/worktrees/feat/issue-197-implement-the-delegation
git fetch origin
bash "$P" feat/issue-197-implement-the-delegation
# first-push ahead=2
```

---

## Structured divergence evidence

When step 15 refuses to rebase divergent history, it prints evidence that
can be parsed to **stderr**, so that the base mismatch can be diagnosed
without working it out again:

```text
ERROR: step-15 refuses to rebase divergent history onto upstream 'origin/feat/issue-197-implement-the-delegation' (issue #978).
  ancestry: ahead=1 behind=1 merge_base=<sha> head=<sha> upstream=origin/feat/issue-197-implement-the-delegation
  Auto-rebasing would replay already-integrated commits and create add/add conflicts.
  Point the branch at its intended PR base and fast-forward, or make an explicit rebase decision.
```

The result record on stdout reports the outcome. `_emit_commit_result` writes
it with `jq -nc`, so every field is present, in the order
`pushed, sha, branch, reason`. `branch` is the local branch, which is the
alias in an alias worktree.

```json
{"pushed":"false","sha":"","branch":"feat/issue-197-implement-the-delegation","reason":"diverged-upstream: ahead=1 behind=1 merge_base=<sha> upstream=origin/feat/issue-197-implement-the-delegation"}
```

The `upstream=` field always names `origin/<TARGET>`, never `@{u}`.

These strings are contracts. The regression tests pin them, and they must not
change unless the tests change too:

- `issue #978`
- `refuses to rebase divergent history`
- `ahead=`, `behind=`, `merge_base=`, `upstream=origin/`
- `"pushed":"true"` on the push paths
- `"reason":"diverged-upstream` (with `"pushed":"false"`) on divergence
- `first push of '` and `(#1969)` on a first push
- `"reason":"no-push-target"` and `cannot resolve the branch to push` on a detached HEAD
- `"reason":"default-branch-target"` and the full line `ERROR: push target '<TARGET>' is the default branch; step 15 never pushes to it (#1969, #858).` on the default-branch refusal

The string `no-upstream` no longer appears in step 15.

---

## Other push sites in the publish phase

The step-16b outside-in fix loop prompt gives the agent this retry command:

```bash
rb="$(bash "${GIT_IDENTITY_HELPER%/*}/workflow_remote_branch.sh" .)" && [ -n "$rb" ] || exit 1
git pull --rebase origin "$rb" && git push -u origin "HEAD:refs/heads/$rb"
```

The `[ -n "$rb" ]` guard matters: on a detached HEAD the resolver exits 0
with no output, and without the guard the push would become
`git push -u origin HEAD:refs/heads/`. With it, the command exits 1 before
pulling or pushing.

Step 15 has already published `origin/<rb>`, so the explicit pull source
exists. No `git push` or `git pull` in `workflow-publish.yaml` relies on
`@{u}`.

Steps outside the publish phase are not covered by this page:

- `step-18c-push-feedback-changes` and `step-20b-push-cleanup` still run a
  bare `git push` when `@{u}` is set. Step 15's `-u` sets the correct upstream
  earlier in the same run. A run that skips step 15 on a branch created before
  #1969 can still meet the old upstream.
- `consensus-publish.yaml` and `consensus-pr-feedback.yaml` use bare pushes.

---

## Brick-rule compliance

Every phase sub-recipe in `default-workflow` is a brick and must stay under
400 physical lines. The `every_phase_subrecipe_under_400_lines` integration
test enforces this with `line_count < BRICK_LIMIT` and `BRICK_LIMIT = 400`.
`workflow-publish` and `workflow-worktree` are among the guarded
`PHASE_RECIPES`, so each is at most 399 lines, and `issue_1969_static.rs`
asserts the same `< 400` bound. Both files run at the limit, so any line added
to step 15 or to step 4 has to be offset by a line removed elsewhere in the
same file. In `workflow-publish.yaml` the lines for the default-branch
backstop and its `ls-remote` query are paid for by deleting the old REL-002
no-upstream block and the `@{u}` comparisons that step 15 no longer uses. The
file must end shorter than it started. Contract strings are never shortened
or merged to save lines.

The classification logic lives in `workflow_push_plan.sh` so that the recipe
stays within the limit. The contract strings listed under
[Structured divergence evidence](#structured-divergence-evidence) stay in
executable `echo`, `printf` or `jq` statements in the recipe. They are never
moved into comments, because the tests check them at run time.

---

## Verification

```bash
# Brick-rule guard: every phase recipe, including workflow-publish, is under 400 lines
cargo test --test integration every_phase_subrecipe_under_400_lines

# Ancestry pins (#978)
cargo test --test integration issue_978

# First push, wrong upstream, stale @{u}, missing helpers and static guards (#1969)
cargo test --test integration issue_1969
```

The #1969 tests use a fixture repository whose `origin` is a local bare
repository:

| Case | Setup | Expected |
|---|---|---|
| New branch, no setup push | Branch created by step 4 with `--no-track`; setup push skipped | `branch.<b>.merge` is unset after step 4. Step 15 prints `first push of '<b>'`, exits 0 with `reason="pushed"`; `origin/<b>` equals `HEAD`; `@{u}` is `origin/<b>`; `origin/<base>` is unchanged. |
| Upstream preset to `origin/<base>` | `git branch --set-upstream-to=origin/<base>` | The push goes to `origin/<b>` and exits 0, not 128. |
| New commit on a pushed branch | `origin/<b>` equals `HEAD`; step 15 then has a staged change to commit | The plan is computed after the commit, so it is `fast-forward ahead=1`, not `already-pushed`. The new commit reaches `origin/<b>`. |
| Diverged from `origin/<TARGET>` | `origin/<b>` has a commit `HEAD` lacks, and `HEAD` has one of its own | Exit 1, `reason="diverged-upstream: …"`, with the #978 evidence. Nothing is rebased or pushed. |
| Only behind `origin/<TARGET>` | `origin/<b>` has a commit `HEAD` lacks; `HEAD` has none of its own | Exit 0, `reason="already-pushed"`. Nothing is pushed. |
| Stale `@{u}` | `@{u}` names a ref unrelated to `origin/<b>` | Decision and destination follow `origin/<b>`. |
| Missing `workflow_push_plan.sh` | Helper removed from the tools directory | Exit 2 before any commit or push. |
| Nothing staged before the first push | Two commits, nothing staged, no `origin/<b>` | The #282 line ends `2 commit(s) ahead of origin.` |
| Unrecognised default branch | The bare `origin`'s `HEAD` points at `refs/heads/trunk`; the clone's `refs/remotes/origin/HEAD` is unset; the task worktree is on `trunk` | Step 15 prints `ERROR: push target 'trunk' is the default branch; step 15 never pushes to it (#1969, #858).`, exits 1 with `reason="default-branch-target"` before staging; `origin/trunk` is unchanged. |
| Check order | Detached HEAD and `workflow_push_plan.sh` removed | Exit 1 with `reason="no-push-target"`, not exit 2. |
| Hostile branch name | A branch name containing a single quote and `$(…)` | `commit_result` is valid JSON and nothing is executed. |

`tests/integration/issue_1762_caller_branch_alias/issue_1969_static.rs` also
checks the recipe text: after `#` comment lines and `echo` lines are
dropped, every `git push` in `workflow-publish.yaml` is followed by
`-u origin "HEAD:refs/heads/`, every `git pull` is `git pull --rebase origin "`,
and none of `--force`, `--force-with-lease`, `--no-verify`, `--mirror` or
`--delete` appears.

---

## Related docs

- [Caller branch continuation and explicit push (#1969)](../recipes/issue-1969-caller-branch-and-explicit-push.md): summary of the #1969 changes to steps 4 and 15.
- [Caller branch worktree alias (#1762, #1969)](../recipes/issue-1762-caller-branch-worktree-alias.md): the remote-branch resolver, alias worktrees, `--no-track` and caller-branch continuation.
- [Recipe commit hook repair](../reference/recipe-commit-hook-repair.md): how a rejecting pre-push hook is repaired before step 15 retries the push.
- [Publish Workflow: Classification-Driven Commit Prefix & Version Bump](../WORKFLOW_PUBLISH_VERSIONING.md): how the publish phase derives the commit prefix and version bump.
- [Workflow Execution Guardrails](workflow-execution-guardrails.md): canonical execution roots and GitHub identity checks for recipe runs.
- [Workflow-Owned PR Recovery Readiness](pr-recovery-readiness.md): recovering and finalizing existing PRs through `default-workflow`.
