# Relative-Path-Safe, Idempotent Worktree Setup (Issue #1121)

`step-04-setup-worktree` now works reliably when the recipe is invoked with a
**relative** `repo_path` (for example `-c repo_path="."`). Re-running against a
worktree that is already registered **and** present on disk no longer aborts the
whole recipe with `git worktree add ... already exists` (exit 128). Setup now
**converges**: an existing, matching worktree is silently **reused**, and a
directory left behind on disk is repaired in place instead of crashing the run.

**Affects:**
- `amplifier-bundle/recipes/workflow-worktree.yaml` — `step-04-setup-worktree`

**Closes:** #1121

---

## Quick Start

No configuration is required. Both absolute and relative `repo_path` values are
now handled identically. The following invocations are equivalent and both
succeed on first run and on every re-run:

```bash
# Absolute repo_path (worked before)
amplihack recipe run default-workflow \
  -c task_description="Fix issue #1234" \
  -c repo_path="$(pwd)"

# Relative repo_path (now equally safe)
amplihack recipe run default-workflow \
  -c task_description="Fix issue #1234" \
  -c repo_path="."
```

Running the setup step a second time when the worktree already exists prints:

```
INFO: Branch 'feat-issue-1234' and worktree '/abs/repo/worktrees/feat-issue-1234' already exist and are clean — reusing.
```

and the step exits `0` with `created=false`. It **never** prints
`fatal: '...' already exists` and **never** exits `128` for an already-present,
matching worktree.

---

## Problem

Each workflow run creates an isolated git worktree under
`${REPO_PATH}/worktrees/<branch>` (see
[Worktree Support](../worktree-support.md)). `step-04-setup-worktree` uses a
three-state idempotency guard to make re-runs safe:

| State | Condition | Action |
| ----- | --------- | ------ |
| 1 | branch **and** worktree both registered | reuse silently (`created=false`) |
| 2 | branch registered, worktree missing | attach a worktree for the branch |
| 3 | neither exists | create branch + worktree |

Two defects combined to break this guard when `repo_path` was relative.

### Defect A — non-canonical path defeats the registration check

The guard derives the target path as
`WORKTREE_PATH="${REPO_PATH}/worktrees/${BRANCH_NAME}"` and tests whether that
worktree is already registered with an **exact** match:

```bash
WORKTREE_EXISTS=$(git worktree list --porcelain \
  | grep -Fx "worktree ${WORKTREE_PATH}" || true)
```

`git worktree list --porcelain` always emits **absolute, canonical** paths
(e.g. `/abs/repo/worktrees/feat-issue-1234`). But with `repo_path="."`,
`WORKTREE_PATH` was `./worktrees/feat-issue-1234`. The `grep -Fx` exact-line
match therefore **never matched** a registered worktree, so a worktree that was
in fact present got misclassified as *missing* (State 2). The same breakage
occurred for any `repo_path` containing `/./`, a trailing slash, or a
symlinked / otherwise non-canonical prefix.

### Defect B — the add path had no existing-directory guard

Once misclassified as State 2 (or State 3), the guard ran a bare
`git worktree add "${WORKTREE_PATH}" ...`. Because the directory was actually
present on disk, git aborted:

```
fatal: '/abs/repo/worktrees/feat-issue-1234' already exists
```

with exit code `128`, which propagated up and marked the **entire recipe run**
as `status: failure`. The existing-branch / PR code path (issue #342) already
guarded its adds against a present-on-disk directory (issue #642); the
three-state guard had regressed that robustness.

---

## Fix

Two changes, both in `step-04-setup-worktree`.

### 1. Canonicalize `repo_path` once, early

Immediately after the step validates it is inside a work tree and `cd`s into
`repo_path`, it now canonicalizes the path:

```bash
cd "$REPO_PATH"
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || { ... }
# Canonicalize so every derived WORKTREE_PATH is absolute and matches
# `git worktree list --porcelain` output (fixes issue #1121 Defect A).
REPO_PATH="$(pwd -P)"
```

Because the step has already `cd`'d into the repo, `pwd -P` yields the absolute,
symlink-resolved canonical path. Every downstream
`WORKTREE_PATH="${REPO_PATH}/worktrees/${BRANCH_NAME}"` is therefore absolute
and canonical, so the `grep -Fx` registration check matches git's porcelain
output. This alone fixes the exact reported failure: a registered **and**
present worktree is now correctly classified as **State 1 (reuse)**.

### 2. Idempotent, guarded `git worktree add`

Every `git worktree add` in the step is now routed through a single shared
helper, `wt_add_idempotent`, defined once near the top of the step. Before
adding, it prunes stale registrations and repairs a present-on-disk directory
instead of crashing:

```bash
# wt_add_idempotent MODE WORKTREE_PATH BRANCH_NAME [REF]
#   MODE ∈ attach | track | create
wt_add_idempotent() {
  local mode="$1" wt="$2" branch="$3" ref="${4:-}"
  git worktree prune 2>/dev/null || true          # drop stale registrations
  if [ -d "$wt" ]; then
    local cur
    cur="$(git -C "$wt" symbolic-ref --short HEAD 2>/dev/null || true)"
    if [ "$cur" = "$branch" ]; then
      return 0                                     # already correct — reuse
    fi
    git worktree remove --force "$wt" 2>/dev/null || rm -rf "$wt"
  fi
  case "$mode" in
    attach) git worktree add -- "$wt" "$branch" ;;
    track)  git worktree add --track -b "$branch" -- "$wt" "origin/$branch" ;;
    create) git worktree add -b "$branch" -- "$wt" "$ref" ;;
  esac
}
```

> **`--` placement matters.** Git stops parsing options at `--`, so every
> option (`-b`, `--track`) must appear **before** the separator. `create`
> therefore uses `add -b "$branch" -- "$wt" "$ref"` (not `add -- "$wt" -b …`,
> which git rejects because `-b` after `--` is read as a pathspec). This
> mirrors the existing `--track` form and preserves the flag-injection guard
> the `--` separator provides.

Key guarantees:

- **Never `git worktree add --force` unconditionally.** A blind `--force` can
  silently clobber a legitimately populated directory. The helper only removes a
  directory *after* confirming its checked-out branch does **not** match the
  target.
- **Reuse when the branch already matches.** A present directory whose `HEAD`
  is the target branch is reused as-is (`return 0`), so no data is lost.
- **Repair when it does not match.** A stale / mismatched directory is removed
  (`git worktree remove --force`, falling back to `rm -rf`) and re-added.
- **Prune first.** `git worktree prune` clears registrations left dangling by a
  partial prior run, so a present-but-unregistered directory still converges.
- **Validation preserved.** The helper is only ever reached after branch names
  pass `git check-ref-format`, and it always uses the `--` end-of-options
  separator, so path/branch values can never be interpreted as flags.

The three-state guard's State 2 and State 3 adds, and the existing-branch /
PR (#342) adds, all call this one helper. Because the several previously
duplicated `git worktree add` invocations collapse into a single function, the
change **adds the guard while net-reducing line count**, keeping
`workflow-worktree.yaml` strictly **under 400 lines** (enforced by check A10 in
`test-issue-840-worktree-leak-proof.sh`).

> **Line-budget watch-item.** `workflow-worktree.yaml` is already **398/400**
> lines, so the guarantee above is tight. The net reduction is only real if the
> helper genuinely collapses the duplicated State-2, State-3, and #342 add-sites
> into single-line calls; a helper that is *added alongside* the existing
> invocations would blow the budget. If collapsing alone does not clear 400
> lines, the documented contingencies are, in order: (1) trim redundant inline
> comments in the step, then (2) externalize the helper to a
> `tools/`-hosted script sourced by the step. Check A10 remains the hard gate
> either way.

---

## Behavior Matrix

With both fixes, `step-04-setup-worktree` converges for every combination of
registration state, on-disk presence, and `repo_path` form:

| `repo_path` | Worktree registered? | Directory on disk? | Branch matches? | Result |
| ----------- | -------------------- | ------------------ | --------------- | ------ |
| `.` or absolute | yes | yes | yes | **State 1** — reuse, exit 0, `created=false` |
| `.` or absolute | yes | yes | no (dirty/ahead) | kept untouched; run moves to a fresh branch, exit 0 ([#1565](issue-1565-rerun-preserves-worktree.md)) |
| `.` or absolute | yes | no | — | **State 2** — attach, exit 0 |
| `.` or absolute | no | yes | yes | prune + reuse, exit 0 |
| `.` or absolute | no | yes | no | prune + remove + re-add, exit 0 |
| `.` or absolute | no | no | — | **State 3** — create, exit 0 |

No combination results in exit 128 or an `already exists` abort.

---

## Preserved Behavior

This fix is surgical. It does **not** change:

- The **#858 caller-checkout rule**: a run never uses the caller's own checkout
  as its task worktree. Since [#1762](issue-1762-caller-branch-worktree-alias.md),
  when the target branch is checked out there, step-04 creates the task worktree
  on a local alias branch `<branch>--wt-<n>` that tracks `origin/<branch>`,
  instead of stopping. A new alias worktree is created with
  `git worktree add --track -b`, not through `wt_add_idempotent`. An existing
  alias is reused only when it is already checked out at its path; otherwise
  step-04 stops. A concurrent create therefore fails on git's ref lock, stops,
  or reuses the other run's worktree. Nothing is removed.
- The **#200 cleanliness resets** — a dirty worktree or a branch with commits
  ahead of `BASE_REF` is still `reset --hard "${BASE_REF}"` before reuse.
- The **#829 / #840 foreign-worktree deconfliction** and the orphan
  [sweep helper](./issue-840-leak-proof-worktree.md).
- The **#342** existing-branch / PR matching (`git worktree list --porcelain`
  awk on the branch ref) and its **#642** existing-directory guard semantics.
- The `created=` / `CREATED` output contract and the step's JSON output.
- Branch-name validation via `git check-ref-format` before any path use.

---

## Regression Test

`amplifier-bundle/recipes/tests/test-issue-1121-relative-repo-path.sh` guards
this behavior. It follows the same idioms as
`test-issue-840-worktree-leak-proof.sh` (repo-root discovery, pass/fail
counters, `set -euo pipefail`, `mktemp -d` + trap cleanup, a `build_repo`
helper, and exit codes `0` pass / `1` fail / `2` harness error). It exercises
the real step body extracted from the recipe (via the same `extract_step` awk
idiom as the #840 test) against a temporary git repo:

**Dynamic scenarios**

1. **Registered + present.** Pre-register and leave on disk a worktree for the
   target branch, then run the step with `repo_path="."`. Asserts the step
   exits `0`, does **not** print `already exists`, and **reuses** the worktree
   (State 1, `created=false`).
2. **Present but unregistered.** Leave a directory on disk for the target
   branch with **no** registration (a stale/partial state). Asserts the guard
   still converges to exit `0` (reuse-if-branch-matches, else remove + re-add),
   never exit `128`.

**Static contract checks**

- (a) step-04 canonicalizes `repo_path` with `pwd -P` after the `cd`.
- (b) the new-branch add path has a prune / existing-directory guard.
- (c) `workflow-worktree.yaml` is still **< 400 lines** (check A10).

Run it directly:

```bash
bash amplifier-bundle/recipes/tests/test-issue-1121-relative-repo-path.sh
# -> exit 0
```

The test is registered in CI (`.github/workflows/ci.yml`) alongside the other
`recipes/tests/*.sh` worktree tests.

---

## Related

- [Leak-Proof, Self-Healing Worktree Setup (Issue #840)](./issue-840-leak-proof-worktree.md)
- [Worktree Support](../worktree-support.md)
- [step-03 Idempotency](./step-03-idempotency.md)
