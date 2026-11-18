---
title: Private-state creation API
description: Unix private-state creation contracts and recovery dependency limits.
doc_type: reference
last_updated: 2026-11-18
---

# Private-state creation API

Private-state creation APIs finalize new directories to Unix mode `0700` and
new data files to `0600`, even when umask removes owner permissions. They
leave the process umask unchanged. Existing objects retain their permissions.
The legacy `ensure_private_directory` helper retains its explicit contract
of tightening an existing leaf; it is not used for recovery admission.

These APIs create storage; callers remain responsible for admitting existing
paths. Recovery uses the runner's authoritative ownership, symlink and
permission checks described in [work-root permissions](recipe-command.md#work-root-permissions).

## Shared file helpers

The helpers live in `amplihack_utils::secure_files`.

| API | Current behavior |
| --- | --- |
| `create_private_directories(&Path)` | Creates missing components relative to directory handles without following symlinks, and finalizes each new directory to `0700`. Existing directories and their permissions remain unchanged. |
| `open_private_write(&Path)` | Returns a writable file, finalizing a newly created descriptor to `0600`. Truncates an existing file while preserving its permissions. Parents must exist. The final component is opened without following a symlink on Unix. |
| `write_private_file(path, bytes)` | Writes all bytes through `open_private_write`; has the same parent, creation and existing-file contracts. |
| `open_private_append(&Path)` | Creates missing parents at `0700` and finalizes a new append file to `0600`; existing file permissions remain unchanged. |

File opens first attempt exclusive creation; only that successful descriptor
is chmodded. An existing-file fallback does not create or chmod anything and
never follows a final symlink. `create_private_file` always creates exclusively.
`PrivateTemp` finalizes its descriptor before returning, before serialization
or publication can observe the temporary file.

Directories are traversed with `openat(O_DIRECTORY | O_NOFOLLOW)` and created
with `mkdirat` relative to the parent handle. A descriptor's actual mode is
checked before return. On Linux, directories already at the required owned
mode need no chmod; readable directory handles use ordinary `fchmod`.
Initially inaccessible directories use `O_PATH` and `fchmodat2(AT_EMPTY_PATH)`.
When that syscall or flag is unavailable, a verified procfs directory handle
addresses the live numeric descriptor through its kernel-owned magic link.
This supports older Linux kernels without chmodding a user pathname. Actual
permission errors are propagated. If no secure mechanism is available, the
operation fails. On other Unix systems ordinary directory handles and `fchmod`
are used; a mask that prevents opening the new directory produces an error. Existing directories are traversed without changing their
modes. Callers remain responsible for admitting existing parents and using a
trusted hierarchy; recovery's ancestor checks remain authoritative.

An already-existing entry never authorizes changing its permissions. These
creation APIs do not normalize existing roots, parent directories or files.
Legacy helpers with explicit permission-repair contracts retain those
separate contracts; they are not used to admit recovery roots or ancestors.
Filesystem errors propagate to callers, which attach
the affected path to CLI errors.

## Private state constructors

The types below are exported by `amplihack_state`. Each `new_private`
constructor selects the permission policy; filesystem operations happen when
the object is used. `exclusive_private` opens and locks the file immediately.

| API | Creation during use |
| --- | --- |
| `AtomicJsonFile::new_private(path)` | Finalizes `0700` for missing parents and `0600` for the publication temporary file, resulting document and lock. |
| `AtomicCounter::new_private(path)` | Uses a private `AtomicJsonFile` for the counter, with the same parent, document, temporary-file and lock modes. |
| `AtomicFlag::new_private(path)` | Finalizes `0700` for missing parents and `0600` for an exclusively created flag. `set()` returns `false` when the flag already exists, without changing it. |
| `FileLock::exclusive_private(&Path, Duration)` | Finalizes `0600` for a missing lock file and acquires an exclusive lock. Parents must already exist. Existing lock permissions remain unchanged. |

Private JSON publication retains the lock-before-update sequence,
serialization, buffer flushing, synchronization, atomic publication and
temporary-file cleanup. Temporary files and locks have their new descriptors
finalized before serialization, locking or publication. Existing parent
directories keep their modes. Updating a document may replace it with a newly
published private file; this is distinct from chmodding an existing object.

Generic constructors such as `AtomicJsonFile::new`, `AtomicCounter::new`,
`AtomicFlag::new` and `FileLock::exclusive` retain their existing permission
contracts. Ordinary Git checkout contents and files written by recipe steps
also retain their own permission policies.

## Runner-created recovery storage

The pinned runner creates recovery objects through its own private-filesystem
API. It requests private modes but does not restore owner permissions removed
by restrictive masks. The exact-mode guarantees for amplihack's
helpers do not extend to those dependency-created objects without a
runner-source change. The runner pin and admission policy are unchanged.
Umask `0002` preserves owner permissions and produces the requested modes;
masks that remove owner access
can still prevent recovery admission.

Diagnostics preserve the runner error and its causes. Read-only inspection
can explain a failure but cannot authorize access or change permissions on
existing storage. See [recovery storage](recipe-recovery.md) and
[work-root configuration](../howto/clean-up-after-recipe-runs.md#settings).
