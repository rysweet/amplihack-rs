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

The ancestry check lives in `amplifier-bundle/tools/workflow_sync_pr_head.sh`.
Step 15 calls it with `--mode step15`; the `publish-sync-pr-head` step calls the
same helper with `--mode pre-probe` before the terminal-state probe (issue
[#1575](https://github.com/rysweet/amplihack-rs/issues/1575)), so both apply one
policy.

Step 15 first checks that an upstream is configured (REL-002). The helper then
fetches `<remote> <branch>`, where the remote is `branch.<branch>.remote` or
`origin`, compares `HEAD` with the fetched commit, and selects exactly one
outcome:

| Condition (relative to `<remote>/<branch>`) | Outcome | Exit | Emitted marker |
| ------------------------------------------- | ------- | ---- | -------------- |
| No upstream tracking branch configured | Skip push | `0` | `reason="no-upstream"` |
| `ahead == 0` after fetch | Nothing to push | `0` | `reason="already-pushed"` |
| `behind == 0`, `ahead > 0` | Fast-forward push | `0` | `"pushed":"true"` |
| `ahead > 0` **and** `behind > 0` | Fail closed (no rebase) | `1` | `reason="diverged-upstream: …"` |

Key invariants:

- **Commit identity is preserved on the fast-forward path.** The push is
  `git push <remote> HEAD:refs/heads/<branch>`; `HEAD` is unchanged and the
  remote branch fast-forwards to it. `verify_pushed_ref.sh` then confirms the
  remote ref equals `HEAD`.
- **Divergence fails closed.** The working tree and `HEAD` are left untouched:
  no rewrite, no conflict markers, no partial push. The helper prints recovery
  commands but never runs a rebase or a force-push.
- **Fetch failures are non-fatal in step 15.** If the fetch fails, the step
  warns and compares against the tracking ref `refs/remotes/<remote>/<branch>`.
- **Protected branches are refused** before any network call: `main`,
  `master`, the remote's default branch and the run's base branch.
- **git output is redacted.** Fetch and push stderr have URL credentials and
  GitHub tokens removed before they are printed or stored in `reason`.

The helper's arguments, result line and exit codes are in the
[Workflow Publish Helpers Reference](../reference/workflow-publish-helpers.md#workflow_sync_pr_headsh).

---

## Structured Divergence Evidence

When step 15 refuses to rebase a divergent history it emits machine-parseable
ancestry evidence to **stderr** so operators and reviewers can diagnose the base
mismatch without re-deriving it:

```text
ERROR: step-15 refuses to rebase divergent history onto upstream 'origin/feature' (issue #978).
  ancestry: ahead=1 behind=1 merge_base=<sha> head=<sha> upstream=origin/feature remote_head=<sha>
  Auto-rebasing would replay already-integrated commits and create add/add conflicts.
  Point the branch at its intended PR base and fast-forward, or make an explicit rebase decision.
```

The result record on stdout reports the non-success outcome. It is emitted by
`_emit_commit_result` via `jq -nc`, so every field is always present in the
order `pushed, sha, branch, reason`:

```json
{"pushed":"false","sha":"","branch":"feature","reason":"diverged-upstream: ahead=1 behind=1 merge_base=<sha> upstream=origin/feature"}
```

After the evidence, the helper prints a hint and the commands to resolve the
divergence by hand:

```text
  Hint: ahead and behind together often mean a commit that was already pushed was amended locally.
  Recovery, run by hand (this helper never rebases or force-pushes):
    git fetch origin
    git log --oneline @{u}..HEAD
    git log --oneline HEAD..@{u}
    then either: git rebase @{u}
    or, if your local commits replace the remote ones: git push --force-with-lease
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

Keep `workflow-publish.yaml` at 399 lines or fewer:

1. **Collapse comment banners.** Long rationale belongs in this document; the
   recipe keeps a one-line pointer.
2. **Keep load-bearing diagnostics executable.** The ancestry markers above
   must stay in executable `echo`/`printf` statements in
   `workflow_sync_pr_head.sh`, never in comments, because the regression tests
   assert them at runtime.
3. **Extract, don't inflate.** Move reusable shell into
   `amplifier-bundle/tools/`, as the ancestry check itself was moved into
   `workflow_sync_pr_head.sh`.

Do **not** weaken `BRICK_LIMIT`, delete the guard test, or drop a pinned marker
to get under the limit.

---

## Verification

Run the size guard and both ancestry regression pins:

```bash
# Brick-rule guard — every phase recipe (incl. workflow-publish) < 400 lines
cargo test -p amplihack --test integration every_phase_subrecipe_under_400_lines

# Ancestry behavior pins for issue #978
cargo test -p amplihack --test integration issue_978
```

Expected results:

- `every_phase_subrecipe_under_400_lines` passes; `workflow-publish.yaml` is
  399 lines or fewer.
- `step15_fast_forwards_ahead_branch_without_rebase`: the branch publishes,
  `"pushed":"true"` is emitted, `HEAD` is unchanged, the remote branch matches
  the local `HEAD`, and no `Rebasing` appears in stderr.
- `step15_fails_closed_on_divergent_upstream_instead_of_rebasing`: the step
  exits non-zero, prints the `issue #978` / `refuses to rebase divergent
  history` message with `ahead=`/`behind=`/`merge_base=` evidence, reports no
  successful push, and leaves `HEAD` and the working tree untouched.

These tests run step 15 with `AMPLIHACK_HOME` set to the workspace root so the
helpers are found in the checkout, not in an installed copy.

---

## Related Docs

- [Publish: PR Head Sync and Task-Scoped Commits](workflow-publish-task-attribution.md): the pre-probe sync step that applies this ancestry check before the terminal-state probe, and step-15 staging.
- [Publish Workflow: Classification-Driven Commit Prefix & Version Bump](../WORKFLOW_PUBLISH_VERSIONING.md) — how the publish phase derives commit prefix and version bump.
- [Workflow Execution Guardrails](workflow-execution-guardrails.md) — canonical execution roots and GitHub identity checks for recipe-driven runs.
- [Workflow-Owned PR Recovery Readiness](pr-recovery-readiness.md) — recovering and finalizing existing PRs through `default-workflow`.
