# Ancestry-Aware Step 15 Publish

**`workflow-publish` step 15 integrates with its upstream by branch ancestry — fast-forward when it can, fail closed when histories diverge — and never blind-rebases already-integrated commits.**

> [Home](../index.md) > [Features](README.md) > Ancestry-Aware Step 15 Publish

## Quick Navigation

- [What This Feature Does](#what-this-feature-does)
- [Behavior](#behavior)
- [Structured Divergence Evidence](#structured-divergence-evidence)
- [Brick-Rule Compliance](#brick-rule-compliance)
- [Verification](#verification)
- [Related Docs](#related-docs)

---

## What This Feature Does

`step-15-commit-push` in `amplifier-bundle/recipes/workflow-publish.yaml` is the
publish phase's commit-and-push step. Before this feature, when a temporary
workstream branch tracked an unrelated or stale upstream, the step ran a blind
`git pull --rebase` before pushing. Rebasing onto a diverged upstream replays
commits that were **already integrated**, producing add/add conflicts and, in the
worst case, silently rewriting tested commit identities.

This feature makes step 15 **ancestry-aware**. It refreshes the upstream ref and
then decides what to do by comparing histories, never replaying already-integrated
history:

- **Fast-forwardable** (`behind == 0`, `ahead > 0`) → push directly, preserving the
  exact tested commit identities. No history rewrite.
- **Already published** (`ahead == 0`) → report `already-pushed` and exit cleanly.
- **Diverged** (`ahead > 0` **and** `behind > 0`) → **fail closed** with structured
  merge-base / ahead / behind evidence and require an explicit merge or rebase
  decision. The step never auto-rebases divergent history.

This closes issue
[#978](https://github.com/rysweet/amplihack-rs/issues/978).

---

## Behavior

Step 15 evaluates the branch against its configured upstream (`@{u}`) and selects
exactly one outcome:

| Condition (relative to `@{u}`) | Outcome | Exit | Emitted marker |
| ------------------------------ | ------- | ---- | -------------- |
| No upstream tracking branch configured | Skip push | `0` | `reason="no-upstream"` |
| `ahead == 0` after fetch | Nothing to push | `0` | `reason="already-pushed"` |
| `behind == 0`, `ahead > 0` | Fast-forward push | `0` | `"pushed":"true"` |
| `ahead > 0` **and** `behind > 0` | Fail closed (no rebase) | `1` | `reason="diverged-upstream: …"` |

Key invariants:

- **Commit identity is preserved on the fast-forward path.** `HEAD` is unchanged
  before and after the push, and `origin/<branch>` fast-forwards to the local
  `HEAD`. No `git rebase` is ever run for a fast-forwardable branch.
- **Divergence fails closed.** On divergence the working tree and `HEAD` are left
  untouched — no rewrite, no conflict markers, no partial push. The operator must
  point the branch at its intended PR base and fast-forward, or make an explicit
  rebase decision.
- **Fetch failures are non-fatal.** If `git fetch origin` fails before the ancestry
  check, the step warns and proceeds using cached upstream state rather than
  aborting the publish.
- **Push and fetch output is redacted.** Fetch and push stderr pass through
  `redact_sensitive_file` before being emitted, so tokens never leak into the
  publish log.

---

## Structured Divergence Evidence

When step 15 refuses to rebase a divergent history it emits machine-parseable
ancestry evidence to **stderr** so operators and reviewers can diagnose the base
mismatch without re-deriving it:

```text
ERROR: step-15 refuses to rebase divergent history onto upstream 'origin/feature' (issue #978).
  ancestry: ahead=1 behind=1 merge_base=<sha> head=<sha> upstream=origin/feature
  Auto-rebasing would replay already-integrated commits and create add/add conflicts.
  Point the branch at its intended PR base and fast-forward, or make an explicit rebase decision.
```

The result record on stdout reports the non-success outcome. It is emitted by
`_emit_commit_result` via `jq -nc`, so every field is always present in the
order `pushed, sha, branch, reason`:

```json
{"pushed":"false","sha":"","branch":"feature","reason":"diverged-upstream: ahead=1 behind=1 merge_base=<sha> upstream=origin/feature"}
```

These markers are contract strings. The following substrings are guaranteed and
must not change without updating the regression tests that pin them:

- `issue #978`
- `refuses to rebase divergent history`
- `ahead=`, `behind=`, `merge_base=`
- `"pushed":"true"` on the fast-forward path
- `"reason":"diverged-upstream"` (or `"pushed":"false"`) on divergence

---

## Brick-Rule Compliance

Every phase sub-recipe in `default-workflow` is a **brick**: it must stay under
**400 physical lines**. This limit is enforced by the
`every_phase_subrecipe_under_400_lines` integration test, which counts the lines
of each recipe in `PHASE_RECIPES` plus `default-workflow` and fails any whose
`lines >= BRICK_LIMIT` (`BRICK_LIMIT = 400`). `workflow-publish` is one of the
guarded `PHASE_RECIPES`, so a compliant recipe is at most **399 lines**.

### Current state and the required cleanup

Adding the ancestry-aware logic grew `workflow-publish.yaml` to **423 lines**,
which **exceeds** the 400-line brick limit — the guard is red until the recipe is
trimmed. Bringing it back under the limit is part of this work and is tracked by
[PR #980](https://github.com/rysweet/amplihack-rs/pull/980). PR #980 is
behavior-neutral: it must reduce the physical line count **without changing any of
the contract strings, markers, or fail-closed behavior** described above.

The trim must be a genuine reduction, not a weakening of the guard. Apply, in
order of preference:

1. **Collapse comment banners.** The multi-line `# FIX (#978)` rationale block is
   the primary target — condense it to a one-line pointer to this document. The
   exhaustive narrative lives here, not inline in the recipe.
2. **Keep load-bearing diagnostics executable.** All ancestry markers
   (`issue #978`, `refuses to rebase divergent history`, `ahead=`/`behind=`/
   `merge_base=`, the `_emit_commit_result` fields) must remain in executable
   `echo`/`printf`/`git`/`jq` statements — never move them into comments to save
   lines, because the regression tests assert them at runtime.
3. **Extract, don't inflate.** If condensing comments is not enough, extract
   reusable shell into `amplifier-bundle/tools/` rather than growing the recipe
   body.

Do **not** weaken `BRICK_LIMIT`, delete the guard test, or drop a pinned marker to
get under the limit. Once the trim lands, `workflow-publish.yaml` must be **≤ 399
lines** and the brick guard green.

---

## Verification

Run the size guard and both ancestry regression pins:

```bash
# Brick-rule guard — every phase recipe (incl. workflow-publish) < 400 lines
cargo nextest run -p amplihack every_phase_subrecipe_under_400_lines

# Ancestry behavior pins for issue #978
cargo nextest run -p amplihack \
  step15_fast_forwards_ahead_branch_without_rebase \
  step15_fails_closed_on_divergent_upstream_instead_of_rebasing
```

Expected once the PR #980 trim has landed (see [Brick-Rule
Compliance](#brick-rule-compliance)):

- `every_phase_subrecipe_under_400_lines` — passes; `workflow-publish.yaml` is
  ≤ 399 lines. **Until the trim lands this guard fails** with a
  `brick rule violation` reporting `workflow-publish` at 423 lines, which is the
  expected pre-cleanup state.
- `step15_fast_forwards_ahead_branch_without_rebase` — the branch publishes,
  `"pushed":"true"` is emitted, `HEAD` is unchanged, `origin/<branch>` matches the
  local `HEAD`, and no `Rebasing` appears in stderr.
- `step15_fails_closed_on_divergent_upstream_instead_of_rebasing` — the step exits
  non-zero, emits the `issue #978` / `refuses to rebase divergent history` message
  with `ahead=`/`behind=`/`merge_base=` evidence, reports no successful push, and
  leaves `HEAD` and the working tree untouched.

Confirm CI is green on the pull request:

```bash
gh pr checks 980 --repo rysweet/amplihack-rs
```

The `Test` check must be `SUCCESS` — a `FAILURE` there indicates either an
ancestry regression or that `workflow-publish.yaml` has crossed the 400-line brick
limit.

---

## Related Docs

- [Publish Workflow: Classification-Driven Commit Prefix & Version Bump](../WORKFLOW_PUBLISH_VERSIONING.md) — how the publish phase derives commit prefix and version bump.
- [Workflow Execution Guardrails](workflow-execution-guardrails.md) — canonical execution roots and GitHub identity checks for recipe-driven runs.
- [Workflow-Owned PR Recovery Readiness](pr-recovery-readiness.md) — recovering and finalizing existing PRs through `default-workflow`.
