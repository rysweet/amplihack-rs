# Worktree Branch Deconfliction

**Non-destructive helper that keeps two concurrently-running recipes from
fighting over the same branch name.**

`workflow_worktree_deconflict.sh` is a self-contained shell brick used by the
worktree/branch setup steps of `workflow-worktree.yaml` (step
`step-04-setup-worktree`) and mirrored as guidance in
`consensus-issue-worktree.yaml` (step `step3-setup-worktree`). It inspects the
live `git worktree` topology and, when the branch a recipe wants is already
checked out by a **different** recipe's worktree, hands back a fresh, distinct,
provably-free branch name instead of destroying the other recipe's work.

With `--base <ref>` (issue #1565) it also hands back a fresh name when the
branch at **this** recipe's own path holds work: uncommitted or untracked
changes, or commits ahead of `<ref>`. A re-run then keeps the earlier work and
gets its own branch. See
[Re-runs never destroy an existing worktree's work](../../docs/recipes/issue-1565-rerun-preserves-worktree.md).

- [Why this exists](#why-this-exists)
- [What it guarantees](#what-it-guarantees)
- [Command-line usage](#command-line-usage)
- [The `--base` flag](#the---base-flag)
- [Contract / API](#contract--api)
- [Ownership classification](#ownership-classification)
- [Deconfliction algorithm](#deconfliction-algorithm)
- [Configuration](#configuration)
- [Integration in the recipes](#integration-in-the-recipes)
- [Examples](#examples)
- [Preserved behaviors](#preserved-behaviors)
- [Security model](#security-model)
- [Testing](#testing)

---

## Why this exists

`smart-orchestrator` derives a branch name deterministically from the target
issue number and a slug of the task description
(`feat/issue-<N>-<slug>`). When two orchestrator runs start from the **same
issue number and the same task-description file**, they derive the **identical
branch slug**. Each run computes its own worktree path
(`<repo>/worktrees/<branch>`).

Previously, the setup step's idempotency state machine assumed that a branch it
found was one of its *own* prior runs. In the "State-2" path (branch exists,
worktree missing at *this* recipe's path, and the branch has commits ahead of
base) it ran:

```bash
git branch -D "${BRANCH_NAME}"
```

If that branch was, at that very moment, checked out by a **foreign** worktree
belonging to a concurrently-running recipe at a *different* path, git refused:

```
error: cannot delete branch 'feat/issue-200-fix-bug' used by worktree at
'/repo/worktrees/feat/issue-200-fix-bug'
```

...and the whole recipe aborted. The existing worktree-existence probe only
matched *this* recipe's exact path via `grep -Fx`, so a foreign worktree owning
the branch elsewhere was invisible to the state machine.

A foreign-owned branch breaks the State-2 path at **two** points, both resolved
by renaming this run's branch upstream of the state machine:

- when the branch is **ahead of base**, `git branch -D` fails as shown above; and
- even when it is **clean**, the fallback `git worktree add "${WORKTREE_PATH}"
  "${BRANCH_NAME}"` (attach without `-b`) fails with `fatal: '<branch>' is
  already used by worktree at '<foreign path>'` — git will not check out one
  branch in two worktrees.

This helper detects that foreign ownership **before** any destructive or reuse
decision is made, and resolves it by *renaming this run's branch* rather than
by destroying the other run's branch or worktree.

## What it guarantees

- **Never destructive to foreign work.** The helper contains **zero**
  destructive commands — no `git branch -D`, no `git worktree remove`, no
  `rm -rf`, no `reset --hard`, no `checkout -f`, no `clean`, no `update-ref`,
  no `branch -m`, no `push`. It only reads git state and prints a branch name.
- **Foreign-owned branches are left untouched.** When the desired branch is
  checked out by any worktree at a path other than the intended one, the
  original branch and its worktree are neither deleted nor reused.
- **The chosen name is provably free.** Every returned name passes
  `git check-ref-format --branch`, has **no** local ref (`refs/heads/*`), **no**
  local remote-tracking ref (`refs/remotes/origin/*`; the helper does not
  fetch), and is **not** checked out by **any** worktree.
- **Bounded and loud.** At most `5` candidate suffixes are tried (a hard ceiling
  that `AMPLIHACK_DECONFLICT_MAX_RETRIES` may only *lower*, never raise); on
  exhaustion the helper exits non-zero with a diagnostic rather than looping or
  silently degrading.
- **Same-path resume is preserved for clean worktrees.** If the branch is
  checked out at *this* recipe's own intended path, the helper returns it
  unchanged so the recipe's State-1 reuse path (`created=false`) runs. With
  `--base`, this only happens when that worktree is clean and has no commits
  ahead of the base.
- **Work is never adopted implicitly (`--base`).** A same-path branch with
  changes or commits ahead, or a leftover branch with commits ahead, gets a
  fresh name. If the helper can't prove the branch is clean, it renames.

## Command-line usage

```bash
workflow_worktree_deconflict.sh resolve [--base <base_ref>] <candidate_branch> <intended_worktree_path> [repo_path]
```

| Argument                  | Meaning                                                              |
| ------------------------- | ------------------------------------------------------------------- |
| `--base <base_ref>` (optional) | Base ref for the dirty/ahead checks (e.g. `origin/main`). Must be the first argument after `resolve`. See [The `--base` flag](#the---base-flag). |
| `candidate_branch`        | The branch name the recipe derived (e.g. `feat/issue-200-fix-bug`). |
| `intended_worktree_path`  | The path this recipe intends to attach the worktree to.             |
| `repo_path` (optional)    | Repository root. Defaults to the current working directory.         |

`stdout` carries **only** the resolved branch name (one line, no
decoration) so it is safe to capture with command substitution. All human
diagnostics go to `stderr`.

```bash
NEW_BRANCH="$(bash workflow_worktree_deconflict.sh resolve \
  "feat/issue-200-fix-bug" \
  "/repo/worktrees/feat/issue-200-fix-bug")"
```

### Exit codes

| Code | Meaning                                                                            |
| ---- | ---------------------------------------------------------------------------------- |
| `0`  | A usable branch name was resolved and printed to stdout.                            |
| `1`  | Bad arguments (including `--base` with a missing or empty value), or all bounded retries (`≤ 5`) were exhausted without a free name. |

## The `--base` flag

Without `--base`, `resolve` behaves exactly as it always has: only a
**foreign** branch is renamed. Existing callers are unaffected.

With `--base <base_ref>`, the helper also checks whether a branch it would
otherwise return unchanged holds work:

| Classification | Renamed when |
| -------------- | ------------ |
| **same-path**  | `git --no-optional-locks -C <worktree> status --porcelain` prints anything (modified, staged or untracked; ignored files don't count), **or** `git rev-list --count "<base_ref>..refs/heads/<branch>"` is greater than 0, **or** either command fails. |
| **leftover** (branch exists, no worktree) | `rev-list --count` is greater than 0, or fails. |
| **absent**     | Never. The name is returned unchanged. |
| **foreign**    | Always, as without `--base`. |

The rules that apply when the helper can't prove the branch is clean:

- `<base_ref>` is checked with `git rev-parse --verify --quiet "<base_ref>^{commit}"`.
  If it doesn't resolve, the branch can't be proven clean and is renamed.
- A count that doesn't match `^[0-9]+$` counts as "ahead".
- A missing or prunable worktree directory makes `status` fail, so the branch
  is renamed.

When a branch with work is renamed, the helper writes one line to stderr:

```text
WARNING: keeping existing_branch=feat/issue-1565-fix-issue-1565 at /repo/worktrees/feat/issue-1565-fix-issue-1565 (uncommitted/untracked changes: yes, commits ahead of origin/main: 1); this run uses feat/issue-1565-fix-issue-1565-1760781234-1-48213-4821. To continue the kept branch, re-run with existing_branch=feat/issue-1565-fix-issue-1565.
```

This wording is fixed: the helper prints exactly this line, filling in only the
branch, path, `yes`/`no`, ahead count, base and new name. It never contains
file names or contents. The fresh name comes from the
same loop as foreign deconfliction (see the [algorithm](#deconfliction-algorithm)).
The helper still prints exactly one line on stdout.

## Contract / API

The helper exposes a single logical operation, `resolve`, with this contract:

**Input:** a candidate branch, the intended worktree path, and (optionally) the
repo root.

**Output (stdout):** exactly one branch name that is guaranteed to satisfy all
freshness invariants at the moment of return. It is either:

1. the **unchanged** candidate — when the branch is absent, or is checked out at
   the intended path (genuine same-path resume), or exists but is **not**
   checked out by any worktree (a normal State-2 branch). With `--base`, the
   last two cases apply only when the branch holds no work; or
2. a **new** deconflicted name — when the candidate is foreign-owned, or (with
   `--base`) when it holds work.

**Side effects:** none. The helper only reads git state.

The caller is responsible for recomputing `WORKTREE_PATH` from the returned
branch and proceeding through its normal create path.

## Ownership classification

The helper parses `git worktree list --porcelain` (the stable, machine-readable
form — never the human `git worktree list` output) and pairs each
`worktree <path>` record with its `branch refs/heads/<name>` record. Both the
live worktree path and the intended path are normalized (symlinks and `..`
resolved, `pwd -P`-style, matching the existing issue #858 caller-checkout
logic) before comparison.

| Situation                                                        | Classification | Action without `--base`                 | Action with `--base`                                   |
| ---------------------------------------------------------------- | -------------- | --------------------------------------- | ------------------------------------------------------ |
| Branch does not exist                                            | **absent**     | return unchanged                        | return unchanged                                       |
| Branch exists and is checked out by no worktree                  | **leftover**   | return unchanged                        | new name if ahead of base (or unprovable), else unchanged |
| Branch is checked out at the **intended** path (after normalize) | **same-path**  | return unchanged (State-1 resume)       | new name if dirty or ahead (or unprovable), else unchanged |
| Branch is checked out at a **different** normalized path         | **foreign**    | deconflict → return a new distinct name | deconflict → return a new distinct name                |

## Deconfliction algorithm

When a branch is classified **foreign**, or holds work under `--base`, the
helper generates a new name with one shared, bounded loop (`fresh_name`):

1. Build a short unique suffix `<epoch>-<attempt>-<pid>-<random>` from
   `date +%s`, the 1-based attempt number, the process id and `$RANDOM`
   (for example `1760781234-1-48213-4821`), then sanitize it through
   `tr -cd 'a-z0-9-'` so it can never introduce shell or ref metacharacters.
2. Form a candidate: `<candidate_branch>-<suffix>`, then cap the total length at
   `DECONFLICT_MAX_BRANCH_LEN` (80) by truncating only the **base** — the
   uniqueness-bearing suffix is preserved intact and any resulting trailing
   hyphen is stripped.
3. Validate the candidate with `git check-ref-format --branch`.
4. Reject the candidate unless it is **free** on every axis:
   - no `refs/heads/<name>` (local branch),
   - no `refs/remotes/origin/<name>` (the local remote-tracking ref; the
     helper does not fetch or query the remote, so a branch pushed to
     `origin` since the last fetch is not seen),
   - not checked out by **any** worktree.
5. On the first candidate that passes all checks, print it to stdout and exit
   `0`.
6. Repeat up to the configured bound (default and hard ceiling **5**; see
   [Configuration](#configuration)). If every attempt is exhausted, print a
   diagnostic to stderr and exit `1` (fail loud — never loop unbounded, never
   fall back to a colliding name).

Because every pass re-validates freshness, the astronomically-rare case of two
runs generating the same suffix is caught on the next iteration, which also
uses a new attempt number and `$RANDOM` value.

## Configuration

The helper is intentionally low-configuration. Its behavior is governed by a
single optional environment variable, plus one internal constant:

| Variable                            | Purpose                                             | Default |
| ----------------------------------- | --------------------------------------------------- | ------- |
| `AMPLIHACK_DECONFLICT_MAX_RETRIES`  | Upper bound on suffix retries. Validated `^[0-9]+$` **and clamped to a hard ceiling of `5`** so the override can only *lower* the bound, never weaken the DoS guarantee. Any invalid value falls back to the default. | `5`     |

The internal `readonly DECONFLICT_MAX_BRANCH_LEN` (80) caps the length of a
renamed branch on the deconfliction path — see the [algorithm](#deconfliction-algorithm),
step 2. It is a constant, not an environment knob.

Resolution of the helper path in the recipe follows the same **best-effort
ladder** modeled on `workflow_worktree_sweep.sh` (issue #829/#840 precedent). As
wired in `workflow-worktree.yaml`, the call site probes, in order,
`$TOOLS_DIR/workflow_worktree_deconflict.sh` and then
`$REPO_PATH/amplifier-bundle/tools/workflow_worktree_deconflict.sh` (falling
back to the current directory only when `$REPO_PATH` is unset). `$TOOLS_DIR` is
not a separate ladder: step-04 sets it to the directory where it found
`workflow_worktree_sweep.sh` (`$AMPLIHACK_HOME`, `$REPO_PATH`, the current
directory, `~/.copilot`, `~/.amplihack`), so the deconflict helper is taken
from the same bundle as the sweep helper (see the snippet under
[Integration in the recipes](#integration-in-the-recipes)). A **missing** helper
is guarded by `-f`, and only fails the step when work is present: a clean
State 1, a State 2 with no commits ahead, State 3 and a foreign branch proceed
as before, while a dirty State 1 or an ahead State 2 hits the recipe's own
re-check, which fails loud instead of resetting (see
[Integration in the recipes](#integration-in-the-recipes)). A helper that
**runs but exits non-zero** (retries exhausted, misconfig, or an old copy that
rejects `--base` as a bad argument) makes step-04 print
`ERROR: worktree branch deconfliction failed for '<branch>' …` and exit `1`
instead of falling back to the conflicting name.

## Integration in the recipes

### `workflow-worktree.yaml` — executable path (primary fix)

The call site sits immediately after `WORKTREE_PATH` is computed
(~L292) and **before** the idempotency state machine's
`BRANCH_EXISTS`/`WORKTREE_EXISTS` probes and the State-2 `git branch -D`
(~L308–334):

```bash
# --- Foreign-worktree and kept-work deconfliction (#829/#840, #1565) ---
DECONFLICT_HELPER="$TOOLS_DIR/workflow_worktree_deconflict.sh"
[ -f "$DECONFLICT_HELPER" ] || DECONFLICT_HELPER="${REPO_PATH:-$(pwd)}/amplifier-bundle/tools/workflow_worktree_deconflict.sh"
if [ -f "$DECONFLICT_HELPER" ]; then
  # Fail loud on non-zero exit; let the helper's stderr surface.
  if ! RESOLVED_BRANCH="$(bash "$DECONFLICT_HELPER" resolve --base "$BASE_REF" "$BRANCH_NAME" "$WORKTREE_PATH" "$MAIN_REPO_PATH")"; then
    echo "ERROR: worktree branch deconfliction failed for '${BRANCH_NAME}' ..." >&2
    exit 1
  fi
  # V1: an old helper that echoes '--base' must never become a branch name.
  git check-ref-format --branch "$RESOLVED_BRANCH" >/dev/null 2>&1 || { echo "ERROR: deconflict helper returned an invalid branch name" >&2; exit 1; }
  if [ "$RESOLVED_BRANCH" != "$BRANCH_NAME" ]; then
    # Neutral wording: step-04 cannot tell a foreign-worktree rename from a
    # kept-work rename. The helper's own INFO/WARNING line says which it was.
    echo "INFO: using branch '${RESOLVED_BRANCH}' instead of '${BRANCH_NAME}'." >&2
    BRANCH_NAME="$RESOLVED_BRANCH"
    WORKTREE_PATH="${MAIN_REPO_PATH}/worktrees/${BRANCH_NAME}"
  fi
fi
```

After this call the state machine re-checks the branch it is about to use. A
dirty State 1 or an ahead State 2 can only reach it if the helper was missing
or the worktree changed in between (an old helper that rejects `--base` already
failed the step); in that case step-04 prints
`ERROR: … refusing to reset existing_branch=<branch> …` and exits `1`. The
state machine contains no `checkout -- .`, `clean -fd`, `reset --hard` or
`branch -D`.

Because `BRANCH_NAME` is reassigned before the state machine runs and before the
JSON is printed, the final (possibly deconflicted) name flows automatically into
the step's structured output:

```json
{
  "worktree_path": "/repo/worktrees/feat/issue-200-fix-bug-1760781234-1-51007-20944",
  "branch_name": "feat/issue-200-fix-bug-1760781234-1-51007-20944",
  "base_ref": "origin/main",
  "base_branch": "main",
  "runtime_root": "/tmp/amplihack-runtime/feat/issue-200-fix-bug-1760781234-1-51007-20944",
  "created": true
}
```

Downstream steps (PR creation, runtime-artifact laddering, etc.) consume
`branch_name` from this object, so they target the deconflicted branch with no
further changes.

### `consensus-issue-worktree.yaml` — mirror (guidance)

The consensus setup step is an `agent:` step (`amplihack:worktree-manager`)
whose bash lives inside a natural-language `prompt:` block. The equivalent
deconfliction guidance and helper-invocation snippet are inserted into that
prompt **before** its own `git branch -D` (~L286). This mirror is verified by a
**static parity assertion** (grep-based) rather than a behavioral test, because
the recipe-runner does not execute prompt bash directly.

## Examples

### Two concurrent runs, same slug

```console
$ # Run A already holds feat/issue-200-fix-bug at /repo/worktrees/feat/issue-200-fix-bug
$ bash workflow_worktree_deconflict.sh resolve \
    feat/issue-200-fix-bug /repo-B/worktrees/feat/issue-200-fix-bug /repo-B
INFO: branch 'feat/issue-200-fix-bug' is owned by a foreign worktree at
      /repo/worktrees/feat/issue-200-fix-bug (intended: /repo-B/worktrees/...) — deconflicting.
feat/issue-200-fix-bug-1760781234-1-51007-20944
```

Run B proceeds on `feat/issue-200-fix-bug-1760781234-1-51007-20944`; Run A is untouched and never
sees `git branch -D`.

### Genuine same-path resume (no rename)

```console
$ # This recipe's own worktree already holds the branch at the intended path
$ bash workflow_worktree_deconflict.sh resolve \
    feat/issue-200-fix-bug /repo/worktrees/feat/issue-200-fix-bug /repo
feat/issue-200-fix-bug
```

Unchanged output → the recipe's State-1 reuse path runs and emits
`created=false`.

### Re-run with a dirty worktree at this recipe's path (`--base`)

```console
$ # The earlier run left an untracked file, an edit and an unpushed commit
$ bash workflow_worktree_deconflict.sh resolve --base origin/main \
    feat/issue-1565-fix-issue-1565 /repo/worktrees/feat/issue-1565-fix-issue-1565 /repo
WARNING: keeping existing_branch=feat/issue-1565-fix-issue-1565 at /repo/worktrees/feat/issue-1565-fix-issue-1565 (uncommitted/untracked changes: yes, commits ahead of origin/main: 1); this run uses feat/issue-1565-fix-issue-1565-1760781234-1-48213-4821. To continue the kept branch, re-run with existing_branch=feat/issue-1565-fix-issue-1565.
feat/issue-1565-fix-issue-1565-1760781234-1-48213-4821
```

The earlier worktree and branch are untouched. Without `--base`, the same call
prints `feat/issue-1565-fix-issue-1565` unchanged, as before.

### Branch exists but no worktree owns it (normal State-2)

```console
$ bash workflow_worktree_deconflict.sh resolve \
    feat/issue-200-fix-bug /repo/worktrees/feat/issue-200-fix-bug /repo
feat/issue-200-fix-bug
```

Unchanged output → the recipe attaches a worktree to its leftover branch. With
`--base`, this happens only when the branch has no commits ahead of the base;
otherwise the helper returns a fresh name and the branch is kept.

## Preserved behaviors

The deconfliction gate is purely additive. Every existing behavior remains
intact:

- **State-1 same-path resume** — a clean worktree is reused with
  `created=false`.
- **Issue #200 no stale state, issue #1565 no lost work** — dirty or ahead
  work is kept, and the run moves to a new branch (#1565). Stale state is
  still never reused (#200). To continue a kept branch on purpose, pass
  `existing_branch=<branch>`; see
  [Continuing an earlier branch on purpose](../../docs/recipes/issue-1565-rerun-preserves-worktree.md#continuing-an-earlier-branch-on-purpose).
- **Issue #342 `existing_branch` / `PR_NUMBER` targeting** — explicit
  branch/PR targeting is honored.
- **Issue #858 caller-checkout refusal** — refusing to hijack a branch the
  caller has checked out.
- **Issue #3023 empty-slug hardened fallback** — `feat/task-unnamed-<ts>`.
- **The sanitized slug pipeline** — `tr -cd 'a-z0-9-'`, `git check-ref-format`,
  and the hardcoded `feat/` fallback prefix.

## Security model

- **Command injection:** all expansions are quoted; there is no `eval` or
  backtick re-execution; porcelain output is parsed as data via `awk`; and every
  candidate must clear `git check-ref-format --branch` before use.
- **Non-destructive invariant (hard):** the helper contains no branch/worktree
  deletion, no `rm`, no `reset --hard`, no `checkout -f`, no `clean`, no
  `update-ref` and no `branch -m`. This is enforced by a static test assertion
  (A6).
- **Deny by default (`--base`):** a failed `status` or `rev-list`, a
  non-numeric count or an unresolvable base all lead to a rename, never to
  reuse. `status` runs with `--no-optional-locks` so the helper never takes a
  live worktree's index lock (#1560).
- **Path spoofing / traversal:** both the intended and live worktree paths are
  `pwd -P`-normalized (symlinks and `..` resolved) before equality comparison,
  so a symlinked path cannot masquerade as same-path.
- **Denial of service:** retries are bounded (`≤ 5`) and the helper fails loud
  on exhaustion — it never loops unbounded.
- **No network / auth / secret surface:** read-only local git introspection
  only; no fetch/push/pull and no credential handling.
- **Data minimization:** stdout carries the resolved branch name only;
  worktree-list detail stays on stderr.
- The helper is **shellcheck-gated in CI** to catch unquoted-expansion and
  injection classes automatically.

## Testing

Behavioral and static coverage lives in
`amplifier-bundle/recipes/tests/test-foreign-worktree-deconflict.sh`, wired as a
named step in `.github/workflows/ci.yml` alongside the other per-recipe tests
(and a shellcheck step for the helper). It follows the issue #840 harness
template (`set -euo pipefail`, `extract_step()` awk, `mktemp -d` fixtures,
`trap cleanup EXIT`, pass/fail counters).

It asserts:

1. **Foreign ownership → new branch.** With a foreign worktree holding
   `BRANCH_NAME` at a different path, the step selects a **new**, valid branch,
   succeeds, and **never** calls `git branch -D` on the foreign branch.
2. **Same-path resume still reuses.** A genuine same-path worktree yields
   `created=false` and no new branch.
3. **Consensus parity.** The `consensus-issue-worktree.yaml` prompt carries the
   equivalent deconfliction guidance.
4. **`--base` behaviour.** Covered end to end by
   `amplifier-bundle/recipes/tests/test-issue-1565-rerun-preserves-dirty-worktree.sh`
   (dirty State 1 kept, clean State 1 reused, ahead State 2 kept, unchanged
   output without `--base`, fail-loud with a missing or old helper).
5. **Static invariants.** Non-destructive command absence, porcelain +
   `check-ref-format` usage, bounded-retry, call-site ordering before the state
   machine, and the `< 400`-line budget on `workflow-worktree.yaml`.
