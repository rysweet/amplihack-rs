---
title: Keep Tests from Writing to an Inherited Claude Profile
type: howto
updated: 2026-11-21
related: issue #2922, issue #1699, issue #1526
---

# Keep Tests from Writing to an Inherited Claude Profile

On 21 November 2026 a `cargo test --workspace` run replaced a live Claude
profile's `settings.json` with a fixture's settings. The test run had been
started inside an agent launch, so its environment held `CLAUDE_CONFIG_DIR`
and `AMPLIHACK_CLAUDE_PROFILE`. A test pointed `HOME` at a temporary folder
and left those two variables alone. The install code honours
`CLAUDE_CONFIG_DIR` when it is set, so it wrote the fixture's hooks, with
paths into a deleted temporary home, into the real profile (issue #2922).

This guide is for contributors who write tests. It describes what the shared
helper clears, how it is applied, and how to write a test that needs one of
the variables set.

## Contents

- [What the helper clears](#what-the-helper-clears)
- [How it is applied](#how-it-is-applied)
- [Write a test that sets HOME](#write-a-test-that-sets-home)
- [Point CLAUDE_CONFIG_DIR inside the temporary home](#point-claude_config_dir-inside-the-temporary-home)
- [Give a child process a clean environment](#give-a-child-process-a-clean-environment)
- [Check that a profile stays unchanged](#check-that-a-profile-stays-unchanged)
- [Limits](#limits)

## What the helper clears

The helper is defined once, in `tests/support/claude_profile_isolation.rs`,
and included by `#[path]` into the `src/test_support.rs` of every crate that
has one, into `cli_integration`'s `tests/support/mod.rs`, into
`tests/support/isolated_amplihack.rs` and into `tests/golden/run_golden.rs`.
It uses the standard library only, and it lives under `tests/`, so it
compiles into no release binary.

It owns exactly five variables, listed in the constant
`CLAUDE_PROFILE_ENV_VARS`:

| Variable | Why it matters |
| --- | --- |
| `CLAUDE_CONFIG_DIR` | Names the directory the install code writes `settings.json` into. |
| `AMPLIHACK_CLAUDE_PROFILE` | Names the active profile. |
| `AMPLIHACK_CLAUDE_PROFILE_ROOT` | Names the folder that holds every profile. |
| `AMPLIHACK_CLAUDE_AUTO_SWITCH` | Enables the auto-switch launch path. |
| `AMPLIHACK_HOME` | Redirects the framework root. |

The helper clears these variables. It does not redirect them. A cleared
`CLAUDE_CONFIG_DIR` makes the install code fall back to the path derived
from `HOME`, which is inside the temporary folder. Clearing therefore cannot
change an existing assertion about where the settings file lands.

Names come from the product constants (`PROFILE_ENV`, `ROOT_ENV`,
`TOGGLE_ENV`) where they exist. A test in `amplihack-cli`
(`issue_2922_helper_contract`) checks that the list contains each of them, so
a product rename fails the build instead of leaving the list stale.

## How it is applied

Isolation follows from setting `HOME` at all. Tests do not need to name the
helper.

| Primitive | Behaviour |
| --- | --- |
| `env_lock()` | Creating the lock fires `purge_inherited()` once per process, which removes the five variables. Nothing restores them. |
| `HomeGuard::set` | Takes a `ClaudeProfileIsolation` with the new `HOME` and carries it as a field. `HOME` is restored first on drop, then the five. |
| `set_home` | Fires `purge_inherited()`. It returns only the old `HOME`, so it cannot restore the five; use `HomeGuard` when restore on drop matters. |
| `EnvGuard::set`, `env_paths`, `with_env` | Take a `ClaudeProfileIsolation` when the key set contains `HOME`. The `EnvGuard` carries it; `with_env` holds it for the call. A key set with no `HOME` in it leaves the five alone. |
| `EnvGuard::assign` | Takes one when the key is `HOME` and the guard has none already, so a guard built with `EnvGuard::capture` and then pointed at a temporary home is isolated in the same way. |
| `IsolatedHome::new` | Carries a `ClaudeProfileIsolation` field, which is the sole owner of the five in that guard. The names are no longer in its own clear list. |

Coverage differs by crate:

| Crate or file | What it gets |
| --- | --- |
| `amplihack-cli` | `env_lock()`, `HomeGuard`, `set_home`, `EnvGuard`, `env_paths` and `IsolatedHome`, as described above. |
| `amplihack-utils` | `env_lock()`, `HomeGuard`, `set_home` and `EnvGuard::set`. |
| `amplihack-memory` | `env_lock()`, `HomeGuard` and `set_home`. It has no `EnvGuard`. |
| `amplihack-hooks` | `env_lock()`, and `isolate_state_home`, which now carries a `ClaudeProfileIsolation` instead of its own `AMPLIHACK_HOME` guard. It has no `EnvGuard` or `set_home`. |
| `amplihack-types`, `amplihack-context` | `env_lock()` only, with the same one-shot purge. |
| `tests/golden/run_golden.rs` | Fires the purge from `golden_env_lock()`. `GoldenEnv` already keeps only `isolated_hook::is_kept` names. |
| `quiet_process` | Fires the purge in each quiet child before the test body runs, and again in `child_set_var` when the key is `HOME`. This covers `cli_integration`, which has no `env_lock()`. |
| `isolated_amplihack` | Removes the five from every `amplihack` command a test starts, on top of its `env_clear()` and allowlist. |

Direct `env::set_var("HOME", ...)` calls elsewhere are not converted to these
primitives. They do not need to be: each one sits in a binary that also has an
`env_lock()`, whose first acquisition has already removed the five from the
process, and holding that lock is already required for the call to be sound.
What such a call site does not get is restore on drop, so a test in
`amplihack-hooks`, `amplihack-types` or `amplihack-context` that needs the
variables put back must do that itself.

One binary had no lock to hang the purge on:
`crates/amplihack-utils/tests/launch_target_memo.rs` is its own target with a
single test in it, and the crate's `test_support` is `#[cfg(test)]` and so out
of its reach. It calls `purge_inherited()` directly, beside the `HOME` it
sets.

The helper reuses each binary's existing lock. It adds no second mutex.

## Write a test that sets HOME

Use `HomeGuard`. The five variables are cleared for the guard's lifetime and
restored when it drops, including when the test panics.

```rust
use crate::test_support::{HomeGuard, env_lock, tempdir_outside_framework_roots};

#[test]
fn install_writes_only_inside_the_temporary_home() {
    let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let home = tempdir_outside_framework_roots();
    let _home = HomeGuard::set(home.path());

    // CLAUDE_CONFIG_DIR is now unset, so this writes
    // <home>/.claude/settings.json.
    ensure_framework_installed_with(&mut Vec::new()).unwrap();

    assert!(home.path().join(".claude/settings.json").exists());
}
```

If a test sets one of the five variables itself, it sets it after taking the
`HomeGuard`. A variable set earlier is cleared by the guard.

### Fix an existing test that now fails

A test that set one of the five variables before taking a guard will see the
value cleared. Either move the `set_var` call after the guard, or, for
`CLAUDE_CONFIG_DIR`, use `with_config_dir` as described below.

## Point CLAUDE_CONFIG_DIR inside the temporary home

A test that needs `CLAUDE_CONFIG_DIR` set uses `with_config_dir`, a free
function in the helper module. The caller must hold `env_lock()` for the
lifetime of the returned guard, as for any environment guard.

```rust
use crate::test_support::claude_profile_isolation::with_config_dir;
use crate::test_support::{HomeGuard, env_lock, tempdir_outside_framework_roots};

let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
let home = tempdir_outside_framework_roots();
let _home = HomeGuard::set(home.path());
let config = home.path().join("custom-profile");
let _config = with_config_dir(home.path(), &config);
```

`with_config_dir` returns a `ClaudeProfileIsolation`. It sets
`CLAUDE_CONFIG_DIR` to the directory given, and the guard's `Drop` restores
the prior value of all five, including when the test panics.

It panics when the directory is not inside the home, before changing
anything. It canonicalises both paths, resolves the nearest existing
ancestor of a directory that does not exist yet, and compares path
components rather than string prefixes. A `..` segment, a symlink inside the
home that points outside it, and a sibling such as `/tmp/home2` against
`/tmp/h` are all rejected. The panic is the intended outcome, and a test can
assert it:

```rust
#[test]
#[should_panic(expected = "outside")]
fn config_dir_outside_the_home_is_rejected() {
    let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let scratch = tempdir_outside_framework_roots();
    let home = scratch.path().join("home");
    let outside = scratch.path().join("other");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let _config = with_config_dir(&home, &outside);
}
```

## Give a child process a clean environment

A spawned child inherits the test process's environment unless told
otherwise.

- Children built through `isolated_amplihack` (`tests/support/`) have the
  five variables removed, on top of `env_clear()` and the kept-name
  allowlist.
- For a `Command` of your own, call
  `claude_profile_isolation::remove_from_command(&mut cmd)`.
- For a child that reaches a settings writer, prefer `env_clear()` and an
  explicit allowlist (`PATH`, `HOME`, `TMPDIR`, `LANG`). A list of five
  names fails open when a sixth variable becomes relevant.
- `in_quiet_process` fires the purge inside each quiet child, before the
  test body runs, and `child_set_var` fires it again whenever the key is
  `HOME`. That is where changing the environment is sound in
  `cli_integration`, which has no `env_lock()`. A test whose subject is what
  a child inherits exports variables on purpose with
  `in_quiet_process_with_env`.

`in_quiet_process` does not call `env_clear()`, so credential variables such
as `ANTHROPIC_API_KEY` still reach quiet children. Do not print a child's
environment in failure output.

## Check that a profile stays unchanged

`issue_2922_inherited_config_dir` is the behaviour test. It runs in process
as a unit test under `crates/amplihack-cli/src/commands/install/tests/`:

1. Create a stand-in profile and a temporary home under
   `tempdir_outside_framework_roots()`, with synthetic content only.
2. Set `CLAUDE_CONFIG_DIR` to the stand-in, as an agent launch would, before
   the home guard is taken.
3. Take the home guard and run `ensure_framework_installed_with`, which
   reaches both `repair_settings_hooks` and `refresh_deny_entries`. The run
   repeats under `with_provider("codex", ..)`.
4. Compare a recursive snapshot of the stand-in before and after. The
   snapshot holds file bytes and the set of paths, so a newly created
   `settings.json` is caught as well as an edited one.
5. Assert that the temporary home's own `settings.json` was written and
   holds none of the stand-in's content, so the test cannot pass by
   returning early.

Snapshot failures report changed paths only, never file contents.

`issue_2922_helper_contract` pins the helper's own behaviour, and
`issue_2922_quiet_child_purge` checks that none of the five variables is
visible inside a quiet child, even when the parent exported them.

Run them with the profile variables unset, then again with them set to a
stand-in under `$TMPDIR`:

```bash
unset CLAUDE_CONFIG_DIR AMPLIHACK_CLAUDE_PROFILE
cargo test -p amplihack-cli issue_2922

standin="$(mktemp -d)"
printf '%s' '{"model":"stand-in"}' > "$standin/settings.json"
CLAUDE_CONFIG_DIR="$standin" AMPLIHACK_CLAUDE_PROFILE=standin \
  cargo test -p amplihack-cli issue_2922
cat "$standin/settings.json"   # unchanged
rm -rf "$standin"
```

Never point either variable at a real profile.

## Limits

- The fix is on the test side. The install code still follows an inherited
  `CLAUDE_CONFIG_DIR` in any process that is not a test.
- A test that builds a temporary home and never reaches `env_lock()`,
  `set_home`, `HomeGuard`, `EnvGuard` or `env_paths` is not covered.
  `scratch_base()` does not fire the purge.
- `remove_from_command` is a list of five names. A sixth relevant variable
  is not removed until it is added to `CLAUDE_PROFILE_ENV_VARS`.
- `AMPLIHACK_ROTATE_AUTOSTART`, `AMPLIHACK_TEST_HOME_GUARD`,
  `AMPLIHACK_CLAUDE_BINARY_PATH`, `XDG_CONFIG_HOME` and any Codex equivalent
  are not cleared. They are tracked as follow-up issues, as is a stderr
  notice from `launch_settings_target` when the settings target lies outside
  the `HOME` the process was given.

See [Keep Tests out of the Real Profile Root](keep-tests-out-of-the-real-profile-root.md)
for the profile-root guard and
[Keep Tests from Changing PATH and HOME](keep-tests-from-changing-path-and-home.md)
for child-process `HOME` handling.
