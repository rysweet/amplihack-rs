---
title: Keep Tests from Leaking Rotator Daemons
type: howto
updated: 2026-10-26
related: issue #1690, issue #1656, issue #1592
---

# Keep Tests from Leaking Rotator Daemons

A launch command (`amplihack claude`, `amplihack launch`,
`amplihack profile run`) can start the profile rotator in the background with
`amplihack profile rotate watch --spawned`. That process is detached with
`setsid`, so it outlives the test that launched it. Before #1690, test runs
left these daemons behind: they ran from `target*/debug/amplihack`, their
parent was PID 1, and their `HOME` was a temp directory the test had already
deleted.

This guide explains the three stops that keep tests from starting a real
rotator, how to run the leak check, and what to do when it fails. It is for
contributors who write tests. Users' autostart is unchanged; see
[Start the rotator automatically](./claude-profile-rotation.md#start-the-rotator-automatically).

## Contents

- [The three stops](#the-three-stops)
- [Write a test that starts the binary](#write-a-test-that-starts-the-binary)
- [Run the leak check](#run-the-leak-check)
- [When the leak check fails](#when-the-leak-check-fails)
- [Test a real daemon on purpose](#test-a-real-daemon-on-purpose)
- [Limits](#limits)

## The three stops

| Stop | Where | What it does | Covers |
|------|-------|--------------|--------|
| A | `tests/support/isolated_amplihack.rs` | `isolated(..)` sets `AMPLIHACK_ROTATE_AUTOSTART=0` next to `AMPLIHACK_TEST_HOME_GUARD=1`. | Every test that starts the binary through `isolated(..)` and keeps its environment. |
| B | `autostart::run()` in `crates/amplihack-cli/src/commands/profile/rotate/autostart.rs` | The first check: if `AMPLIHACK_TEST_HOME_GUARD` is set and not empty, return `Outcome::Skipped("test")`. | Fixtures that call `env_clear()` after `isolated(..)` and set the guard again, which drops stop A. |
| C | `autostart_on_launch` in the same file | Test builds (`#[cfg(test)]`) use a spawner that always refuses. Other builds use `daemon::spawn`. | In-crate unit tests, which never set the guard. |

Stop B comes before every other autostart check. A skipped launch reads no
setting, creates no file or directory, takes no lock and prints nothing to
stdout or stderr. It cannot skip the checks and still spawn: the only result
is `Skipped("test")`.

Stop B is not a user setting. Only the test harness sets
`AMPLIHACK_TEST_HOME_GUARD` (see the
[environment variable reference](../reference/amplihack-env-vars.md)). Under
the guard, `rotate status` can still print `autostart: on (default)`, because
the status text describes the setting, not the test harness.

Stop C is compiled only into test builds. Release and debug binaries use the
real spawner, so a user's launch starts the rotator exactly as before.

## Write a test that starts the binary

Start the binary through `isolated(..)`. It clears the environment and gives
the child a temp `HOME`, the home guard and the autostart opt-out. In
`crates/amplihack-cli/tests/`, `main.rs` already includes the helper with
`#[path = "../../../tests/support/isolated_amplihack.rs"]`:

```rust
use crate::isolated_amplihack::isolated;

let mut cmd = isolated(env!("CARGO_BIN_EXE_amplihack"));
cmd.args(["profile", "rotate", "status"]);
let out = cmd.output().unwrap();
assert!(out.status.success());
```

`crates/amplihack-cli/tests/issue_1690_isolated_autostart.rs` checks that
`isolated(..)` sets both variables:

| Variable | Value |
|----------|-------|
| `AMPLIHACK_TEST_HOME_GUARD` | `1` |
| `AMPLIHACK_ROTATE_AUTOSTART` (`ROTATE_AUTOSTART_ENV`) | `0` |

A test that needs its own `HOME` sets it again after `isolated(..)`; the
guard and the opt-out stay. If your fixture calls `env_clear()` after
`isolated(..)`, set the guard again afterwards. Stop B then still holds even
though stop A is gone:

```rust
use crate::isolated_amplihack::{isolated, HOME_GUARD_ENV};

let tmp = tempfile::tempdir().unwrap();
let mut cmd = isolated(env!("CARGO_BIN_EXE_amplihack"));
cmd.env_clear()
    .env("PATH", std::env::var_os("PATH").unwrap())
    .env("TMPDIR", std::env::temp_dir())
    .env("HOME", tmp.path().join("home"))
    .env(HOME_GUARD_ENV, "1");
```

Keep `TMPDIR` as well. The home guard accepts only a `HOME` under the child's
temp dir, so if your `TMPDIR` is not `/tmp` and the child loses it, the guard
rejects the test's `HOME`.

A fixture that calls `env_clear()` and does not set the guard again has no
stop. Only the leak check catches it.

## Run the leak check

`scripts/check-leaked-rotators.sh` lists rotator daemons that a test run left
behind. Run it after the test suite:

```bash
cargo nextest run --workspace
bash scripts/check-leaked-rotators.sh
echo "exit: $?"
```

On a clean run:

```
check-leaked-rotators: no leaked rotator daemons
exit: 0
```

A process counts as a leak only when all of these hold:

- It belongs to the current user.
- Its argv contains `profile`, `rotate`, `watch` and `--spawned` as separate
  entries.
- Its executable is under a `target*/` directory, under `$CARGO_TARGET_DIR`,
  or under this repo's `target/`. Those last two are resolved through
  symlinks first, so a `target/` that links to, say,
  `/d0/cargo-targets/<name>/` still counts. A ` (deleted)` suffix on
  `/proc/<pid>/exe` is ignored; if the link cannot be read, argv[0] is used.
- Its `HOME` is under `$TMPDIR` or `/tmp`.

Your own rotator (`~/.local/bin/amplihack`, `HOME=/home/<you>`) never matches,
and the script never sends a signal to any process.

| Exit | Meaning |
|------|---------|
| 0 | No leaked rotator, or not Linux (no `/proc`; prints a note). |
| 1 | At least one leaked rotator. One line each is printed. |
| 2 | `/proc` exists but the scan could not read its own entry or saw no processes. |

CI runs the check in the `test` job after both nextest steps and
`cargo test --doc`, as **No leaked rotator daemons (issue #1690)**, with
`if: always()` so it also runs after a failing test. The `check` job runs the
self-test `tests/issue_1690_leaked_rotator_check.sh`. That script starts a
decoy process that looks like a leaked rotator, checks that the detector
exits 1 and names its pid, then kills only the decoy and checks for exit 0:

```bash
bash tests/issue_1690_leaked_rotator_check.sh
```

## When the leak check fails

Each leak prints one line:

```
leaked rotator: pid=48213 exe=/home/dev/amplihack-rs/target/debug/amplihack HOME=/tmp/.tmpQ3x9aB/home AMPLIHACK_CLAUDE_PROFILE_ROOT=/tmp/.tmpQ3x9aB/profiles
```

The output holds only those four fields. Control characters are removed and
each value is cut to about 256 bytes, because CI logs are public and a full
environment can hold tokens.

To find the test that started it:

1. Search the tests for fixtures that launch with auto-switch on and drop the
   guard. `env_clear()` without a following `AMPLIHACK_TEST_HOME_GUARD` (or
   `HOME_GUARD_ENV`) is the usual cause. This lists files that call
   `env_clear()` and name neither:

   ```bash
   grep -rln 'env_clear()' crates/amplihack-cli/tests tests \
     | xargs -r grep -LE 'AMPLIHACK_TEST_HOME_GUARD|HOME_GUARD_ENV'
   ```

2. If the profile root still exists, its `rotate.err` names the launch that
   started the daemon.
3. Fix the fixture by setting the guard again after `env_clear()`, or by using
   `isolated(..)` unchanged.

To stop a leaked daemon on your machine, check the pid again first. PIDs are
reused, so make sure it is still a `rotate watch --spawned` process running a
test-build binary with a temp `HOME`, then kill that pid alone:

```bash
pid=48213
tr '\0' ' ' < "/proc/$pid/cmdline"; echo   # must contain: profile rotate watch --spawned
readlink "/proc/$pid/exe"                   # must be a test build (see above)
tr '\0' '\n' < "/proc/$pid/environ" | grep -E '^HOME='   # must be under $TMPDIR or /tmp
kill "$pid"
```

If any of the three checks does not match, the pid now belongs to another
process. Do not kill it.

Do not use `pkill -f 'rotate watch'`. It also matches your real rotator.

## Test a real daemon on purpose

The explicit `amplihack profile rotate start` path has no test stop. Stop B
covers autostart only. The `issue_1592_daemon` tests need a real daemon: they
start it with `--interval 1` and stop it themselves. If such a test fails
before it stops its daemon, the leak check reports it. That is intended.

A test that needs a real autostart must remove the guard on purpose with
`cmd.env_remove("AMPLIHACK_TEST_HOME_GUARD")` and must stop the daemon it
starts. No test does this today.

## Limits

- A fixture that runs `env_clear()` and never sets the guard again bypasses
  stops A and B. Only the leak check catches it.
- Only an empty or unset guard lets autostart run. Any other value, even
  one that is not valid UTF-8, skips it. The harness always sets `1`.
- The leak check misses a daemon whose `HOME` is outside `$TMPDIR` and
  `/tmp`. Test homes are always `tempfile` directories.
- On a developer machine the leak check can report a daemon from another
  worktree's test run that is still in progress. CI runners start clean.

## Related

- [Rotate Claude Logins](./claude-profile-rotation.md) - What the rotator does and when a launch starts it
- [`amplihack profile` reference](../reference/profile-command.md) - `rotate start`, `rotate watch` and `rotate autostart`
- [AMPLIHACK_* environment variables](../reference/amplihack-env-vars.md) - `AMPLIHACK_TEST_HOME_GUARD` and `AMPLIHACK_ROTATE_AUTOSTART`
- [Developing amplihack](./develop-amplihack.md) - Build and test setup
