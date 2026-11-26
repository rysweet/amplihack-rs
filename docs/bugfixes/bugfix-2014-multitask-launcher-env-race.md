---
title: "Bug Fix #2014: multitask launcher tests no longer read the process profile variables"
type: explanation
issue: 2014
updated: 2026-11-09
files:
  - crates/amplihack-cli/src/commands/multitask/command_builder.rs
  - crates/amplihack-cli/src/commands/multitask/launcher.rs
  - crates/amplihack-cli/src/commands/multitask/command_builder_tests.rs
  - crates/amplihack-cli/src/commands/multitask/launcher_tests.rs
  - crates/amplihack-cli/src/commands/multitask/issue_1570_launcher.rs
  - crates/amplihack-cli/src/commands/multitask/issue_1966_criticality.rs
  - crates/amplihack-cli/src/commands/multitask/issue_2014_launcher_env.rs
---

# Bug Fix #2014: Multitask Launcher Tests and the Profile Root Variable

> **Issue:** [#2014](https://github.com/rysweet/amplihack-rs/issues/2014)

The multitask launcher builders now take the environment as arguments. The
tests pass an empty environment, so they no longer depend on what other tests
in the same process do to `AMPLIHACK_CLAUDE_PROFILE` and
`AMPLIHACK_CLAUDE_PROFILE_ROOT`. The only change in production behaviour is that
`launch_workstream` no longer panics when any environment variable is not
valid UTF-8.

## Contents

- [Cause](#cause)
- [The seam](#the-seam)
- [How the tests use it](#how-the-tests-use-it)
- [The regression tests](#the-regression-tests)
- [Running the tests](#running-the-tests)
- [Limits](#limits)
- [Related](#related)

## Cause

`write_recipe_launcher` and `write_classic_launcher` build `run.sh` with
`export` lines for the parent's Claude login profile (issue #1567). They read
`AMPLIHACK_CLAUDE_PROFILE` and `AMPLIHACK_CLAUDE_PROFILE_ROOT` from the
process when they are called. A value that is set but not valid UTF-8 is
refused, because dropping it could start the nested session under a
different account:

```text
AMPLIHACK_CLAUDE_PROFILE_ROOT is set but is not valid UTF-8; refusing to start the workstream because the nested launch could not keep the parent's Claude profile. Unset it or set it to a UTF-8 value
```

libtest runs the tests of one binary on parallel threads of one process.
`run_scripts_refuse_a_non_utf8_profile_root` set the root to `/\xff` under
`env_lock()`, but `classic_launcher_script_is_lf_only_and_bash_valid` and
`recipe_launcher_scripts_are_lf_only_and_bash_valid` read the variable
without the lock. When the two overlapped, the launcher test failed with the
error above. Each test passed when run alone.

## The seam

The two script builders and two new launcher functions take the profile
getter and the auto-switch substitute as arguments. The original launcher
functions keep their signatures and pass the production functions.

| Function | Module | Environment |
|----------|--------|-------------|
| `recipe_run_script(ws, delegate, get, substitute)` | `command_builder.rs` | Arguments only |
| `classic_run_script(ws, delegate, get, substitute)` | `command_builder.rs` | Arguments only |
| `write_recipe_launcher_with(ws, delegate, get, substitute)` | `launcher.rs` | Arguments only |
| `write_classic_launcher_with(ws, delegate, get, substitute)` | `launcher.rs` | Arguments only |
| `write_recipe_launcher(ws, delegate)` | `launcher.rs` | Process, through `env_var_os` and `substitute_inherited_profile` |
| `write_classic_launcher(ws, delegate)` | `launcher.rs` | Process, through `env_var_os` and `substitute_inherited_profile` |

The parameter types are:

```rust
get: impl Fn(&str) -> Option<OsString>,
substitute: impl Fn(&str) -> Option<String>,
```

`orchestrator.rs` still calls the two-argument functions, so a workstream
started by `amplihack multitask` exports the same profile lines as before.
`profile_exports` and its error text are unchanged; the error names the
variable and never prints the value.

The `_with` functions build every script before they write any file, as the
originals did. A refused export leaves no `run.sh` or `launcher.sh` behind,
and `context.json` is still written with mode 0600.

`env_var_os` in `command_builder.rs` is `pub(super)` so that `launcher.rs`
can pass it. All the new functions are `pub(super)` and are not visible
outside the `multitask` module.

The session-tree variables (`AMPLIHACK_SESSION_DEPTH`, `AMPLIHACK_TREE_ID`,
`AMPLIHACK_MAX_DEPTH` and `AMPLIHACK_MAX_SESSIONS`) are still read from the
process. Those reads fall back to defaults and cannot fail.

## How the tests use it

Tests in the `multitask` module pass the environment explicitly and do not
change the process environment. `command_builder_tests.rs` provides three
helpers:

| Helper | Returns |
|--------|---------|
| `no_env` | `None` for every key: no profile is pinned |
| `keep` | `None`: the inherited profile name is not substituted |
| `getter(&[(key, value)])` | The listed UTF-8 values, `None` for anything else |

`no_env` and `keep` are `pub(super)` so that the other test files in the
module can import them. `getter` stays private to `command_builder_tests.rs`.

A launcher test that does not care about profiles passes the first two:

```rust
use super::command_builder_tests::{keep, no_env};
use super::launcher::write_classic_launcher_with;

let base = tempfile::tempdir().unwrap();
let state = tempfile::tempdir().unwrap();
let ws = test_workstream(base.path(), state.path());
fs::create_dir_all(&ws.work_dir).unwrap();

write_classic_launcher_with(&ws, "amplihack copilot", no_env, keep).unwrap();
assert_script_is_lf_only_and_bash_valid(&ws.work_dir.join("run.sh"));
```

A test of the exports passes the values it needs. `impl Fn` is not `Copy`, so
build one getter for each call:

```rust
let recipe = recipe_run_script(
    &ws,
    "amplihack claude",
    getter(&[(PROFILE_VAR, "work"), (ROOT_VAR, "/p/it's")]),
    keep,
)
.unwrap();
assert!(recipe.contains(r"export AMPLIHACK_CLAUDE_PROFILE_ROOT='/p/it'\''s'"));
```

A non-UTF-8 value is passed through a closure, as in
`run_scripts_refuse_a_non_utf8_profile_root`, which no longer sets the
variable in the process:

```rust
use std::os::unix::ffi::OsStringExt;

let bad_root = |k: &str| match k {
    PROFILE_VAR => Some(OsString::from("work")),
    ROOT_VAR => Some(OsString::from_vec(vec![b'/', 0xff])),
    _ => None,
};
let err = classic_run_script(&ws, "amplihack claude", bad_root, keep).unwrap_err();
let msg = format!("{err:#}");
assert!(msg.contains("AMPLIHACK_CLAUDE_PROFILE_ROOT"), "{msg}");
assert!(msg.contains("not valid UTF-8"), "{msg}");
```

The following changes go with the seam:

- `with_profile_env` is removed. It set both variables in the process, and
  `with_profile_env(Some("work"), ..)` could reach the developer's real
  profile store through `substitute_inherited_profile`.
- `issue_1570_launcher.rs` calls
  `write_recipe_launcher_with(&ws, "amplihack claude", no_env, keep)` and no
  longer has its own `without_profile` wrapper.
- `issue_1966_criticality.rs`, which reached `main` after this fix was
  written, makes the same call in its three launcher tests and imports
  `no_env` and `keep` from `command_builder_tests.rs`.

With an empty getter, `substitute_inherited_profile` and
`ProfileStore::from_env()` are never called, so these tests do not read any
real profile root.

## The regression tests

`issue_2014_launcher_env.rs` is compiled only on Unix and holds four tests.
They reuse `test_workstream` and `assert_script_is_lf_only_and_bash_valid`
from `launcher_tests.rs`.

### `process_reading_launchers_see_a_non_utf8_root_and_the_seam_does_not`

This test checks that the production wrappers still read the process and
that the seam does not. It holds the shared environment lock through
`with_env`, unsets `AMPLIHACK_CLAUDE_PROFILE`, sets
`AMPLIHACK_CLAUDE_PROFILE_ROOT` to `OsStr::from_bytes(b"/\xff")`, and
checks the following:

- `write_recipe_launcher` and `write_classic_launcher` return an error whose
  `{:#}` text contains `AMPLIHACK_CLAUDE_PROFILE_ROOT` and `not valid UTF-8`,
  and leave no `run.sh` or `launcher.sh` in the work directory.
- `write_recipe_launcher_with` and `write_classic_launcher_with`, given
  `no_env` and `keep`, succeed.

The profile name is unset, so the real auto-switch substitution never runs.
If a later change makes the wrappers pass an empty getter, the first check
fails. Such a change would drop the parent's profile pin in production.

### `injected_profile_reaches_the_written_run_scripts`

This test passes a getter that pins `work` with the root `/p/it's`, and a
substitute that replaces `work` with `spare`, to both `_with` launchers. It
checks that each written `run.sh` exports `spare` and the quoted root, so the
seam passes the injected values through instead of dropping them.

### `launcher_tests_pass_while_another_thread_sets_a_non_utf8_root`

This test reproduces the race from the issue and checks that the seam is
immune to it.

1. A mutator thread opens one `with_env` window that sets
   `AMPLIHACK_CLAUDE_PROFILE_ROOT` to `/\xff`. Inside the window it sends on
   a `ready` channel, then blocks on a `done` channel.
2. The main thread blocks on `ready`. When it returns, the invalid value is
   in the process, and the test asserts that it is.
3. Without taking the lock, the main thread builds the recipe and classic
   launchers in fresh temporary directories through the `_with` functions,
   then checks `launcher.sh` and `run.sh` for LF-only content and with
   `bash -n`.
4. The main thread closes `done` and joins the mutator. `with_env` restores
   the previous value when the window closes.

The overlap comes from the handshake, not from timing. The test has no
sleeps, deadlines or polling loops, and the invalid root is in the process
for one window only. Other tests that need `env_lock()` wait only for the
time the checks take.

A `StopGuard` drops the `done` sender and joins the thread in `Drop`, so a
panic in the main thread closes the window and does not leave the mutator
running. If the checks are pointed at `write_recipe_launcher` and
`write_classic_launcher` instead, the test fails with the UTF-8 error from
[Cause](#cause).

### `launch_workstream_tolerates_a_non_utf8_variable_and_sets_the_delegate`

`launch_workstream` used to copy the process environment with
`std::env::vars()` before it started `run.sh`. That call panics when any
variable is not valid UTF-8, so `amplihack multitask` could crash instead of
starting the workstream. The child now inherits the environment, and only
`AMPLIHACK_DELEGATE` is set on its `Command`, so the child sees the same
variables as before.

The test sets `AMPLIHACK_CLAUDE_PROFILE_ROOT` to `/\xff` under `with_env`,
launches a `run.sh` that writes `$AMPLIHACK_DELEGATE` to a file, waits for
the child, and checks the file.

## Running the tests

Run the `multitask` tests, then the regression tests on their own:

```bash
cargo test -p amplihack-cli --lib -- multitask::
cargo test -p amplihack-cli --lib issue_2014
```

To check for the intermittent failure, run the whole library suite several
times:

```bash
for i in 1 2 3 4 5; do
  cargo test -p amplihack-cli --lib || break
done
```

## Limits

- The mutator holds an invalid root for one window while the regression
  test runs. A test elsewhere in the binary that reads
  `AMPLIHACK_CLAUDE_PROFILE_ROOT` through production code, for example
  `root_config::store_from_env`, without `env_lock()` would see that value
  and fail. The fix for such a failure is to take `env_lock()` in that test,
  or to pass the value as an argument as described here.
- Every remaining in-process change to `AMPLIHACK_CLAUDE_PROFILE_ROOT` in
  `crates/amplihack-cli/src` is made under `env_lock()`, directly or through
  `with_env`. A test that only reads the variable gains no protection from
  that lock unless it takes the lock too.
- `write_classic_launcher` writes `TASK.md` with the default umask, not 0600.
  This is unchanged by this fix.

## Related

- [Keep Tests out of the Real Profile Root](../howto/keep-tests-out-of-the-real-profile-root.md#pass-the-profile-variables-to-multitask-launchers) - Isolation rules for tests that touch `AMPLIHACK_CLAUDE_PROFILE_ROOT`
- [Keep Tests from Changing PATH and HOME](../howto/keep-tests-from-changing-path-and-home.md) - The same problem for `PATH` and `HOME`, and the `_with` seam pattern
- [`amplihack multitask` reference](../reference/multitask-command.md) - The command that writes these launchers
- [AMPLIHACK_* environment variables](../reference/amplihack-env-vars.md) - `AMPLIHACK_CLAUDE_PROFILE` and `AMPLIHACK_CLAUDE_PROFILE_ROOT`
