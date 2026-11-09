# Local Worktree Archives and the Sweep Lock (Issue #2061)

**Type**: Reference
**Applies to**: `amplifier-bundle/tools/workflow_worktree_sweep.sh`, called from
`step-04-setup-worktree` in `amplifier-bundle/recipes/workflow-worktree.yaml`

The orphan sweep in step 4 records unmerged work on stale branches under
`refs/amplihack-archive/<branch>` in the local clone only. It never contacts a
remote, so it never runs the repository's `pre-push` hook. Each repository has
one sweep lock; a second sweep that starts while the first is running skips
and exits `0`.

## Contents

- [Background](#background)
- [Archive behaviour](#archive-behaviour)
- [Sweep lock](#sweep-lock)
- [Log messages](#log-messages)
- [Inspecting and restoring archives](#inspecting-and-restoring-archives)
- [Removing archive refs left on a remote](#removing-archive-refs-left-on-a-remote)
- [Finding the holder of the lock](#finding-the-holder-of-the-lock)
- [Configuration](#configuration)
- [Tests](#tests)
- [Related documentation](#related-documentation)

## Background

Before #2061, `archive_branch` ran `git ls-remote` against `origin` and then
pushed either `refs/heads/<branch>` or the archive ref. In amplihack-rs the
`pre-push` hook runs `cargo test --workspace`, so every step 4 with a stale,
unmerged worktree started the full workspace test suite, and parallel runs
started it several times over. Lanes worked around this by exporting
`AMPLIHACK_WORKTREE_STALE_SECS=315360000`, which turns the sweep off.

Archiving and pruning are now local operations. The data-loss guard from
[#840](issue-840-leak-proof-worktree.md#safety-invariant-data-loss-gate) is
unchanged: a worktree with unmerged commits, or with uncommitted or untracked
files, is never removed.

## Archive behaviour

`archive_branch <repo> <branch> <sha>` is called by `sweep` for each stale
worktree with unmerged commits, and by `cleanup_on_failure` before it prunes.

| Situation | Action | Return |
|---|---|---|
| `<sha>` is not 40 or 64 lowercase hex characters | Nothing written; `WARN: Refusing to archive ...` logged | `1` |
| `refs/amplihack-archive/<branch>` already holds `<sha>` | Nothing written, so the reflog is unchanged; `INFO ... already current` logged | `0` |
| The ref holds a commit that `<sha>` does not contain (the branch was rewritten or its name reused) | Nothing written; `WARN: Keeping <ref> at <old>: ...` logged, so the archived commits stay reachable | `1` |
| The ref is absent or holds an ancestor of `<sha>` | `git update-ref -m "amplihack worktree archive" <ref> <sha> <old>`, a compare-and-swap against the value just read | `0` |
| The compare-and-swap fails, but a re-read shows `<sha>` | Treated as success (another process wrote the same value) | `0` |
| The update fails for any other reason, for example a ref-name conflict with an existing `refs/amplihack-archive/feat` | `WARN Could not archive ...` logged | `1` |

A return of `1` keeps the worktree and branch. In `cleanup_on_failure` it also
stops the prune, with
`WARN: cleanup_on_failure: could not archive unique work on '<branch>'; refusing to prune (data-loss guard).`

The script runs no `git push`, `git fetch`, `git ls-remote` or
`git remote get-url`, and logs no remote URL.

### Consequence for unmerged work

Archive refs no longer leave the machine. Unmerged work on a stale branch now
exists in two places, both in the local clone: the kept worktree and branch,
and the archive ref. If the clone is deleted, that work is gone. Push the
branch yourself if you want a copy on the remote.

## Sweep lock

`main`'s `sweep` mode calls `sweep_locked`, which wraps `sweep` in a
non-blocking `flock` on:

```text
<common git dir>/amplihack-worktree-sweep.lock
```

The common git dir is resolved to an absolute, symlink-free path
(`common_git_dir`, using `pwd -P`), so the main checkout and every linked
worktree of a repository share one lock file.

| Condition | Behaviour |
|---|---|
| Lock acquired | The sweep runs; the lock is released when the sweep process and any child still holding the descriptor have exited |
| Lock held by another process | `INFO: sweep skipped: lock held: <abs path>`; exit `0`. No `worktree prune`, no `info/exclude` edit, no archive, no removal |
| `flock` not on `PATH` | `WARN`; the sweep runs without a lock |
| The common git dir cannot be resolved | `WARN`; the sweep runs without a lock |
| The lock path is a symbolic link | `WARN`; the sweep runs without a lock and does not write to the link target |
| The lock file cannot be opened (for example, the path is a directory or read-only) | `WARN`; the sweep runs without a lock |
| `flock` fails for a reason other than contention (for example `ENOLCK` on a filesystem without lock support) | `WARN`; the sweep runs without a lock |

`flock` runs with `-E 75`, so only exit code `75` (contention) causes a skip. The lock is opened in append mode, so an existing file is never truncated.
There is no wait, timeout or retry: a sweep that finds the lock held does
nothing and lets the setup step continue. Sourcing the script opens no file
descriptor; the lock exists only in the `sweep` subcommand.

`cleanup_on_failure` takes no lock. It acts on one named branch, and the
compare-and-swap in `archive_branch` keeps it safe if it overlaps a sweep.

Running unlocked is the pre-#2061 behaviour. Two unlocked sweeps apply the same
stale, clean and merged checks, and both run only local git commands.

## Log messages

All messages go to stderr.

| Message | Meaning |
|---|---|
| `WARN: Refusing to archive '<branch>': '<sha>' is not a full object name.` | The SHA argument failed the format check; nothing written, the worktree is kept |
| `INFO: Archived '<branch>' (<sha>) to refs/amplihack-archive/<branch>.` | Archive ref created or moved |
| `INFO: archive refs/amplihack-archive/<branch> already current at <sha>` | Archive ref already held the SHA; nothing written |
| `WARN: Keeping refs/amplihack-archive/<branch> at <old>: '<branch>' (<sha>) does not contain it; not archived.` | The archive would move backwards or sideways; nothing written, the worktree is kept |
| `WARN: Could not archive '<branch>' to refs/amplihack-archive/<branch> locally. git: <error>` | `update-ref` failed; the git error follows `git:` and the worktree is kept |
| `INFO: sweep skipped: lock held: <abs path>` | Another sweep of this repository holds the lock |
| `WARN: flock not found; running sweep without a lock.` | util-linux `flock` is missing |
| `WARN: cannot open lock <abs path>; running sweep without a lock.` | The lock file could not be opened |
| `WARN: lock path <abs path> is a symlink; running sweep without a lock.` | Symlinked lock path refused |
| `WARN: cannot lock <abs path> (flock exit <n>); running sweep without a lock.` | `flock` failed for a reason other than contention |
| `WARN: could not resolve the common git dir of '<repo>'; running sweep without a lock.` | `git rev-parse --git-common-dir` failed |

## Inspecting and restoring archives

List the archive refs in a clone:

```bash
git -C "$REPO_PATH" for-each-ref --format='%(objectname:short) %(refname)' refs/amplihack-archive/
# 3f9c2a1 refs/amplihack-archive/feat/issue-1234-retry-on-lock
```

Show when each archive was written (reflogs exist when
`core.logAllRefUpdates` is `always`):

```bash
git -C "$REPO_PATH" reflog show refs/amplihack-archive/feat/issue-1234-retry-on-lock
# 3f9c2a1 refs/amplihack-archive/feat/issue-1234-retry-on-lock@{0}: amplihack worktree archive
```

Recreate a branch from an archive:

```bash
git -C "$REPO_PATH" branch feat/issue-1234-retry-on-lock \
  refs/amplihack-archive/feat/issue-1234-retry-on-lock
```

Delete an archive you no longer need:

```bash
git -C "$REPO_PATH" update-ref -d refs/amplihack-archive/feat/issue-1234-retry-on-lock
```

## Removing archive refs left on a remote

Sweeps before #2061 pushed `refs/amplihack-archive/*` to `origin`. The sweep
does not remove them. To list and delete them by hand:

```bash
git ls-remote origin 'refs/amplihack-archive/*'
git push origin --delete refs/amplihack-archive/feat/issue-1234-retry-on-lock
```

The delete is an ordinary push and runs the `pre-push` hook.

## Finding the holder of the lock

A sweep that keeps logging `sweep skipped: lock held` points at a process that
still has the lock file open, for example a detached git child that inherited
the descriptor. Nothing is removed while the lock is held. Pass the absolute
path from the message to `fuser`:

```bash
fuser -v /home/dev/src/amplihack-rs/.git/amplihack-worktree-sweep.lock
```

To build the path from a clone instead, resolve the common git dir first.
`--path-format=absolute` needs git 2.31 or later:

```bash
fuser -v "$(git -C "$REPO_PATH" rev-parse --path-format=absolute --git-common-dir)/amplihack-worktree-sweep.lock"
```

## Configuration

No new variables. The sweep reads the same settings as before:

| Variable | Default | Effect |
|---|---|---|
| `AMPLIHACK_WORKTREE_STALE_SECS` | `86400` | Minimum worktree age before the sweep considers it. See [#840 configuration](issue-840-leak-proof-worktree.md#configuration) |
| `AMPLIHACK_WORKTREE_BASE_REF` | resolved by setup | Base ref for the merged check |

Lanes that export `AMPLIHACK_WORKTREE_STALE_SECS=315360000` to stop the sweep
pushing can drop that export once this change is installed.

## Tests

```bash
bash amplifier-bundle/recipes/tests/test-issue-2061-sweep-no-push.sh
```

The test builds a bare origin and a clone in a temporary directory, isolates
`HOME` and git configuration, unsets every `GIT_*` discovery variable, and sets
`AMPLIHACK_WORKTREE_STALE_SECS=0` itself, so a lane's stopgap value does not
affect it. The clone has a `pre-push` hook that records every call to a log;
the test first proves the hook fires, then asserts the log stays empty in every
scenario.

| Scenario | Asserts |
|---|---|
| A | First sweep creates the archive ref with one reflog entry, keeps the worktree, pushes nothing |
| B | Second sweep logs `already current`, leaves the reflog at one entry, pushes nothing |
| C | A new commit moves the archive ref; the reflog has two entries |
| D | With the branch already on `origin`, `git ls-remote origin` is unchanged and no remote archive ref appears |
| E | With the lock held, the sweep logs the skip with the absolute path and changes no ref or worktree; a linked worktree path resolves to the same lock; the sweep runs after release |
| F | With the lock path a directory, the sweep warns, archives and exits `0` |
| G | With no `flock` on `PATH`, the sweep warns and archives (prints `SKIP G` if the shim cannot be built) |
| H | `cleanup_on_failure` on an unmerged branch archives it and pushes nothing |
| I | A ref-name conflict makes the archive fail; the sweep keeps the worktree, and `cleanup_on_failure` refuses to prune |
| J | After the branch is rewritten, the archive keeps the old commit, the worktree is kept and `cleanup_on_failure` refuses to prune |
| K | With a `flock` that fails for a reason other than contention, the sweep warns, archives and does not log a skip |
| Symlink | With the lock path a symlink, the sweep warns, runs unlocked and leaves the target unchanged |

CI runs it after the #1494/#1560 sweep test, and both the script and the test
are in the `shellcheck -S warning` lists in `.github/workflows/ci.yml`.

## Related documentation

- [Leak-Proof, Self-Healing Worktree Setup (#840)](issue-840-leak-proof-worktree.md): the sweep and `cleanup_on_failure` algorithms
- [Caller-branch worktree alias (#1762)](issue-1762-caller-branch-worktree-alias.md#cleaning-up-alias-worktrees): how the sweep treats alias worktrees
- [Re-runs Never Destroy an Existing Worktree's Work (#1565)](issue-1565-rerun-preserves-worktree.md)
