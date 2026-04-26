# Troubleshoot Worktree Behavior

> **Status: Shipped.** This guide uses only commands that exist today —
> standard `git` plus `amplihack doctor`. There is no `amplihack worktree`
> subcommand and this page does not pretend otherwise.

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

## See also

- [Git Worktree Support](../concepts/worktree-support.md)
- [Power-Steering Re-enable Prompt](../concepts/power-steering-compaction.md)
- [Diagnose with Doctor](./diagnose-with-doctor.md)
