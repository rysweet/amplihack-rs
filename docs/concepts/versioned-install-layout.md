---
title: Versioned Install Layout
description: How atomic release switching preserves running processes and trusted auto-drive bundles.
last_updated: 2026-11-11
review_schedule: quarterly
owner: amplihack-maintainers
doc_type: explanation
---

# Versioned Install Layout

> [Home](../index.md) > Concepts > Versioned Install Layout

`amplihack install`, `amplihack update` and the startup self-heal never change
files that a running recipe run may still execute or read. Each install writes
a new release directory, then switches the symlinks it planned (up to four)
to it. Old releases are
deleted only when no process uses them. The installer never sends a signal to
a process it did not start.

This page explains the layout and why it works this way (issue
[#1954](https://github.com/rysweet/amplihack-rs/issues/1954)).

## Contents

- [Why releases are versioned](#why-releases-are-versioned)
- [Layout on disk](#layout-on-disk)
- [How a switch works](#how-a-switch-works)
- [What a running run sees](#what-a-running-run-sees)
- [Cleanup of old releases](#cleanup-of-old-releases)
- [Mapped directories under ~/.amplihack/.claude](#mapped-directories-under-amplihackclaude)
- [The install lock](#the-install-lock)
- [No signals](#no-signals)
- [Self-heal and update](#self-heal-and-update)
- [The first update to this release](#the-first-update-to-this-release)
- [One-time migration from the old layout](#one-time-migration-from-the-old-layout)
- [Uninstall](#uninstall)
- [Security rules](#security-rules)
- [Known limits](#known-limits)
- [Inspecting an install](#inspecting-an-install)
- [Tests](#tests)
- [See also](#see-also)

## Why releases are versioned

Before this layout, an install deleted and replaced files in place:

- `stage_and_swap_bundle` and `copy_mapped_dir` (in
  `commands/install/directories.rs`) renamed the live tree to `*.old`, renamed
  the new tree into place and then ran `remove_dir_all` on `*.old`. Between
  the two renames the path did not exist. After the delete, a run whose
  working directory or open files were in the old tree lost them.
- The startup self-heal could re-stage the bundle while an explicit install
  was doing the same, because the explicit install did not take the install
  lock.

On 5 Nov at 22:00:27Z a binary and bundle swap of this kind ended two
long-running recipe drives within seconds. The reproduction in
`tests/issue_1954_install_leaves_live_runs_alone.rs` shows the cause on the
code before this layout:

- **What broke the run was a file change, not a signal from another
  process.** The install renamed `amplifier-bundle` to `amplifier-bundle.old`
  and deleted it. The run's working directory went with it, and the next
  read failed:

  ```
  ls: cannot access './recipes': No such file or directory
  cwd: $HOME/.amplihack/amplifier-bundle.old (deleted)
  ```

- **No other process sent a signal.** A sentinel process with `TERM`, `HUP`
  and `INT` traps, started beside the run, recorded no signal across the
  install.

Where the `SIGTERM` seen in the 5 Nov drive logs came from has not been
reproduced. It may have come from the run's own teardown after a step failed,
but this is not proven, and the fix does not depend on it.

The versioned layout removes the cause: published files are never changed or
deleted while in use.

## Layout on disk

```
~/.local/bin/
├── amplihack                 -> ~/.amplihack/releases/<release>/bin/amplihack
├── amplihack-hooks           -> ~/.amplihack/releases/<release>/bin/amplihack-hooks
└── amplihack-asset-resolver  -> ~/.amplihack/releases/<release>/bin/amplihack-asset-resolver

~/.amplihack/
├── amplifier-bundle          -> ~/.amplihack/releases/<release>/amplifier-bundle
├── .install.lock
├── .installed-version
├── .claude/                  (mapped framework assets; see below)
└── releases/                 (mode 0700, owned by you)
    ├── 1.2.107/
    │   ├── .complete
    │   ├── .installed-by
    │   ├── bin/
    │   │   ├── amplihack
    │   │   ├── amplihack-hooks
    │   │   └── amplihack-asset-resolver
    │   └── amplifier-bundle/
    ├── 1.2.108/
    ├── 1.2.108-20261105T220027Z-48113/
    └── .retired/             (mode 0700; old trees waiting for cleanup)
        └── 20261105T220027Z-48113-skills/
```

Naming rules:

| Case | Release directory |
|------|-------------------|
| First install of a version | `releases/<version>/` |
| Same version again (`--force-refresh`, reinstall, self-heal) | `releases/<version>-<UTC yyyymmddTHHMMSSZ>-<pid>/` |

Old trees that are replaced but may still be in use (mapped directories, the
old real-directory bundle found at migration) are not releases. They are moved
to `releases/.retired/<ts>-<pid>-<label>/`.

- An existing release directory is never reused or written to.
- `<version>` must match `^[0-9A-Za-z][0-9A-Za-z._+-]{0,63}$`. Names such as
  `.`, `..` and `.retired` are rejected. An invalid version stops
  the install with an error. It is never silently rewritten.
- `.installed-by` is written just before `.complete`. It records which
  process committed the release; see
  [The `.installed-by` record](#the-installed-by-record).
- `.complete` is written last, before any link switches. A release without
  `.complete` is never a link target.
- After `.complete` is written, the release root is mode `0755` and no file in
  the release is writable by group or others. Nothing writes into a release
  after its links switch. Runtime state goes to `~/.amplihack/.claude/`
  (#1929).

Hook commands in `~/.claude/settings.json` keep the stable path
`~/.local/bin/amplihack-hooks`, so settings do not change on each install.

## How a switch works

The install builds the whole release first, then switches the links:

1. Copy the binaries into `releases/<release>/bin/`.
2. Stage the framework bundle into `releases/<release>/amplifier-bundle.staging/`
   and run every check from
   [framework source selection](../reference/framework-source-selection.md):
   version match, manifest check, mode normalization, and compatibility. Then
   rename the staging directory to `amplifier-bundle/`.
3. Write `.installed-by`, then `.complete`, and fix modes. If writing
   `.installed-by` fails, the install stops and the unpublished release is
   removed.
4. Switch the links this install planned, in this order: `amplihack-hooks`,
   `amplihack-asset-resolver`, `amplihack`, `amplifier-bundle`.

An install plans a link only when it has the file to put behind it. A full
install plans all four when an `amplihack-asset-resolver` binary is available,
and three otherwise. `amplihack update` plans only `amplihack` and
`amplihack-hooks` (see [Self-heal and update](#self-heal-and-update)). A link
that is not planned is left alone and keeps pointing at its earlier release, so
the links can point into different releases. Each one still points into a
release with `.complete`.

Each link switch is atomic. The installer creates a temporary symlink
`.<name>.tmp-<pid>-<n>` next to the target and calls `rename(2)` over the
target. At every moment the path names either the old release or the new one.
It never points at a missing file or a half-written tree.

When the target is still a real directory from the old layout, Linux swaps it
with `renameat2(RENAME_EXCHANGE)`. See
[one-time migration](#one-time-migration-from-the-old-layout).

If the install fails before step 4, no link changes. The unpublished release
directory is removed, and the previous release stays in use.

The [mapped directories under `~/.amplihack/.claude`](#mapped-directories-under-amplihackclaude)
are not part of a release. They are exchanged after step 4, so a failure while
the release is staged or checked leaves them on the old version together with
the links. If exchanging one of them fails, the links and bundle are already on
the new release and the trees exchanged before it stay new; there is no
rollback, and running `amplihack install` again exchanges the rest.

### A failure partway through switching

Switching returns a report of which links switched. It does not roll back. If
one link switch fails after an earlier one succeeded, install stops with an
error naming the link that failed, and the links are mixed: some point into the
new release and some into the old one. Every link still names a complete
release, and both releases are kept by the per-link rule in
[cleanup](#cleanup-of-old-releases). The next successful install switches the
remaining links.

## What a running run sees

| A running process has... | After the switch |
|--------------------------|------------------|
| An `amplihack` binary already executing | Keeps running the old inode. The old release is not deleted. |
| Its working directory inside the bundle | The directory still exists, unchanged, in the old release. |
| Open files in the old release | Still valid; the files are not changed or deleted. |
| A shell that re-reads `~/.amplihack/amplifier-bundle/...` by path | Reads the new release's files. Each file is complete, never half-written. |
| A new `amplihack` or `amplihack-hooks` exec | Starts the new release's binary. |
| An auto-drive invocation | Uses its private launch copy of trusted recipes, criteria, templates, evaluator, and helpers. |

### One trusted bundle per auto-drive invocation

The runner binds auto-drive's trusted recipes, criteria, templates, evaluator, and
helpers at launch. A release switch cannot redirect later review or
finalization to new instructions or PR-authored tools. Selection checks
ownership and modes before starting work. A release-backed bundle resolves
to its physical path beneath the configured `releases/` root and requires
`.complete`; the bundle, release, and releases root must have trusted
ownership and no group/other write access. A later public-link switch cannot
change that selection.

This responsibility is independent of merge enforcement. There is no merge shim on `PATH`, gate-specific `BASH_ENV`, or process-identity merge guard.
Runner-owned authorization records and evidence snapshots preserve workflow
state; the finalization agent observes hosting-service state.

For other consumers, a step that re-reads the public bundle link can still
see a new complete release. See [framework source selection](../reference/framework-source-selection.md#auto-drive-launch-bundle)
and [auto-drive's execution contract](../reference/auto-drive-to-merge.md#trusted-execution-and-portability).

## Cleanup of old releases

Cleanup runs after every successful switch, still under the install lock.

It always keeps, for each link separately:

- the release the link points to now;
- the release the link pointed to before this switch.

Because the rule is per link, mixed links (after an update, an unplanned link
or a failed switch) never lose their target.

Any other entry in `releases/` or `releases/.retired/` is deleted only if no
process uses it. On Linux the installer checks, for every process owned by
your uid, these entries in `/proc/<pid>/`:

- `exe`, `cwd`, `root`
- `fd/*` (link targets only)
- `maps`
- `environ` (only "does any value name a path under the candidate")

The check is read-only. It never uses `ptrace`, never sends a signal and
never logs or stores an environment value.

Processes owned by other uids are skipped. A non-dumpable process has
`/proc` entries its own user cannot read. A process becomes non-dumpable by
calling `prctl(PR_SET_DUMPABLE, 0)`, as `ssh-agent` and `gpg-agent` do, or by
running a set-user-ID, set-group-ID or file-capability program, such as the
systemd user manager (`systemd --user`), its `(sd-pam)` helper, or `ping` on
distributions that give it `cap_net_raw`. For such a process the installer
first reads `/proc/<pid>/cgroup`, which stays readable. If it is exactly
`0::/user.slice/user-<uid>.slice/user@<uid>.service/init.scope`, the process
is the user manager or `(sd-pam)`, which run from `/` and system binaries,
and it keeps no candidate (see [Known limits](#known-limits)). Otherwise the
installer reads `/proc/<pid>/stat`, which also stays readable, and takes its
start (fork) time from it:

- Every candidate that existed when it started is kept (with 2 seconds of
  slack for the kernel's whole-second boot time). The process may have
  inherited its working directory, root, open files or environment there from
  its parent. This holds for privileged processes too: a privileged
  executable is never a release file, but a recipe step that runs `ping` from
  inside a release gives `ping` a working directory in that release.
- A candidate created after it started can be removed. The process did not
  inherit a path into it, and it has not run a release binary since, because
  that `exec` would have made it dumpable again.

If the start time cannot be read, the result is unknown and every candidate
is kept. A candidate's creation time is its birth time (`statx`
`STATX_BTIME`) only. On a filesystem that does not record one, such as NFS
or some FUSE and ecryptfs mounts, the creation time is unknown and the
candidate is kept for every non-dumpable process. The modification time is
never used instead: a write into a release after it was committed moves it
later, which would make an old release look newer than a process inside it. Install then prints:

```
ℹ Old amplihack releases kept: a running process of this user could not be inspected, so no release was removed (~/.amplihack/releases)
```

On non-Linux hosts there is no process scan, so every old release is kept,
and install and `uninstall` print:

```
ℹ Old amplihack releases kept: this platform cannot tell which releases running processes use, so no release was removed (~/.amplihack/releases)
```

Anything kept is checked again on the next install. When more than 5 releases
are kept because they are in use, install prints this warning:

```
⚠️  7 amplihack releases kept because running processes use them (~/.amplihack/releases); they are removed by a later install once unused
```

A candidate is deleted only if it is a direct child of `releases/` or
`releases/.retired/`, is a real directory owned by you, and is still not a
symlink when checked again just before deletion. Read-only modes are cleared
only on files you own, without following symlinks or crossing devices, and
only right before deletion.

Expect about 2–3 times the disk use of a single install while old runs are
alive.

### Decision for #2024: non-dumpable processes

In the first design of this cleanup (#1954), one non-dumpable process of the
same user, such as `ssh-agent`, made the scan unknown and kept every old
release for as long as it ran, so each install added about 640 MB. Issue
#2024 offered two options: (a) keep a release only for processes that could
have inherited a path into it, judged by comparing times, or (b) keep the old
behaviour and report the growth. Option (a), as refined and merged in #2137,
is the rule adopted:

- A non-dumpable process keeps a release only if the release's birth time is
  at or before the process start time plus 2 seconds.
- The rule uses the birth time (`statx` `STATX_BTIME`), not the modification
  time of `.complete` that the issue proposed. A later write into a release
  moves its modification time forward, which could let a release be removed
  while a process is still inside it. When the birth time cannot be read,
  the release is kept.
- Every release that existed when the process started is kept. This covers
  the working directory, open files and environment values the process may
  have inherited from its parent, which was the concern raised in the #1954
  security review.
- The systemd user manager and its `(sd-pam)` helper are exempt, matched by
  their exact cgroup v2 path. Hosts where that path does not match are
  described in [Known limits](#known-limits).
- For a non-dumpable process, the scan result is unknown, and every release
  is kept, only when its start time cannot be read.
- The remaining risk is a backward step of the system clock, described in
  [Known limits](#known-limits).

Option (b) on its own was not adopted. A size notice makes the growth
visible but does not stop it. The warning shown above, printed when more than
5 releases are kept, remains as a backstop.

## Mapped directories under ~/.amplihack/.claude

Install also copies mapped framework directories (skills, agents, context,
recipes, tools and so on) into `~/.amplihack/.claude/`. These stay outside the
release because `~/.claude` publication depends on their paths. They are
exchanged after the links switch to the new release (see
[How a switch works](#how-a-switch-works)), one tree at a time, and follow the
same rule:

- The new tree is built at a staging name, then swapped with the live tree
  using `renameat2(RENAME_EXCHANGE)`, so the path always exists.
- The old tree, now at the staging name, is moved to
  `releases/.retired/<ts>-<pid>-<name>/`. Cleanup deletes it once unused.
- Leftover `.staging` and `.old` directories from an older amplihack are
  retired the same way, not deleted.
- If a move into `.retired/` fails with `EXDEV` (different filesystem), the
  tree stays where it is and install prints a warning.
- The user's own backups are never deleted.

The only directory install deletes directly is its own staging tree that was
never swapped into place.

### Slash-command directories

Install also publishes the slash commands outside `~/.amplihack`, to
`~/.claude/commands/amplihack/` and to the Copilot plugin's `commands/`. Each
new command set is built in a scratch directory that the client does not
scan, then exchanged with the live directory, so the path is never missing.
The previous command set is moved to
`releases/.retired/<ts>-<pid>-commands/`, and cleanup deletes it once unused.
Leftover `.old` command trees from an interrupted install are retired the same
way.

The new commands are already in place at that point, so a failed move only
prints a warning. A tree that cannot be moved (for example `EXDEV`, because
`~/.claude` is on another filesystem than `~/.amplihack`) is removed only when
the process scan completes and shows that no process uses it, the same rule
release cleanup applies. Otherwise it is renamed to
`<name>.unretired-<n>` in the scratch directory, so it does not block the next
install, and each later install checks it again the same way. A leftover
`.staging` directory from an install that stopped after the exchange is
handled the same way, because it holds the command set that was live before.

If `RENAME_EXCHANGE` is not supported (`EINVAL` or `ENOSYS`), install falls
back to two renames and logs that the path was missing for a few
microseconds.

## The install lock

All install work takes an exclusive lock on `~/.amplihack/.install.lock`:

- explicit `amplihack install`;
- `amplihack update` while it downloads and switches the binaries;
- the install child that `amplihack update` starts afterwards;
- the startup self-heal;
- `amplihack uninstall`.

Rules:

- The lock blocks with no timeout. After about 2 seconds of waiting, this
  exact line is printed once:

  ```
  ⏳ waiting for another amplihack install to finish (lock: $HOME/.amplihack/.install.lock)
  ```

- The lock is re-entrant on the thread that holds it, so self-heal calling
  install does not deadlock. Another thread in the same process waits.
- `amplihack update` releases the lock before it starts the post-update
  install child, and the child takes it again.
- Self-heal re-reads the version stamp once it holds the lock. If another
  process already installed the same version, self-heal does nothing (#1526).
- The lock file is opened with `O_NOFOLLOW`, mode `0600` and close-on-exec.
  A symlink at the lock path is refused.

An `amplihack` command started inside a run while an update holds the lock
waits for the update to finish. This is intended.

## No signals

Install, update, self-heal, migration, cleanup and uninstall never signal a
process they did not start. The only allowed `kill` is
`util::run_with_timeout` / `terminate_timed_out_child` on the installer's own
timed-out child. A source-scan test enforces this; see [Tests](#tests).

## Self-heal and update

The startup [self-heal](../features/self-heal-asset-restage.md) re-stages by
calling the same install path, under the same lock. It creates a new release
and switches links like any install.

`amplihack update`:

1. downloads the release and checks its checksum (unchanged);
2. allocates a release from the validated tag;
3. copies `amplihack` and `amplihack-hooks` into it (the release archive has
   no other binary);
4. runs `--version` on the release binary and checks it matches the tag;
5. switches only `amplihack-hooks` and `amplihack`;
   `amplihack-asset-resolver` and `amplifier-bundle` stay on their earlier
   release;
6. releases the lock and starts the new binary's install child. The child
   builds a further release with the binaries and the bundle, and switches
   `amplihack-hooks`, `amplihack` and `amplifier-bundle`, plus
   `amplihack-asset-resolver` only if it has a resolver binary to deploy. See
   [post-update re-exec](../features/update-reexec-new-binary.md).

If the child fails, the binaries point into the update's release and the
bundle into the earlier one. Both are kept, and the next install brings them
together.

When the running binary is a release path, update targets the link that led
to it (`~/.local/bin/amplihack`, or the `PATH` entry that resolves to that
release). The free-space check is made on `releases/`, for the full size of
both binaries. A binary installed under a system prefix that is not a link is
updated as before.

## The first update to this release

The first `amplihack update` from an installed 1.2.107 to this release
**does not need a window with no recipe runs.** The newly downloaded binary
does all the re-staging, not the old installed one.

What each binary does during that update:

| Step | Binary | Effect on live runs |
|------|--------|---------------------|
| Download, checksum, replace `~/.local/bin/amplihack` and `amplihack-hooks` | Old (1.2.107) | Copy to a temporary name, then `rename(2)` over the file. Running processes keep the old inode. Safe. |
| Start `amplihack install --force-refresh` with `AMPLIHACK_POST_UPDATE_INSTALL=1` | Old | Runs the path it just installed, as a child process. |
| Build `releases/<version>/`, switch links, migrate the old real-directory bundle, clean up | New | Follows this page. Nothing is rewritten or deleted in place. |

The hand-off in 1.2.107 is `build_install_command_with_path` in
`crates/amplihack-cli/src/update/check.rs`:

```rust
let mut cmd = std::process::Command::new(installed_exe);
cmd.arg("install")
    .arg("--force-refresh")
    .env("AMPLIHACK_NO_UPDATE_CHECK", "1")
    .env("AMPLIHACK_NONINTERACTIVE", "1")
    .env("AMPLIHACK_POST_UPDATE_INSTALL", "1")
    .env("AMPLIHACK_REPAIR_ORIGINAL_PATH", &parent_path)
    .stdin(std::process::Stdio::inherit())
    .stdout(std::process::Stdio::inherit())
    .stderr(std::process::Stdio::inherit());
if let Some(path) = repair_subprocess_path(installed_exe, &parent_path) {
    cmd.env("PATH", path);
}
```

`tests/issue_1954_first_install_restager.rs` shows it. A stub "old" binary
does only what 1.2.107 does: it renames the new binary over
`~/.local/bin/amplihack` and runs it with argv exactly
`install --force-refresh` and `AMPLIHACK_NO_UPDATE_CHECK=1`,
`AMPLIHACK_NONINTERACTIVE=1` and `AMPLIHACK_POST_UPDATE_INSTALL=1` set. The
test then reads `.installed-by` in the new release and checks that the pid
is the stub's child (the new binary's process) and not the stub's own, the
version is the new one, and `post_update=1`. `exe` must be non-empty and be
one of: `$HOME/.local/bin/amplihack` as written or canonicalized, the built
binary, or the new release's `bin/amplihack`. The test accepts all of these
because `current_exe` and `canonicalize` can resolve to different paths. On
the first update the path is that of the new binary: the regular file
1.2.107 renamed into place.

The stub lists `~/.amplihack` just before it starts the new binary, and the
test checks that this listing equals the one it took before starting the
stub, so the old binary's part of the hand-off wrote nothing there. The test
also checks that a run parked in the old real-directory bundle finishes
normally.

An old 1.2.107 process still running after the update runs 1.2.107's own
self-heal code, not this release's. That code already has the #1526
downgrade guard (in `self_heal.rs` in 1.2.106 and 1.2.107): when the installed
version stamp is newer than the running binary, it does nothing. So an old
process does not re-stage over the new release. This release keeps the same
guard in its own self-heal, pinned by
`self_heal_from_older_binary_leaves_newer_release_alone` in
`commands/install/tests/issue_1954_release_layout.rs`. That test covers this
release's code only; the guard in 1.2.107 is not tested here.

**Earlier releases.** The same hand-off to the new binary has been in
`amplihack update` since 0.9.69 (#690, which closed #683), and
`AMPLIHACK_POST_UPDATE_INSTALL=1` since 0.9.71. Releases from 0.9.69 to
1.2.106 should behave as 1.2.107 does, but only the 1.2.107 hand-off is
tested. From 0.9.69 or 0.9.70 the new binary still does the work, but
`.installed-by` records `post_update=0`.

**Installs older than 0.9.69** run the install in the old process with the
old code (`run_install` in-process), which re-stages the bundle in place.
Before updating such an install, stop all recipe runs, or install this
release by hand into an idle host.

### The `.installed-by` record

Each committed release holds `.installed-by`, a text file of `key=value`
lines, mode `0444`:

```
format=1
version=<version>
exe=$HOME/.local/bin/amplihack
pid=<pid>
post_update=1
```

| Key | Value |
|-----|-------|
| `format` | Always `1`. |
| `version` | The version of the binary that committed the release. |
| `exe` | Canonical path of that binary (`/proc/self/exe`). After the first update from 1.2.107 this is `$HOME/.local/bin/amplihack`, the regular file the old binary renamed into place. Empty if it cannot be resolved, is not UTF-8, or contains a newline, carriage return or NUL. |
| `pid` | Process id of that binary. |
| `post_update` | `1` when the install ran with `AMPLIHACK_POST_UPDATE_INSTALL=1` (started by `amplihack update`), else `0`. |

The file is written to `.installed-by.tmp` with `O_EXCL` and renamed into
place. A leftover `.installed-by.tmp` is removed only if it is a regular file
or symlink; a symlink there is never followed.

The record is evidence for debugging and tests only. No production code reads
it, and nothing is gated on it, signals the recorded pid or runs the recorded
path. `release_layout::read_installed_by(release_dir)` returns
`Option<InstalledBy>`; it opens the file with `O_NOFOLLOW`, accepts only a
regular file, reads at most 4 KiB, ignores unknown keys, and returns `None`
for a missing version or an unparsable pid.

To see which binary built the current release:

```bash
cat "$(readlink -f ~/.amplihack/amplifier-bundle)/../.installed-by"
```

## One-time migration from the old layout

The first install with this layout finds real files and directories where
links now go:

| Found | What install does |
|-------|-------------------|
| Regular file `~/.local/bin/amplihack` (or `-hooks`, `-asset-resolver`) | Renames the new link over it. Running processes keep the old inode. |
| Real directory `~/.amplihack/amplifier-bundle/` | Swaps it with the new link using `RENAME_EXCHANGE`, then moves the old tree to `releases/.retired/<ts>-<pid>-amplifier-bundle/`. Cleanup deletes it once unused. |
| Old directory on another filesystem (`EXDEV`) | Leaves it at its temporary name, `~/.amplihack/.amplifier-bundle.tmp-<pid>-<n>`, prints a warning and continues. |
| Old directory that is a symlink or owned by another user | Refuses with an error naming the path. |

Runs already alive keep their working directory and open files. A run that
resolves `~/.amplihack/amplifier-bundle` by path after the migration sees the
new release.

## Uninstall

`amplihack uninstall` takes the install lock, then:

1. Removes each link in `~/.local/bin/` and `~/.amplihack/amplifier-bundle`
   only if it is a symlink. A regular file or directory at a link path is
   retired, not deleted.
2. Runs cleanup with no releases kept by default, including `.retired/`.
3. Lists the releases kept because running processes use them, in this
   format:

```
  🗑️  Removed amplifier-bundle at $HOME/.amplihack/amplifier-bundle
  🗑️  Removed binary $HOME/.local/bin/amplihack
  🗑️  Removed $HOME/.amplihack/releases/1.2.108
ℹ Kept 1 release in use by running processes:
    ~/.amplihack/releases/1.2.107 (pid 48113)
  Run `amplihack uninstall` again after those processes exit to remove it.
```

A binary path recorded in the install manifest that is a regular file (a
pre-#1954 install) is unlinked as before; a process still running it keeps
its inode. Once nothing is left in them, `releases/.retired/` and
`releases/` are removed too.

## Security rules

- Every decision about a path under `releases/` goes through
  `release_layout::is_release_path`. It canonicalises the path, compares by
  component (never by matching the string `/releases/`), requires at least
  one component below `releases/`, and treats a canonicalise failure as "not a
  release".
- Paths read back from the install manifest or from `read_link` must pass
  `is_release_path` before any removal, retirement or exchange.
- `releases/` and `.retired/` must be owned by you and not writable by group
  or others. If not, install stops with an error naming the directory and
  does not repair it.
- A source checkout's `amplifier-bundle` link is followed only when the
  `releases/` beside it is a real directory that meets the same rule, so a
  `releases` symlink cannot point staging at another tree.
- The current runner pins its merge guard only from a release with trusted
  ownership and modes. Planned auto-drive bundle selection retains these
  checks independently of merge enforcement.
- A release is allocated only under the install lock. Cleanup removes an
  incomplete release with no age check, which is safe only because no other
  process can be staging one at the same time.
- Deletion uses only `std::fs::remove_dir_all`, which does not follow
  symlinks.
- A temporary link name left over from a crash is removed only if it is a
  symlink.
- Downloaded binaries enter a release only after the checksum check passes.
- All `unsafe` code is in `release_layout.rs`, Linux only, with `SAFETY`
  comments.

### Link-aware checks

These checks accept a link into a release and still reject anything else:

| Check | Rule |
|-------|------|
| `bundle_compat` | A top-level `amplifier-bundle` symlink is accepted if its target is a real directory inside `releases/` with `.complete`. Validation runs on the target. Symlinks inside the tree are still rejected. |
| `find_source_root` | A symlinked source bundle is accepted only if it resolves to a complete release. |
| `clean_broken_symlinks`, `stale_wrappers` | Skip links whose target is a release path. |
| `path_verify`, `path_conflicts` | Compare canonical targets, so a link and its release count as one copy. |

## Known limits

- A non-dumpable process of your user, such as `ssh-agent` or a setgid or
  file-capability program, keeps every release that existed when it started
  until it exits. A long-lived one started before an install (an
  `ssh-agent` from login) therefore keeps the releases on disk at that
  moment until it exits. Remove such releases by hand once it has, using the
  check in [Inspecting an install](#inspecting-an-install).
- The systemd user manager (`systemd --user`) and its `(sd-pam)` helper are
  the exception. They are recognised by `/proc/<pid>/cgroup`, which stays
  readable, being exactly
  `0::/user.slice/user-<uid>.slice/user@<uid>.service/init.scope`, and they
  keep no release: they run from `/` and system binaries. The units they start
  live elsewhere in the cgroup tree and are inspected as usual. A container's
  `systemd` nested below the manager, or any process on a cgroup v1 or hybrid
  host, does not match and falls under the rule above. On such a host the
  manager still keeps the releases present when it started (at boot with
  lingering on, at first login without), about 640 MB each.
- The start-time rule compares two wall-clock readings: the process start
  (`btime` in `/proc/stat`, which the kernel recomputes from the current wall
  clock, plus the ticks since boot) and the release's birth time. If the wall
  clock steps backward by more than about 2 s plus the gap between creating a
  release and starting a non-dumpable process (an NTP step, or a VM or WSL
  clock resync that overshoots), that process reads as older than the
  release, and the release can be removed while the process still holds a
  path into it. A forward step only keeps more. The reasons for using this
  rule are recorded in
  [Decision for #2024: non-dumpable processes](#decision-for-2024-non-dumpable-processes).
- Auto-drive binds its full trusted execution bundle per invocation.
  Other consumers that read `$AMPLIHACK_HOME/amplifier-bundle/<file>` directly
  after a switch can see a different complete file.
- Mapped directories under `~/.amplihack/.claude/` are swapped, not
  versioned. A run reading them by path after a swap sees the new tree.
- On non-Linux hosts, cleanup is skipped and old releases accumulate until
  removed by hand. Install and `uninstall` print a notice when this happens.

## Inspecting an install

```bash
readlink ~/.local/bin/amplihack ~/.amplihack/amplifier-bundle
# $HOME/.amplihack/releases/1.2.108/bin/amplihack
# $HOME/.amplihack/releases/1.2.108/amplifier-bundle

ls ~/.amplihack/releases ~/.amplihack/releases/.retired
# $HOME/.amplihack/releases:
# 1.2.107  1.2.108
#
# $HOME/.amplihack/releases/.retired:
```

To find which process keeps an old release (Linux):

```bash
for p in /proc/[0-9]*; do
  ls -l "$p/cwd" "$p/exe" 2>/dev/null | grep -q releases/1.2.107 && echo "${p#/proc/}"
done
```

Do not delete release directories by hand while runs are alive. Running
`amplihack install` again removes those that are no longer used, except those
that already existed when a non-dumpable process of your user started (see
[Cleanup of old releases](#cleanup-of-old-releases)). To list processes whose
`/proc` entries cannot be read:

```bash
for p in /proc/[0-9]*; do
  [ -O "$p" ] && ! readlink "$p/cwd" >/dev/null 2>&1 && echo "${p#/proc/} $(cat "$p/comm" 2>/dev/null)"
done
```

A release is safe to remove by hand when no link points into it and the
first loop above prints nothing for it. A process listed by the second loop
may still use any release that existed when it started. The systemd user
manager (`systemd`) and its `(sd-pam)` helper are started by the system
manager from `user@.service`, not from a shell or a recipe step, so they
cannot have inherited a path into a release; any other process in that list
must exit first. To remove a
release (here 1.2.107) once those checks pass:

```bash
rm -rf ~/.amplihack/releases/1.2.107
```

## Tests

| Test | Covers |
|------|--------|
| `tests/issue_1954_install_leaves_live_runs_alone.rs` | A bash run with its working directory and an open file in the bundle, and a signal sentinel, stay alive across two `--force-refresh` installs and a self-heal. The run reports ready through a FIFO and blocks on a "go" FIFO that the test writes only after the installs finish; the test then joins it with `wait()`. The run finishes normally, the sentinel records no signal, and each link read before, between and after the installs names its old target or a complete release. Fails on the code before this layout. |
| `tests/issue_1954_first_install_restager.rs` | The first update from 1.2.107: a stub old binary hands off as 1.2.107 does, and `.installed-by` shows the new binary (not the stub) built and switched the release, and a listing of `~/.amplihack` taken by the stub just before it starts the new binary matches the one taken before the stub ran. A run parked in the old real-directory bundle finishes. |
| `tests/issue_1954_no_foreign_signals.rs` | Source scan: no `kill`, `killpg`, `Signal::` or `kill`/`pkill`/`killall` command in install, update, self-heal, lock, layout or uninstall code, except `util.rs` and `Child::kill`. |
| `commands/install/tests/issue_1954_release_layout.rs` | Release naming, atomic switch over a file, link or directory, switching only planned links, a partial switch report with the per-link keep set, exchange fallback, `EXDEV` retire, in-use cleanup (holders signal readiness on stdout and are released by closing stdin), a non-dumpable process keeps only releases created no later than 2 seconds after it started (#2024), the exact message text and the security rules above. Also the `.installed-by` record (fields, sanitised `exe`, planted symlink not followed, garbage or oversized file reads as `None`) and this release's #1526 downgrade guard (`self_heal_from_older_binary_leaves_newer_release_alone`): self-heal from an older binary leaves a newer release alone. |
| `commands/install/tests/issue_1954_install_lock.rs` | Lock re-entrancy on one thread, a second thread or process waiting (ordering checked through a channel, not a timer), no lock passed to a child, a symlinked lock path refused, no truncation, mode 0600 and the exact wait message. |
| `update/tests/build_install_command.rs` (`exact_handoff_contract_matches_v1_2_107`) | The update hand-off has argv exactly `["install", "--force-refresh"]` and `AMPLIHACK_NO_UPDATE_CHECK=1`, `AMPLIHACK_NONINTERACTIVE=1` and `AMPLIHACK_POST_UPDATE_INSTALL=1` set, as in 1.2.107. Other variables (`AMPLIHACK_REPAIR_ORIGINAL_PATH`, `PATH`) are not part of the check. |
| `commands/recipe/run/tests_merge_guard_pin.rs` | Current guard pinning rejects unsafe ownership/modes and invalid release paths. Planned trusted-bundle tests must also reject PR-authored instruction fallback; that coverage is not established here. |
| `commands/install/command_staging/tests.rs` | The previous slash-command set is retired under `releases/.retired/`, not deleted. When it cannot be retired, it is removed only if the scan shows no process uses it, and otherwise kept as `.unretired-<n>` and checked again by the next install; a leftover `.staging` tree is never deleted unchecked. |
| `commands/install/tests/deploy_binary_tests.rs` (`stage_binary`) | The #302, #524 and #885 rules on the production path: a deleted source with an installed binary is left alone, a missing source and destination is an error, and re-staging the installed binary copies it without touching the previous release. |

These tests wait on events only: a FIFO or pipe read, `Child::wait`, a
channel or the lock itself. None uses a sleep, a deadline or a polling loop.
Each child process gets a cleared environment with `HOME` inside the test's
temporary directory, so the tests never touch the host's real `~/.amplihack`.

The host-level test reads each link at fixed points rather than sampling it
continuously. That a link never names a missing target during a switch
follows from `rename(2)` and is checked at the syscall level in
`issue_1954_release_layout.rs`.

## See also

- [amplihack install reference](../reference/install-command.md): install
  phases and uninstall
- [Install completeness](../reference/install-completeness.md): what an
  install must contain, including "a run alive across an install completes"
- [Idempotent installation](./idempotent-installation.md): what repeated
  installs change
- [Post-update re-exec](../features/update-reexec-new-binary.md): how
  `amplihack update` starts the new binary's install
- [Self-heal asset re-stage](../features/self-heal-asset-restage.md): the
  startup check that re-runs install
- [Framework source selection](../reference/framework-source-selection.md):
  the checks a staged bundle must pass
