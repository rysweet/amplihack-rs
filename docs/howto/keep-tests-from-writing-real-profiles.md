---
title: Keep Tests from Writing Real Profiles
type: howto
updated: 2026-10-27
related: issue #1699, issue #1690, issue #1661
---

# Keep Tests from Writing Real Profiles

`amplihack install` rewrites the hooks in every profile's `settings.json`
under the profile root (issue #1661). Before #1699, an install run with a
temporary `HOME` could still find the user's real profile root, through an
inherited `AMPLIHACK_CLAUDE_PROFILE_ROOT` or through a `profile-root` file
that pointed outside the temp `HOME`. It then wrote hook commands such as
`/tmp/.tmpQ3x9aB/home/.local/bin/amplihack-hooks` into real profiles. Once
the test deleted its temp dir, every Claude session in those profiles ran a
hook that no longer existed.

This guide explains the checks that now stop this, how to write a test that
touches profiles, and how to run the guard that catches a test writing to a
real profile root. It is for contributors who write tests.

## Contents

- [What stops a bad hook path](#what-stops-a-bad-hook-path)
- [Which writers are checked](#which-writers-are-checked)
- [Write a test that touches profiles](#write-a-test-that-touches-profiles)
- [Run the real profile root guard](#run-the-real-profile-root-guard)
- [When the guard fails](#when-the-guard-fails)
- [Limits](#limits)

## What stops a bad hook path

Before amplihack writes a hook command into a profile's `settings.json`, it
checks the `amplihack-hooks` path it is about to write. The checks live in
`crates/amplihack-cli/src/commands/profile/hook_path_guard.rs`.

| Check | Rule | Reason printed when it fails |
|-------|------|------------------------------|
| Binary exists | The path is absolute. | `hooks binary "<path>" is not an absolute path` |
| | The path is a regular file after following symlinks. | `hooks binary "<path>" does not exist` |
| Binary is safe to run | On Unix, an execute bit is set. | `hooks binary "<path>" is not executable` |
| | On Unix, the file is owned by you or by root. | `hooks binary "<path>" is owned by uid <uid>, not by you or root` |
| | On Unix, the file is not writable by group or others. | `hooks binary "<path>" is writable by group or others` |
| Binary is not temporary | If the binary is under the system temp dir, the profile directory must be under it too. | `hooks binary "<path>" is under temporary dir "<tmp>" but profile "<dir>" is not` |

The checks run in the order shown, and the first failure is the reason
printed. On Windows the "safe to run" checks are skipped; the other two
apply.

"The system temp dir" is `std::env::temp_dir()`: `$TMPDIR`, or `/tmp` when
`TMPDIR` is unset. Both sides are resolved through symlinks before they are
compared, so `/var/folders/…` and `/private/var/folders/…` on macOS count as
the same place. The comparison is by path component, so `/tmpfoo` is not
under `/tmp`. If the profile directory cannot be resolved, it is treated as
not temporary and a temporary binary is refused.

The third check is the one that matters for #1699. A temp `HOME` made with
`mktemp -d` or `tempfile` sits under the temp dir, so its
`~/.local/bin/amplihack-hooks` is temporary. A real profile root is not under
the temp dir, so the write is refused. A test whose profiles are also under
the temp dir still gets its hooks written.

The third check also matters for security. Another user on the same machine
can create a file at a deleted temp path, and a hook path left pointing there
would run that file as you.

The checks run just before the write. A binary that is swapped between the
check and a later Claude session is not detected.

## Which writers are checked

| Writer | What happens when the check fails |
|--------|-----------------------------------|
| `amplihack install` refreshing existing profiles | That profile is skipped and its `settings.json` is not opened for writing. The install still succeeds and prints `⚠️ Profile work: settings.json not refreshed (hooks binary "/tmp/.tmpQ3x9aB/home/.local/bin/amplihack-hooks" is under temporary dir "/tmp" but profile "/home/dev/.claude-profiles/work" is not)`. |
| Seeding a new profile (`amplihack profile add`) | Only the amplihack hook entries whose `amplihack-hooks` path fails the check are left out of the new profile. Your own hooks, `statusLine` and every other key are copied as before. A warning names each path that was left out. |
| Hook auto-repair in `~/.claude/settings.json` | Only the "binary exists" and "safe to run" checks apply, because the target is always `$HOME/.claude`. On failure, nothing is written and amplihack prints the warning shown below the table. |
| Auto-switch status line | Not checked. It writes `statusLine`, not hook paths, and a refresh keeps the existing `statusLine` value unchanged. |

The hook auto-repair warning:

```
amplihack: warning: hook auto-repair skipped (<reason>). Run `amplihack install`.
```

An amplihack hook entry is a command whose program is `amplihack-hooks`, or
`amplihack-hooks.exe` on Windows, given as a path. A bare `amplihack-hooks`
command, found through `PATH` when the hook runs, has no path to check, so
seeding copies it unchanged.

The explicit `amplihack install` write to `~/.claude/settings.json` is not
checked. It only ever writes into the `HOME` it runs under.

Warnings name only paths. They never contain `settings.json` contents,
command strings or environment values.

## Write a test that touches profiles

Start the binary through `isolated(..)`. It already calls `env_clear()`, so
a test started this way never inherited `AMPLIHACK_CLAUDE_PROFILE_ROOT`. It
now also calls `env_remove(PROFILE_ROOT_ENV)` explicitly, next to the temp
`HOME`, the home guard and the rotator opt-out (see
[Keep Tests from Leaking Rotator Daemons](./keep-tests-from-leaking-rotator-daemons.md)).
This is not the fix for #1699. It makes the rule visible and lets a test
assert it, so a later change to `isolated(..)` that stops clearing the
environment fails a test. The hook path checks above and the CI sentinel
below are what stop the bad writes:

```rust
use crate::isolated_amplihack::{isolated, PROFILE_ROOT_ENV};

let cmd = isolated(env!("CARGO_BIN_EXE_amplihack"));
// After env_clear(), std drops a removed key from get_envs() rather than
// listing it as (key, None); either way the child gets no value.
assert!(!cmd
    .get_envs()
    .any(|(k, v)| k == PROFILE_ROOT_ENV && v.is_some()));
```

`crates/amplihack-cli/tests/issue_1699_isolated_profile_root.rs` checks this.

A test that needs a profile root sets it again after `isolated(..)`, to a
directory inside its own temp dir:

```rust
let tmp = tempfile::tempdir().unwrap();
let home = tmp.path().join("home");
let root = tmp.path().join("profiles");
let mut cmd = isolated(env!("CARGO_BIN_EXE_amplihack"));
cmd.env("HOME", &home).env(PROFILE_ROOT_ENV, &root);
```

In-process tests that call `install::settings::refresh_profile_hooks` or the
seeding code change process-wide variables (`HOME`, `TMPDIR`,
`AMPLIHACK_CLAUDE_PROFILE_ROOT`, `AMPLIHACK_TEST_PROFILE_REFRESH`). Hold
`env_lock()` for the whole test and restore every variable in a `Drop`
guard, so a failing assertion cannot leave them changed.

A test that changes `TMPDIR` to make a sibling directory count as "outside
the temp dir" must point it at `test_support::fake_system_tmp_base()/tmp`,
not at a subdir of its own tempdir. Tests that do not hold `env_lock()` still
create tempdirs while `TMPDIR` is changed, and deleting the test's own
tempdir would delete theirs. The fake base lasts for the whole test process
and is removed at exit; put each test's own files in
`tempfile::tempdir_in(..)` under it.

If the test writes hooks, give it a real hooks binary. A made-up path such as
`/opt/amplihack/amplihack-hooks` now fails the "binary exists" check. Create
a stub instead. This example is Unix-only; gate it with `#[cfg(unix)]`, and
on Windows create an `amplihack-hooks.exe` file instead:

```rust
use std::os::unix::fs::PermissionsExt;

let bin = home.join(".local/bin/amplihack-hooks");
std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
std::fs::write(&bin, "#!/bin/sh\n").unwrap();
std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
```

Do not weaken a test's assertions to make it pass under the new checks. Add
the stub.

## Run the real profile root guard

`scripts/check-real-profile-root.sh` records every profile `settings.json`
under your real profile roots before the tests and compares them after. It
only reads files.

Keep the snapshot in `~/.amplihack/work/`, not `/tmp`, so a reboot does not
remove it mid-run:

```bash
mkdir -p ~/.amplihack/work
bash scripts/check-real-profile-root.sh snapshot ~/.amplihack/work/profile-root.snap
cargo test -p amplihack-cli
bash scripts/check-real-profile-root.sh compare ~/.amplihack/work/profile-root.snap
echo "exit: $?"
```

On a clean run:

```
check-real-profile-root: 4 settings.json files unchanged
exit: 0
```

### What it watches

The roots are taken from your real `HOME` when the snapshot is made:

- `$AMPLIHACK_CLAUDE_PROFILE_ROOT`
- the first line of `~/.amplihack/profile-root`
- `~/.amplihack/claude-profiles`
- `~/.claude-profiles`

Only roots that exist are used, and a root reached by two routes is counted
once. In each root it records `<root>/settings.json` and
`<root>/*/settings.json`: path, size and SHA-256. Modification times are not
compared.

It does not watch:

- credential and login files, because a running rotator changes them;
- `~/.claude/settings.json`, because Claude Code writes it during normal use.

With no roots, it prints this and exits 0:

```
check-real-profile-root: nothing to check
```

| Exit | Meaning | Output |
|------|---------|--------|
| 0 | No change, or nothing to check. | `check-real-profile-root: N settings.json files unchanged` or `check-real-profile-root: nothing to check` |
| 1 | A `settings.json` changed, appeared or disappeared. | One `CHANGED`, `ADDED` or `REMOVED` line per file, shown in [When the guard fails](#when-the-guard-fails). |
| 2 | Bad usage, the snapshot file is inside a watched root, or the snapshot file or its directory is missing. | One of the messages below, on stderr. |

The exit 2 messages:

```
usage: check-real-profile-root.sh snapshot|compare <snapshot-file>
check-real-profile-root: refusing snapshot file "<file>" inside watched root "<root>"
check-real-profile-root: snapshot directory "<dir>" does not exist
check-real-profile-root: no snapshot file "<file>"
```

### In CI

The `check` job runs the guard's self-test next to the #1690 one:

```bash
bash tests/issue_1699_real_root_guard_check.sh
```

It builds a decoy `HOME` and root and checks that an unchanged root passes, a
changed `settings.json` exits 1 and is named, the decoy is byte-identical
after the run, and a root path containing a space or `$(...)` runs no
command.

The `test` job creates a sentinel profile at
`$HOME/.claude-profiles/ci-sentinel/settings.json`, with hooks and a
`statusLine`, and exports `AMPLIHACK_CLAUDE_PROFILE_ROOT=$HOME/.claude-profiles`
for the test steps. That is the variable that caused #1699, so a test that
leaks it would write to the sentinel. The job takes a snapshot into
`$RUNNER_TEMP` before the tests and compares after them as **Real profile
root unchanged (issue #1699)**, with `if: always()`. The snapshot is never
uploaded.

## When the guard fails

Each change prints one line:

```
CHANGED /home/dev/.claude-profiles/work/settings.json
ADDED   /home/dev/.claude-profiles/scratch/settings.json
REMOVED /home/dev/.claude-profiles/old/settings.json
```

To find the cause:

1. Look at the hook commands in the changed file. A command that starts with
   a path under `/tmp` or `$TMPDIR` came from a test:

   ```bash
   grep -o '"command": *"[^"]*amplihack-hooks[^"]*"' \
     /home/dev/.claude-profiles/work/settings.json
   ```

2. Find tests that clear the environment or set a profile root but do not
   go through `isolated(..)` unchanged:

   ```bash
   grep -rlE 'AMPLIHACK_CLAUDE_PROFILE_ROOT|PROFILE_ROOT_ENV|profile-root' \
     --include='*.rs' crates tests \
     | xargs -r grep -lE '\.env_clear\(\)|set_var'
   ```

3. Fix the test so its profile root is inside its own temp dir.

On a developer machine, a change can also come from your own work in another
terminal, such as `amplihack profile add` or a real `amplihack install`. Run
the guard again with nothing else running before you look for a test.

## Limits

- The "binary is not temporary" check only knows the system temp dir. A
  temp `HOME` outside it (for example `HOME=/var/tmp/x` with `TMPDIR` unset)
  is not detected, and its hook path is written into real profiles. The
  checks run only at write time, so deleting that dir later does not remove
  the stale path: it stays until the next `amplihack install` from a real
  binary rewrites it. The CI sentinel and the guard script are what catch
  a test doing this.
- Installing from a build tree under `/tmp` into real profiles is now
  refused with a warning. Install from a binary outside the temp dir.
- A hooks binary owned by another non-root user (for example in a shared
  tool dir) is refused by the "safe to run" check.
- The guard watches `settings.json` only. It does not detect writes to other
  files in a profile.

## Related

- [Keep Tests from Leaking Rotator Daemons](./keep-tests-from-leaking-rotator-daemons.md) - The other test leak check, for `rotate watch` daemons
- [Manage Claude Login Profiles](./claude-profiles.md) - Profile roots and what a new profile inherits
- [`amplihack profile` reference](../reference/profile-command.md#seeding) - How `amplihack install` refreshes profile hooks
- [AMPLIHACK_* environment variables](../reference/amplihack-env-vars.md) - `AMPLIHACK_CLAUDE_PROFILE_ROOT`
- [Developing amplihack](./develop-amplihack.md) - Build and test setup
