---
title: Troubleshoot Worktree Behavior
description: Diagnose worktree state and restore finalization context or hand off.
last_updated: 2026-11-18
review_schedule: as-needed
owner: workflow-team
doc_type: howto
---

# Troubleshoot Worktree Behavior

Use this guide when amplihack appears to "lose state" between a main
checkout and one of its git worktrees, or when power-steering settings
don't seem to apply across them.

## Before you start

- Confirm you are actually using git worktrees: `git worktree list`.
- Have a shell open in the worktree where the misbehavior is observed.

## Common symptoms and fixes

### Symptom: power-steering re-prompts in every worktree

**Cause:** `AMPLIHACK_RUNTIME_DIR` is set in only one shell, or each
worktree somehow resolves to a different runtime path.

**Diagnose:**

```sh
amplihack doctor
git worktree list
git rev-parse --git-common-dir   # should be identical across worktrees
echo "$AMPLIHACK_RUNTIME_DIR"
```

The `git-common-dir` value must match in every worktree of the same
project. If it does not, you are not actually in a worktree of the same
repo.

**Fix:** unset any per-shell `AMPLIHACK_RUNTIME_DIR` overrides or set the
same value in every shell. Re-run amplihack and answer the
power-steering prompt once; the resulting `.disabled` (or its absence) is
shared automatically.

### Symptom: `AMPLIHACK_RUNTIME_DIR` rejected with `InvalidRuntimeDir`

**Cause:** The path resolves outside `$HOME` and outside `/tmp`. This is
intentional — see the security notes in
[Git Worktree Support](../concepts/worktree-support.md).

**Fix:** point `AMPLIHACK_RUNTIME_DIR` at a path inside your home directory
or `/tmp`, or unset it.

### Symptom: stale runtime data after deleting a worktree

amplihack does not clean up runtime data when you remove a worktree (it
keys off the *main* repo's path, not the worktree path). If the main repo
is also gone, the runtime directory is harmless but orphaned.

**Fix:** remove it manually:

```sh
ls ~/.amplihack/runtime/   # find the hashed directory
rm -rf ~/.amplihack/runtime/<hashed-dir>
```

There is no `amplihack worktree prune` command; use plain `rm`.

### Symptom: code-graph database conflicts between worktrees

The code-graph default path is per-project, so two worktrees may try to
write to the same database simultaneously. Pass `--db-path` per-worktree
to isolate them:

```sh
amplihack index-code export.json --db-path "$(git rev-parse --show-toplevel)/.amplihack/code-graph.lbug"
```

## When to file a bug

If `git rev-parse --git-common-dir` agrees across worktrees but amplihack
still resolves different runtime directories, that is a bug in
`crates/amplihack-utils/src/worktree.rs`. Capture the output of
`amplihack doctor` from each worktree and file an issue.

### Symptom: finalization cannot resolve the recorded worktree

**Cause:** The run's `worktree_setup` output is missing or malformed, or its
recorded worktree is removed or unusable. An open PR does not prove that audit
edits and local commits were published.

**Diagnose:** Read the failing step's log and recovery record. Compare the
recorded `worktree_setup.worktree_path` with `git worktree list` from a surviving
checkout. Retain publication errors, scope exclusions, and any diagnostic report
path for recovery.

**Restore or hand off:**

1. Restore the recorded task worktree and its intended branch, preserving audit
   edits and local commits. If the JSON context was lost, have the parent recipe
   owner restore the complete `worktree_setup` output for the run.
2. Re-execute publication before readiness so missing commits can reach the
   remote branch. Readiness must then validate the exact published HEAD from
   that same worktree.
3. If the worktree, audit work, or parent context cannot be restored, hand off
   with the failing operation, recorded path, publication or scope errors, and
   required owner action. Do not report successful finalization.

Under the [finalization contract](../reference/workflow-finalize-publication.md),
steps 20b and 21 enter essential recovery with stop-on-handoff; an unresolved
handoff stops the run with exit `81`. Recovery performs no GitHub writes. It
repairs supported local state and re-executes the original operation, which owns
the push or validated readiness mutation.

Exporting `WORKTREE_SETUP_WORKTREE_PATH` in a shell does not restore recipe
context: [`worktree_setup_path.sh`](../../amplifier-bundle/tools/worktree_setup_path.sh)
reads only the complete `RECIPE_VAR_worktree_setup` JSON object. An export in a
recovery child cannot update its parent's context.

Only read-only verification (step 19c) can warn and use `REPO_PATH` or the current
directory. Step 20a may warn and skip, but publication must still run its own
artifact checks. Publication cannot succeed by skipping, and readiness must
not substitute another checkout. A failure here calls for context restoration
or handoff, rather than assuming the recipe files are outdated.

See [Resilient Worktree Cleanup](../recipes/issue-647-resilient-worktree-cleanup.md)
for directory requirements by operation.

## See also

- [Git Worktree Support](../concepts/worktree-support.md)
- [Power-Steering Re-enable Prompt](../concepts/power-steering-compaction.md)
- [Diagnose with Doctor](./diagnose-with-doctor.md)
