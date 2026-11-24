---
title: Run recipe::run Tests Inside a Recipe Session
type: howto
updated: 2026-11-09
related: issue #1984, issue #2033
---

# Run recipe::run Tests Inside a Recipe Session

The `recipe::run` unit tests in `amplihack-cli` start a fake recipe runner as
a child process. When the tests themselves run inside a recipe run (a QA step,
a pre-push hook started by a recipe, or an agent's step), the outer run's
`AMPLIHACK_*` variables used to reach that child. The child then joined the
outer run's session tree and run directory instead of its own (#1984).

Which tests fail because of the leak is still to be confirmed by reproducing
#1984. `test_execute_recipe_via_rust_times_out_hung_runner` is the expected
one; this guide will name it as a confirmed case only once the reproduction
shows it. If the reproduction shows the stall comes from a variable that is
not in the list below, the list and this guide change with it.

The tests now remove those variables before they start a child. This guide
shows how to run the tests from inside a recipe session and how to write a new
test that starts a runner.

## Contents

- [Run the tests from a recipe session](#run-the-tests-from-a-recipe-session)
- [Which variables are removed](#which-variables-are-removed)
- [Write a test that starts a runner](#write-a-test-that-starts-a-runner)
- [The regression test](#the-regression-test)
- [Shell steps](#shell-steps)
- [Troubleshooting](#troubleshooting)

## Run the tests from a recipe session

Nothing extra is needed. The same command passes in a clean shell and inside a
recipe run:

```bash
cargo test -p amplihack-cli recipe::run
```

Inside an auto-drive round, `autodrive_qa_evidence.sh` and the merge-round
steps also clear the git and run-identity variables before any test runs, so
the suite starts with neither (see the
[Git Repository Environment Helper](../reference/git-repo-env-helper.md)).
The Rust tests do not rely on that: they clear their own environment, so they
also pass when started by hand from an agent's shell inside a run.

## Which variables are removed

`crates/amplihack-cli/src/test_support.rs` defines the list once:

```rust
pub(crate) const OUTER_RUN_IDENTITY_VARS: [&str; 7] = [
    "AMPLIHACK_MAX_DEPTH",
    "AMPLIHACK_RECIPE_RUN_ID",
    "AMPLIHACK_RUN_DIR",
    "AMPLIHACK_RUN_ID",
    "AMPLIHACK_SESSION_DEPTH",
    "AMPLIHACK_SESSION_TREE_DIR",
    "AMPLIHACK_TREE_ID",
];
```

It is the shell helper's five run-identity names plus the two depth
variables. The tests clear the depth variables because each test starts a
fresh, top-level run. The shell helper keeps them, because there they feed the
recursion guard of a real run.

These places use the constant:

| Place | Use |
| --- | --- |
| `clear_outer_run_env()` in `test_support.rs` | Removes the 7 names; restores them when the returned guard drops. |
| `fixture_environment()` in `tests_execute.rs` and `tests_teardown.rs` | Captures and removes the 7 names with the fixture's other variables. |
| `tests_execute.rs`, `tests_context.rs`, `tests_root_sandbox.rs` | Replace their hand-written five-name lists with `clear_outer_run_env()`. A site whose assertions need `AMPLIHACK_RUN_DIR` or `AMPLIHACK_RUN_ID` to pass through keeps its old five-name `EnvGuard::unset`. |
| `execute_recipe_via_rust_for_test` in `run/mod.rs` | Removes the 7 names when `is_outer_orchestration` is true. |

Production code in `execute.rs` is unchanged: a real nested run still
inherits its parent's run identity.

## Write a test that starts a runner

1. Take `env_lock()` first. `clear_outer_run_env()` changes the process
   environment, which other tests share.
2. Call `clear_outer_run_env()` before your own `EnvGuard::set`, so your
   values are not removed.
3. Keep both guards alive until the child has exited. They restore the old
   values when dropped, including on panic.

```rust
use crate::test_support::{clear_outer_run_env, env_lock, EnvGuard};

#[test]
fn test_execute_recipe_via_rust_uses_stub_runner() {
    let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let _outer = clear_outer_run_env();
    let dir = tempfile::tempdir().expect("tempdir");
    let runner = write_stub_runner(dir.path()); // the stub-runner pattern in tests_execute.rs
    let _env = EnvGuard::set([("RECIPE_RUNNER_RS_PATH", runner.to_str().unwrap())]);

    // ... start the run and assert on its result ...
}
```

Do not write a new list of `AMPLIHACK_*` names in a test. Add the name to
`OUTER_RUN_IDENTITY_VARS` instead, so every test picks it up.

## The regression test

`test_execute_recipe_via_rust_child_sees_no_outer_run_identity` in
`tests_execute.rs` guards #1984:

1. It takes `env_lock()` and sets all 7 variables to values a real outer run
   would have:
   - `AMPLIHACK_SESSION_DEPTH` and `AMPLIHACK_MAX_DEPTH` to the same number
     (for example both `3`). A child that inherits them is already at the
     depth limit and refuses to start.
   - `AMPLIHACK_RUN_DIR` and `AMPLIHACK_SESSION_TREE_DIR` to real directories
     inside the test's own temporary directory.
   - `AMPLIHACK_RUN_ID`, `AMPLIHACK_RECIPE_RUN_ID` and `AMPLIHACK_TREE_ID` to
     IDs in the same format the runner generates.
2. It calls `clear_outer_run_env()` and starts a stub runner that prints the 7
   variables as one JSON object.
3. It asserts that the child started and exited 0, that its run directory,
   run IDs and tree values are not the outer ones, and that its depth is the
   one a fresh top-level run gets.

Why these values: `execute.rs` always sets `AMPLIHACK_SESSION_DEPTH` and
`AMPLIHACK_MAX_DEPTH` on the child, and gives it new tree and run-ID values.
With placeholder strings such as `outer-…`, "no printed value equals an outer
value" would pass even without the fix. Non-numeric depth values also send the
child through the #1326 "nothing corroborates it" path, which is not the #1984
path. Numeric depth at the limit and a real outer run directory make the test
fail on the code before the fix.

The test must be shown to fail on the code before the fix. Run it against
that code once and record the failure output in the pull request, the same way
as the mutation check for the drift guard.

The test reads no clock and waits only for the child to exit. Run it alone:

```bash
cargo test -p amplihack-cli child_sees_no_outer_run_identity
# Output includes: test ...::test_execute_recipe_via_rust_child_sees_no_outer_run_identity ... ok
```

## Shell steps

QA and test steps in recipes clear the same leak with the shell helper:

```bash
. "$AMPLIHACK_HOME/amplifier-bundle/tools/git-repo-env.sh"
amplihack_clear_git_repo_env
amplihack_clear_run_identity_env
cargo test -p amplihack-cli recipe::run
```

`amplihack_clear_git_repo_env` also removes `GIT_DIR`, `GIT_WORK_TREE` and the
other repository selectors, so the tests act on their own checkout. The full
list and the evidence fields are in the
[helper reference](../reference/git-repo-env-helper.md).

## Troubleshooting

**A test fails only inside a recipe run.** Print the environment the child
sees: `env | grep -E '^(AMPLIHACK|GIT)_' | cut -d= -f1`. Any `AMPLIHACK_`
name that names a run and is not in `OUTER_RUN_IDENTITY_VARS` belongs there;
add it to the constant (and, if a real QA step must clear it too, to the shell
helper's run-identity list).

**A test now misses `AMPLIHACK_RUN_DIR` it used to read.** The test was
reading the outer run's value. Set the value the test needs with
`EnvGuard::set` after `clear_outer_run_env()`.

**A test hangs or runs too long.** Do not raise its time limit. The 25-second
limit and the "ran too long" assertion in the runner tests stay as they are;
find which inherited variable or shared state the child is waiting on and
isolate it.

## Related

- [Git Repository Environment Helper](../reference/git-repo-env-helper.md) - the shell lists, callers and drift guard
- [Keep Tests from Changing PATH and HOME](keep-tests-from-changing-path-and-home.md) - giving a child its own environment
- [Keep Tests Passing Under Host Load](keep-tests-passing-under-host-load.md) - fixing load failures with isolation, not longer windows
