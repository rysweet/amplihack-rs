# Git Worktree Support

> **Status: Shipped (resolution layer only).** amplihack-rs detects whether
> the current directory lives inside a git worktree and resolves a
> **shared runtime directory** that worktrees and the main repo all see.
> There is **no** `amplihack worktree` subcommand and no managed worktree
> root. The implementation lives in
> `crates/amplihack-utils/src/worktree.rs`.

## What this gives you

Power-steering state, hook caches, and any other per-project runtime data
amplihack writes are placed under a single shared directory keyed off the
*main* repository, not the worktree path. So if you have:

```
~/code/myproj          # main checkout
~/code/myproj-feat-x   # git worktree of myproj
```

both end up using the same `~/.amplihack/runtime/<hash-of-main-repo>/`
location and avoid duplicate or stale state.

## How it resolves

`get_shared_runtime_dir(project_root)` in `worktree.rs`:

1. Honors `AMPLIHACK_RUNTIME_DIR` if set, but **rejects** values outside
   `$HOME` or `/tmp` (returns `WorktreeError::InvalidRuntimeDir`).
2. Otherwise asks `git` for the *common* dir (the main repo's `.git`).
3. Hashes that path to produce a stable per-repo key.
4. Creates the directory `0o700` (owner-only) if it does not exist.
5. Caches the answer in-process (LRU, size 128) to avoid re-shelling to
   `git` on every call.

## Today vs. Planned

| Capability                                        | Today        | Planned                |
|---------------------------------------------------|--------------|------------------------|
| Worktree → main-repo runtime resolution           | ✅ shipped   | unchanged              |
| `AMPLIHACK_RUNTIME_DIR` override with safety check | ✅ shipped   | unchanged              |
| Sharing power-steering `.disabled` across worktrees | ✅ shipped | unchanged              |
| `amplihack worktree list/prune/remove` CLI         | ❌ not present | not currently planned |
| Auto-creating worktrees per session                | ❌ not present | out of scope           |

## What this is **not**

This module does **not** create, list, or delete git worktrees. Use
`git worktree …` for that. amplihack only **reads** the worktree topology
to decide where its own runtime files belong.

## Security notes

- The runtime directory is created with mode `0o700`, so other local users
  cannot read your power-steering state or other cached data.
- `AMPLIHACK_RUNTIME_DIR` is canonicalized and validated before use. A
  symlink pointing outside the home directory will be rejected with an
  `InvalidRuntimeDir` error.

## See also

- [Power-Steering Re-enable Prompt](./power-steering-compaction.md)
- [Troubleshoot Worktree Behavior](../howto/troubleshoot-worktree.md)
- `crates/amplihack-utils/src/worktree.rs`
