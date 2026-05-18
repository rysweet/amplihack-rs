# Resilient Worktree Cleanup in Default Workflow (Issue #647)

Late-stage workflow steps (19c, 20b, 21) no longer hard-fail when the worktree
directory has been cleaned up before the finalize phase completes.

---

## Problem

Steps 19c (zero-BS verification), 20b (push-cleanup), and 21 (pr-ready) each
`cd` into `WORKTREE_SETUP_WORKTREE_PATH` using a `${VAR:?}` guard. When the
worktree directory no longer exists — because the agent, a prior step, or an
external process already removed it — the `cd` fails and aborts the recipe with
exit code 1.

This is a false failure. By the time these steps run, the actual work (coding,
testing, committing, pushing, PR creation) is complete. The remaining steps
perform read-only verification and cleanup that can function from the repository
root.

```
# Typical error before this fix:
bash: line 3: cd: /tmp/worktrees/feat-auth-1234: No such file or directory
# Recipe aborts — PR already open, branch already pushed
```

## Solution: Resilient `cd` Fallback Chain

Steps 19c, 20b, and 21 use a three-tier directory resolution instead of a
hard-fail `cd`:

```
WORKTREE_SETUP_WORKTREE_PATH  (preferred — run in the worktree)
        │ not found
        ▼
    REPO_PATH                  (fallback — run from repo root)
        │ not found
        ▼
      $(pwd)                   (final — run from current directory)
```

Each fallback emits a `WARNING` to stderr so the behavior is visible in logs:

```
WARNING: WORKTREE_SETUP_WORKTREE_PATH=/tmp/worktrees/feat-auth-1234 not found, falling back to REPO_PATH
```

### Which Steps Are Resilient

| Step | Recipe | Behavior | Rationale |
|------|--------|----------|-----------|
| 15 (design) | workflow-pr-review | **Hard-fail** | Needs worktree files to review |
| 16 (review) | workflow-pr-review | **Hard-fail** | Needs worktree files to review |
| 18c (enforce-verdict) | workflow-pr-review | **Hard-fail** | Verifies work artifacts in worktree |
| **19c (zero-BS)** | workflow-pr-review | **Resilient** | Read-only checks; can use repo root |
| **20b (push-cleanup)** | workflow-finalize | **Resilient** | Push and cleanup; branch is remote |
| **21 (pr-ready)** | workflow-finalize | **Resilient** | Final status check; reads git remote |

Early-stage steps (15, 16, 18c) retain their `${WORKTREE_SETUP_WORKTREE_PATH:?}`
hard-fail guards. These steps genuinely require the worktree to be present — the
files under review live there.

### Fallback Pattern

**Compact form** (used in `workflow-pr-review.yaml` which is at 399/400 LOC brick limit):

```bash
if [ -d "${WORKTREE_SETUP_WORKTREE_PATH:-}" ]; then cd "$WORKTREE_SETUP_WORKTREE_PATH"; elif [ -d "${REPO_PATH:-}" ]; then echo "WARNING: WORKTREE_SETUP_WORKTREE_PATH=${WORKTREE_SETUP_WORKTREE_PATH:-unset} not found, falling back to REPO_PATH" >&2; cd "$REPO_PATH"; else echo "WARNING: WORKTREE_SETUP_WORKTREE_PATH=${WORKTREE_SETUP_WORKTREE_PATH:-unset} not found, falling back to cwd ($(pwd))" >&2; fi
```

**Multi-line form** (used in `workflow-finalize.yaml` where LOC headroom exists):

```bash
if [ -d "${WORKTREE_SETUP_WORKTREE_PATH:-}" ]; then
  cd "$WORKTREE_SETUP_WORKTREE_PATH"
elif [ -d "${REPO_PATH:-}" ]; then
  echo "WARNING: WORKTREE_SETUP_WORKTREE_PATH=${WORKTREE_SETUP_WORKTREE_PATH:-unset} not found, falling back to REPO_PATH" >&2
  cd "$REPO_PATH"
else
  echo "WARNING: WORKTREE_SETUP_WORKTREE_PATH=${WORKTREE_SETUP_WORKTREE_PATH:-unset} not found, falling back to cwd ($(pwd))" >&2
fi
```

Both forms are semantically identical. The compact form is required because
`workflow-pr-review.yaml` is at 399 of its 400-line brick limit; the multi-line
form is used in `workflow-finalize.yaml` where LOC headroom exists.

## Configuration

No configuration required. The resilient fallback is always active for steps
19c, 20b, and 21. There is no way to force a hard-fail on these steps — the
old behavior was a bug, not a feature.

## Security

- All variables remain double-quoted — no shell injection risk.
- `set -euo pipefail` is preserved in all steps. Only the `cd` target selection
  is resilient; git operations still fail-closed on errors.
- WARNING output goes to stderr only — no information disclosure beyond local
  paths that are already in the recipe log.
- No `|| true`, `>/dev/null 2>&1`, `set +e`, or other error-suppression
  patterns are used. The fallback is explicit conditional logic.
- `REPO_PATH` is typically available via implicit context propagation from the
  parent recipe — no new inputs are added to `workflow-pr-review.yaml`. The
  `$(pwd)` final fallback covers cases where `REPO_PATH` is not set.

## Validation

### Integration Tests

`default_workflow_decomposition_test.rs` validates the hard-fail vs. resilient
split:

| Test target | Assertion |
|-------------|-----------|
| step-18c | Contains `WORKTREE_SETUP_WORKTREE_PATH:?` (hard-fail preserved) |
| step-18c | Contains `set -euo pipefail` |
| step-19c | Contains `set -euo pipefail` |
| step-19c | Contains `WARNING` (resilient fallback emits warning) |
| step-19c | Contains `REPO_PATH` (fallback target) |
| step-19c | Does NOT contain `WORKTREE_SETUP_WORKTREE_PATH:?` with `cd` (no hard-fail) |

### Shell Tests

| Test file | What changed |
|-----------|-------------|
| `tests/issue_412_fail_loud_other_recipes.sh` | `:?` count assertions updated: finalize `2→0`, pr-review `2→1` |
| `tests/issue_414_fail_loud_phase_bricks.sh` | Section B counts, Section C loop membership, Section F resilience assertions updated |

### Running the Tests

```sh
# Rust integration tests
cargo test default_workflow_decomposition -- --nocapture

# Shell assertion tests
bash tests/issue_412_fail_loud_other_recipes.sh
bash tests/issue_414_fail_loud_phase_bricks.sh
```

---

## Related

- [Recipe Resilience](../concepts/recipe-resilience.md) — Branch sanitization, worktree bases, and late-stage resilience
- [worktree_setup Propagation](../reference/worktree-setup-propagation.md) — How `WORKTREE_SETUP_WORKTREE_PATH` flows through recipes
- [Troubleshoot Worktree](../howto/troubleshoot-worktree.md) — Includes entry for recipe abort after early cleanup
- [P1 Workflow Reliability Fixes](./P1_WORKFLOW_RELIABILITY_FIXES.md) — Prior reliability improvements
