# Leak-Proof, Self-Healing Worktree Setup (Issue #840)

`default-workflow` worktree setup is now **leak-proof** and **self-healing**.
A failed or aborted run no longer leaves behind an orphaned worktree directory
or branch that blocks future runs, and every subsequent setup best-effort
sweeps orphans left by prior failed runs so re-running always **converges**
(succeeds) instead of erroring.

**Affects:**
- `amplifier-bundle/tools/workflow_worktree_sweep.sh` (new helper)
- `amplifier-bundle/recipes/workflow-worktree.yaml` — `step-04-setup-worktree`

**Closes:** #840

---

## Quick Start

No configuration is required. The behavior is transparent — `step-04-setup-worktree`
sweeps orphaned worktrees before it creates a new one, and a failing run cleans
up after itself.

```bash
# Run the default workflow as usual. Setup self-heals before creating the worktree.
amplihack recipe run default-workflow \
  -c task_description="Fix issue #1234" \
  -c repo_path="$(pwd)"
```

If a prior run crashed and left `worktrees/feat-issue-1234/` behind with no
unmerged work, the next setup logs:

```
INFO: workflow_worktree_sweep: pruned orphan worktree 'worktrees/feat-issue-1234' (stale, no unmerged commits)
```

and proceeds to create a fresh worktree. **Re-running after a failed run is
always safe** — setup converges to a clean, working worktree.

---

## Problem

Each `default-workflow` run creates an isolated git worktree under
`${REPO_PATH}/worktrees/<branch>` (see [Worktree Support](../worktree-support.md)).
Two gaps caused worktrees and branches to leak:

1. **No cleanup on failure.** When a run failed or was aborted partway through,
   its worktree directory and branch were left registered. The next run that
   resolved the same branch name could collide with the leftover state. The
   characteristic error reported in #840 was:

   ```
   error: cannot delete branch 'feat-issue-1234' used by worktree at
   '/path/to/worktrees/feat-issue-1234'
   ```

2. **No orphan sweep.** Nothing reclaimed worktrees left by *previous* failed
   runs. Over time, `worktrees/` accumulated stale directories and dead
   branches, and a colliding leftover could make a fresh setup error out
   instead of converging.

> **Already solved (context).** The collision *state machine* in
> `step-04-setup-worktree` (reuse-if-clean, reset-hard-if-dirty,
> add-worktree-if-branch-exists-but-worktree-missing, full-create) already
> prevents the exact `git branch -D` error, because `git branch -D` only runs
> when the worktree is absent. See
> [step-04 Re-Prune After Orphan Cleanup](step-04-worktree-reattach-prune.md)
> and [Resilient Worktree Cleanup (#647)](issue-647-resilient-worktree-cleanup.md).
> Issue #840 closes the two remaining gaps above: **cleanup-on-failure** and
> the **orphan sweep**.

---

## Solution

The destructive worktree-lifecycle logic lives in a single self-contained shell
brick, `amplifier-bundle/tools/workflow_worktree_sweep.sh`, so the
`workflow-worktree.yaml` recipe stays strictly under its 400-line brick limit
and no new recipe step is introduced (the
[step inventory contract](#contract--constraints) is untouched).

`step-04-setup-worktree` invokes the helper's `sweep` mode best-effort near the
top of the step. The call is a **subprocess CLI invocation** (not a sourced
function), so a failure inside the helper is isolated to its own process and can
never abort the surrounding setup — even though step-04 runs under
`set -e`. The invocation is guarded so a non-zero exit (or a missing helper) is
swallowed:

```bash
# Inside step-04-setup-worktree (best-effort, isolated subprocess):
bash "${AMPLIHACK_HOME}/amplifier-bundle/tools/workflow_worktree_sweep.sh" \
  sweep "$REPO_PATH" || true
```

If the helper is absent (e.g., a partial install), setup proceeds as a graceful
no-op — consistent with the
[#829 graceful-degradation precedent](issue-647-resilient-worktree-cleanup.md).

```
step-04-setup-worktree
        │
        ▼
  best-effort: workflow_worktree_sweep.sh sweep "$REPO_PATH"   ← NEW (#840)
        │  (graceful no-op if helper missing; never aborts setup)
        ▼
  existing three-state idempotency guard (reuse / reset / create)
        ▼
  worktree ready
```

### Two complementary guarantees

| Mechanism | When it runs | What it does |
| --- | --- | --- |
| **Orphan sweep** | Start of every `step-04-setup-worktree` | Prunes stale, mergeable orphans left by prior failed runs. This is the durable, self-healing leak cleanup. |
| **Cleanup-on-failure** | On a run's failure/abort path | Archives any unique commits, then prunes that run's own worktree + branch so it never leaks in the first place. |

The recipe runner executes steps sequentially with abort-on-failure and has no
native `try`/`finally`, so the **orphan sweep at the next setup is the primary,
test-anchored guarantee**. `cleanup_on_failure` is the proactive complement: it
is invoked by an explicit failure-path step in the workflow (not a shell trap),
so it only runs when the runner reaches that step. Because that reachability is
not guaranteed on a hard abort, the next-run orphan sweep remains the durable
backstop — `cleanup_on_failure` reduces, but does not solely guarantee, leak
cleanup.

---

## Helper API — `workflow_worktree_sweep.sh`

`amplifier-bundle/tools/workflow_worktree_sweep.sh` is both **sourceable**
(functions) and **CLI-invocable** (subcommands). It is hardened with
`set -euo pipefail` and a sanitized `IFS`, is `shellcheck`-clean, and uses
`--` terminators on all git commands.

### `sweep <repo_path>`

Best-effort reclamation of orphaned worktrees.

```bash
amplifier-bundle/tools/workflow_worktree_sweep.sh sweep "$REPO_PATH"
```

Algorithm:

1. `git -C "<repo_path>" worktree prune` — clear stale `.git/worktrees/`
   registrations.
2. Enumerate registered worktrees via `git worktree list --porcelain` and
   restrict to entries under `<repo_path>/worktrees/` (realpath
   prefix-containment; symlink / `..` escapes are rejected).
3. For each orphan candidate, remove the worktree directory **and** its branch
   only when **both** are true:
   - **Stale** — directory mtime is older than the staleness threshold
     (default 24h; see [Configuration](#configuration)).
   - **No unmerged meaningful work** — `git rev-list --count <BASE_REF>..HEAD`
     equals `0` (mirrors the existing cleanliness gate at
     `workflow-worktree.yaml` lines 284/300).

**Exit code is always `0`.** Per-orphan failures are logged (`WARN:`) and never
abort the sweep or the surrounding setup step. This is what makes setup
*converge* after a prior failed run.

### `cleanup_on_failure <repo_path> <branch>`

Proactive teardown for a run that is failing or aborting.

```bash
amplifier-bundle/tools/workflow_worktree_sweep.sh cleanup_on_failure "$REPO_PATH" "$BRANCH_NAME"
```

Algorithm:

1. If `<branch>` has unique commits (`<BASE_REF>..<branch> > 0`), archive/push
   them first to `refs/amplihack-archive/<branch>` (and push to the configured
   remote when available). **No destructive operation runs until the archive
   succeeds.**
2. Prune that run's worktree directory and branch.
3. If meaningful unmerged work cannot be archived/pushed, **skip the prune** and
   log a `WARN:` — the branch/worktree is preserved so no work is lost.

### Safety invariant (data-loss gate)

> A worktree or branch with **unmerged meaningful commits is never destroyed.**
> The archive/push must succeed first; only then may a prune proceed.

This gate is enforced in both modes and is covered by the contract test
(see [Tests](#tests)).

### Log prefixes

The helper emits stable, greppable prefixes to **stderr** only:

| Prefix | Meaning |
| --- | --- |
| `INFO: workflow_worktree_sweep: …` | Normal action taken (e.g., orphan pruned) |
| `WARN: workflow_worktree_sweep: …` | Best-effort step skipped or per-orphan failure |

Logs contain only branch and path names — never diff contents, tokens, or
environment values.

---

## Configuration

| Variable | Default | Purpose |
| --- | --- | --- |
| `AMPLIHACK_WORKTREE_STALE_SECS` | `86400` (24h) | Minimum worktree directory age (seconds) before it is eligible for sweeping. Must match `^[0-9]+$`; any invalid value falls back to the default. Set to `0` for deterministic tests (treat every orphan as stale). |
| `BASE_REF` | resolved by setup | Base ref used for the "no unmerged meaningful work" gate (`BASE_REF..HEAD`). Inherited from the recipe context. |

Example — sweep aggressively in a throwaway CI sandbox:

```bash
AMPLIHACK_WORKTREE_STALE_SECS=0 \
  amplifier-bundle/tools/workflow_worktree_sweep.sh sweep "$REPO_PATH"
```

The staleness gate exists to avoid removing a **concurrent** run's live
worktree; lowering it is only safe when no other run is active.

---

## Examples

### Recover from a prior failed run (convergence)

```bash
# Simulate a leaked worktree + branch from a crashed run.
git -C "$REPO_PATH" worktree add "$REPO_PATH/worktrees/feat-issue-1234" \
  -b feat-issue-1234 main
# (no commits made — the crash left it empty)

# Re-run setup. The sweep prunes the stale, mergeable orphan, then setup
# creates a fresh worktree and exits 0.
amplihack recipe run default-workflow \
  -c task_description="Fix issue #1234" -c repo_path="$REPO_PATH"
# ✓ converges — no "cannot delete branch ... used by worktree" error
```

### Preserve unmerged work (safety gate)

```bash
# A leaked worktree that DOES contain unmerged commits.
git -C "$REPO_PATH/worktrees/feat-issue-9999" commit -am "WIP: real work"

AMPLIHACK_WORKTREE_STALE_SECS=0 \
  amplifier-bundle/tools/workflow_worktree_sweep.sh sweep "$REPO_PATH"
# WARN: workflow_worktree_sweep: skipping 'worktrees/feat-issue-9999'
#       (1 unmerged commit ahead of base) — preserved
# The worktree and branch are left intact.
```

---

## Contract & Constraints

The implementation is bound by two compiled assertions in
`tests/integration/default_workflow_decomposition_test.rs`:

| Assertion | Guarantee |
| --- | --- |
| `every_phase_subrecipe_under_400_lines` | Every phase sub-recipe YAML, including `workflow-worktree.yaml`, stays **strictly under 400 lines**. The sweep logic is extracted to the helper script precisely to honor this. The inline guarded call adds 9 lines, taking `workflow-worktree.yaml` from **354 → 363 lines**, comfortably under the 400-line ceiling. |
| `EXPECTED_STEP_INVENTORY` length + order | The step list and order are unchanged. The sweep is an **inline best-effort call inside the existing `step-04-setup-worktree`** — no new step ID is added. |

Verify the contract:

```bash
cargo test -p amplihack --test default_workflow_decomposition
```

---

## Tests

A shell contract test exercises the helper and the recovery path:

```bash
bash amplifier-bundle/recipes/tests/test-issue-840-worktree-leak-proof.sh
```

It builds a real temporary git repository and asserts:

1. **Convergence** — after a simulated prior failed run leaves a colliding
   worktree + branch, setup recovers and exits `0`.
2. **Orphan prune** — `sweep` removes a stale orphan worktree with no unmerged
   work.
3. **Best-effort** — `sweep` exits `0` even when an individual orphan cannot be
   pruned.
4. **Safety gate** — an orphan with unmerged meaningful commits is **preserved**
   (archived, never destroyed).
5. **Adversarial branch names** — names like `--force-me` or `a/../../etc`
   cannot escape `worktrees/` (realpath prefix-containment, `--` terminators,
   `^[A-Za-z0-9._/-]+$` validation, no leading `-`, no `..`).
6. **Graceful no-op** — setup proceeds when the helper is absent.

The test is wired into `.github/workflows/ci.yml` beside the existing
`default-workflow` recipe contract tests, and the helper is `shellcheck`-ed in
the same job.

---

## Security

- All expansions are double-quoted; no `eval`, no unquoted command
  substitution. Worktrees are parsed only via `git worktree list --porcelain`.
- Branch input is validated against `^[A-Za-z0-9._/-]+$`, rejecting a leading
  `-` and any `..`; git commands use `--` terminators to prevent option/command
  injection.
- `AMPLIHACK_WORKTREE_STALE_SECS` is numeric-validated (`^[0-9]+$`) and never
  `eval`-ed; invalid values fall back to the default.
- Deletions are realpath-scoped strictly to `${REPO_PATH}/worktrees/*`
  registered directories; symlink and `..` escapes are blocked.
- The staleness mtime gate prevents removing a live concurrent worktree.
- Archive/push relies on the configured remote auth; tokens are never echoed,
  and logs emit only branch/path names — never diff contents or environment
  values.

---

## Related Documentation

- [step-04 Re-Prune After Orphan Cleanup](step-04-worktree-reattach-prune.md) — Three-state idempotency guard and stale-registration prune
- [Resilient Worktree Cleanup (#647)](issue-647-resilient-worktree-cleanup.md) — Late-stage resilient `cd` fallbacks
- [Recipe Resilience](../concepts/recipe-resilience.md) — Branch sanitization, worktree bases, late-stage resilience
- [Troubleshoot Worktree](../howto/troubleshoot-worktree.md) — Worktree debugging
- [Worktree Support](../worktree-support.md) — Feature overview
- [Create Your Own Tools](../CREATE_YOUR_OWN_TOOLS.md) — The `tools/*.sh` helper-brick pattern
