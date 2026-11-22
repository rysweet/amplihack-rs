# Config Write Durability

> [Home](../index.md) > Concepts > Config Write Durability

**Type**: Explanation (Understanding-Oriented)

This page explains why one write to `~/.amplihack/config.toml` skips the
`fsync` that every other amplihack config write performs. It is the design rationale for
GitHub issue #1992. The same decision is recorded in the module comment of
`user_config::edit` in `crates/amplihack-cli/src/user_config/edit.rs`.

## Contents

- [The write in question](#the-write-in-question)
- [What the sync cost](#what-the-sync-cost)
- [The decision](#the-decision)
- [Why the consent can go unsynced](#why-the-consent-can-go-unsynced)
- [What a crash can do](#what-a-crash-can-do)
- [Writes that still sync](#writes-that-still-sync)
- [How the code enforces the split](#how-the-code-enforces-the-split)
- [Out of scope](#out-of-scope)
- [Related](#related)

## The write in question

On the first launch, the launcher asks whether it may set
`NODE_OPTIONS=--max-old-space-size=<mb>` for the agent CLIs it starts. A
non-interactive launch consents without asking. Either way the answer is saved
as `node.heap_consent` in `~/.amplihack/config.toml` before claude starts, and
later launches report it:

```text
ℹ NODE_OPTIONS=--max-old-space-size=32768 (saved preference). To change: amplihack config set node.heap_consent <true|false>
```

The save goes through `memory_config::save_user_preference`, then
`user_config::save_heap_consent_nosync`, then the atomic writer in `user_config::edit`.

## What the sync cost

The atomic writer called `sync_all()` on the temporary file before renaming it
over the target. `sync_all` waits until the file's data and metadata reach
stable storage, so its cost depends on everything else the disk is flushing at
that moment, not on the size of `config.toml`.

One `sync_all` of `~/.amplihack/config.toml` took **9.76 s**. It was measured
with strace on the first launch with a fresh HOME, on a build host under write
load, on `main` at 606f3c746. Claude did not start until it returned.

## The decision

The launch-path consent save does not sync. It writes the same way as before
(a temporary file in the same directory, mode 0600, then a rename) but skips
`sync_all`. Every other write through the atomic writer still syncs.

Deferring the write until after claude has started was the alternative. It
was rejected because it would spread consent handling across the launch code
for a value that does not need the guarantee.

## Why the consent can go unsynced

`node.heap_consent` is a preference that can be recovered:

- If the value is lost, the next interactive launch asks again.
- A non-interactive launch consents automatically, which is the same outcome
  as for a fresh HOME.
- The value controls a Node heap limit. It holds no secret and grants no
  access.

A stall of several seconds before every first launch on a busy host costs more
than the durability of that answer is worth.

## What a crash can do

The no-sync write rewrites the whole user file, not only `node.heap_consent`.
A power loss or kernel crash between the rename and the kernel's own writeback
can therefore affect every key in `~/.amplihack/config.toml`:

| State after the crash | Effect |
|-----------------------|--------|
| The old file is still there | The consent was not saved. The next launch asks again. |
| The file is missing or empty | Every key falls back to its default. The next launch asks again, or consents if non-interactive. |
| The file is unreadable, for example filled with NUL bytes | The launcher finds no saved consent, because a file that does not parse yields no values and the parse error is not shown on this path. It asks again, or consents if non-interactive, on every launch until the file is repaired. Each consent save fails without overwriting anything, and the failure appears only as a `tracing::warn` from `save_user_preference`. `amplihack config edit` opens the file and reports any problem left in it after the editor exits. |

Some filesystems, such as ext4 with `auto_da_alloc` (the default) and btrfs,
flush the new data before a rename over an existing file, which makes a
zero-length result unlikely there. Other filesystems give no such guarantee.
On the first launch the file usually does not exist yet, so there are no other
keys to lose.

Errors that only appear at `fsync` time, such as `EIO`, `ENOSPC`, `EDQUOT` or
a failed NFS writeback, are not reported on this path. Errors from writing the
temporary file and from the rename still are.

The effects stay within `~/.amplihack/config.toml`. The consent may be asked
for again, other keys in the file may revert to their defaults, and an
unreadable file makes the launcher ask on every launch until it is repaired.

A lost `false` is the one case with a visible effect: a later non-interactive
launch consents and raises the Node heap limit. That matches the behaviour for
a fresh HOME.

## Writes that still sync

These writes keep `sync_all`. `config set` and `config unset` are explicit
user commands, and the user expects them to survive a crash. The migration is
left unchanged (see [Out of scope](#out-of-scope)).

| Write | Path |
|-------|------|
| `amplihack config set` | `user_config::edit::set_value`, which calls `set_value_with(.., true)` |
| `amplihack config unset` | `user_config::edit::unset_value`, which calls `write_atomic` |
| Migration from `~/.amplihack/config` | `set_value` for `node.heap_consent`, and `write_atomic` for the rewrite of the legacy file, both called from `user_config::migrate` |

`amplihack config edit` is not in the table. It runs `$VISUAL`, `$EDITOR` or
`vi` on the file directly, so the editor decides how the file is written and
whether it is synced.

Running `amplihack config set node.heap_consent true` after a launch makes the
saved answer durable:

```sh
amplihack config set node.heap_consent true
amplihack config get node.heap_consent --show-origin
# true  # user (/home/alice/.amplihack/config.toml)
```

## How the code enforces the split

- `write_atomic_with(path, bytes, sync)` is the private writer. Both modes
  resolve a symlinked target, create the directory with mode 0700, write a
  random-name temporary file set to 0600 before any byte, and rename it over
  the target. The only difference is whether `sync_all` runs.
- `write_atomic` keeps its signature and calls `write_atomic_with(.., true)`.
  `unset_value` and the legacy-file rewrite in `user_config::migrate` call
  `write_atomic`.
- `set_value_with(path, key, value, sync)` is the private TOML edit. It passes
  `sync` through to `write_atomic_with`.
- `set_value` keeps its signature and calls `set_value_with(.., true)`.
- `set_value_nosync` is `pub(crate)` and calls `set_value_with(.., false)`.
  Its doc comment says it is only for recoverable launch-path preferences.
- `user_config::save_heap_consent_nosync(consent)` is the only caller of
  `set_value_nosync`, and `save_user_preference` is its only caller. It takes
  no key: it always writes `node.heap_consent`, so no other key can reach the
  unsynced write in any build.
- A `#[cfg(test)]` thread-local counter records each successful `sync_all`.
  Tests assert that `set_value` raises it by one and that `set_value_nosync`
  and `save_user_preference` leave it unchanged. Release builds contain no
  counter.

## Out of scope

- Other syncs on the launch path: the legacy migration in
  `user_config::migrate`.
- Syncing the parent directory after the rename.
- The consent prompt itself, its timeout and the non-interactive auto-yes.

`.claude/runtime/launcher_context.json` is written atomically, mode 0600, but
not synced, because every launch rewrites it (#2068). After a crash it may be
unchanged, missing, empty or unparsable. Readers treat the last three as absent
and apply the 24-hour staleness check to an unchanged one, as described in
[agent binary resolution](./agent-binary-routing.md#resolution-algorithm).

## Related

- [User Configuration](../reference/user-config.md#how-files-are-written) - How every config file is written, and which writes sync
- [Memory Configuration Consent](../features/memory-consent-prompt.md) - The prompt whose answer `node.heap_consent` stores
