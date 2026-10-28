---
title: Keep Tests out of the Real Profile Root
type: howto
updated: 2026-10-28
related: issue #1699, issue #1661, issue #1690
---

# Keep Tests out of the Real Profile Root

`amplihack install` refreshes the hooks in every profile's `settings.json`
under the profile root (issue #1661). Before #1699, a test install with a
temp `HOME` could inherit a real `AMPLIHACK_CLAUDE_PROFILE_ROOT` and rewrite
every real profile so that its hooks pointed at
`/tmp/.tmpXXXX/home/.local/bin/amplihack-hooks`, a binary deleted when the
test ended.

This guide is for contributors who write tests. It describes the three
controls that keep a test run away from the real profile root, how to write
a test that exercises the refresh, and what to do when the guard fails.

## Contents

- [The three controls](#the-three-controls)
- [Write a test that starts the binary](#write-a-test-that-starts-the-binary)
- [Test the profile refresh in process](#test-the-profile-refresh-in-process)
- [When the real-root guard fails](#when-the-real-root-guard-fails)
- [Limits](#limits)

## The three controls

| Control | Where | What it does |
|---------|-------|--------------|
| Refusal | `refresh_profile_settings` in `crates/amplihack-cli/src/commands/profile/settings_refresh.rs` | Refuses the refresh when the hooks binary is missing, or is under a temp directory while the profile root is outside `HOME`. See [the profile reference](../reference/profile-command.md#seeding). |
| Isolation | `tests/support/isolated_amplihack.rs` | `isolated(..)` clears the environment and also calls `.env_remove(PROFILE_ROOT_ENV)`, so no child inherits `AMPLIHACK_CLAUDE_PROFILE_ROOT`. |
| Guard | `crates/amplihack-cli/tests/issue_1699_real_root_guard.rs` | Hashes the real profiles' `settings.json` files before `main` on Linux (when the guard test runs on other Unix systems) and again when the test process exits, and fails the run if any changed. |

The refusal is the only one that protects users. It is enforced in
`refresh_profile_settings`, the single production entry point; `refresh_in`
is unchecked and is called directly only by unit tests.

`env_clear()` in `isolated(..)` remains the main isolation control. The
explicit `env_remove` is a second layer that still holds if a later change
adds the profile root to the list of variables `isolated(..)` keeps.
`crates/amplihack-cli/tests/issue_1699_isolated_profile_root.rs` checks it
in two ways. One test sets the variable and asserts that the built
`Command` gives the child no value for it; after `env_clear()` this only catches a kept list
that copies the variable. A source assertion checks that
`tests/support/isolated_amplihack.rs` calls `env_remove(PROFILE_ROOT_ENV)`
after the loop that copies the kept variables, so deleting that line fails
the test.

A test that calls `run_install` in process is outside both `isolated(..)`
and the library's `cfg!(test)` gate, so it must call
`child_remove_var("AMPLIHACK_CLAUDE_PROFILE_ROOT")` itself.
`in_process_installs_remove_the_profile_root` in the same file fails for any
file under `crates/amplihack-cli/tests` that calls `run_install` without
that line.

## Write a test that starts the binary

Start the binary through `isolated(..)` and give it a profile root inside
the temp `HOME`. A root outside `HOME` combined with a hooks binary under
the temp directory is exactly the case the refusal blocks, so such a test
sees `Skipped` for every profile:

```rust
use crate::isolated_amplihack::{isolated, PROFILE_ROOT_ENV};

let tmp = tempfile::tempdir().unwrap();
let home = tmp.path().join("home");
let mut cmd = isolated(env!("CARGO_BIN_EXE_amplihack"));
cmd.env("HOME", &home)
    .env(PROFILE_ROOT_ENV, home.join("profiles"))
    .args(["install"]);
let out = cmd.output().unwrap();
assert!(out.status.success());
```

Set the profile root only to a path under the test's own temp directory.
Never copy the parent's value.

## Test the profile refresh in process

Choose the call by what the test checks:

| Test checks | Call |
|-------------|------|
| The refusal rule itself | `hook_path_refusal(hooks_bin, root, home, temp_bases)` with injected `temp_bases` |
| The JSON edit on a fixture | `refresh_in(&store, update)` (unchecked) |
| The whole install path | `refresh_profile_hooks` under `env_lock()`, with `HOME` and the root both in one temp directory |

`hook_path_refusal` and `refresh_in` take all their inputs as arguments and
are called directly; they need no environment variables. In-crate calls to
`refresh_profile_hooks` reach the refresh only when
`AMPLIHACK_TEST_PROFILE_REFRESH` is set, and the test must hold `env_lock()`
while it changes process variables.

For the whole install path, create a real executable hooks file at
`<home>/.local/bin/amplihack-hooks` and put the root at `<home>/profiles`.
`install_registers_this_builds_hooks_in_each_profile` in
`crates/amplihack-cli/src/commands/install/tests/profile_refresh_install.rs`
uses this layout.

## When the real-root guard fails

The guard prints the changed paths and a hint to re-run, then exits the test
process with status 1. This happens after libtest prints `test result: ok`, and
no test is named as the failure:

```text
issue_1699 real-root guard: real profile root changed during this test run:
  /home/user/.claude-profiles/work/settings.json
issue_1699 real-root guard: another process can cause this (`amplihack install`, `profile add`, rotator status-line setup or a rotator restart during the run); re-run the tests before investigating
```

1. Check whether you changed the file yourself during the run, for example
   with `amplihack install` or `/config` in a live session, or by a rotator
   restart, because `record_root` rewrites `<root>/.auto-switch.d/rotate/root`.
   The guard compares content, not mtimes, so only a real change trips it.
   If so, run the tests again.
2. Otherwise a test wrote the file. List the tests that set `HOME` without
   `isolated(..)`:

   ```bash
   grep -rnE 'set_var\("HOME"|\.env\("HOME"' crates bins tests
   ```

   Each match must either go through `isolated(..)` or remove
   `AMPLIHACK_CLAUDE_PROFILE_ROOT` in the same scope.
3. Restore the profile's hooks by running your installed `amplihack install`
   from a normal shell. It rewrites amplihack's hook entries and keeps
   everything else.

The guard reads only `<root>/<profile>/settings.json` and
`<root>/.auto-switch.d/rotate/root`, for the resolved root,
`~/.claude-profiles` and `~/.amplihack/claude-profiles`. It never writes
under those roots, never reads `.credentials.json`, does not follow
symlinks, hashes at most 1 MiB per file and prints only paths. Roots under a
temp directory are skipped, so on CI, where no real root exists, it checks
nothing and passes.

## Limits

- The guard takes its baseline in `.init_array` before `main` on Linux, and
  when the guard test runs on other Unix systems, so it does not see changes
  made before that point. It does not see changes made by other test
  binaries; libtest has no hook that runs before or after the whole suite.
- A `settings.json` that cannot be read both before and after the run is not
  compared. A change in readability is reported.
- On failure the guard calls `_exit(1)`, which skips later exit handlers. A
  leftover temp directory is possible.
- The refusal does not cover a real `HOME` with a hooks binary under the temp
  directory and a root under `HOME`, or scratch directories that are not
  temp bases.

## Related

- [`amplihack profile` reference](../reference/profile-command.md#seeding) - The #1661 refresh and the #1699 refusal
- [Keep Tests from Leaking Rotator Daemons](./keep-tests-from-leaking-rotator-daemons.md) - The other stops `isolated(..)` sets
- [AMPLIHACK_* environment variables](../reference/amplihack-env-vars.md) - `AMPLIHACK_CLAUDE_PROFILE_ROOT`
- [Developing amplihack](./develop-amplihack.md) - Build and test setup
