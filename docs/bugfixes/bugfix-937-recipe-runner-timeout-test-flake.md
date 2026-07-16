# Bug Fix #937 — Load-robust wall-clock ceilings in recipe-runner timeout tests

> **Issue:** [#937](https://github.com/rysweet/amplihack-rs/issues/937)

---

## Summary

Two recipe-runner regression tests in
`crates/amplihack-cli/src/commands/recipe/run/tests_execute.rs` asserted tight
wall-clock elapsed ceilings. They passed reliably in isolation but flaked under
a saturated full-workspace `cargo nextest run`, where CPU contention inflated
sub-second operations past the ceilings:

| Test | Old ceiling | Symptom under load |
| --- | --- | --- |
| `test_execute_recipe_via_rust_times_out_hung_runner` | `elapsed < 3s` | Timeout fired correctly, but process-spawn + tree-kill + pump-teardown overhead pushed elapsed past 3s |
| `test_execute_recipe_via_rust_verbose_survives_non_utf8_stderr` | `elapsed < 5s` | Healthy sub-second run inflated past 5s by scheduler latency |

The fix relaxes both ceilings to `25s` (well under the 30s per-test harness
timeout) **without weakening the tests' intent**. The primary correctness
assertions are unchanged; the wall-clock bounds now act purely as hang
detectors that a genuine failure still trips, while machine load cannot.

This is a **test-only** change. No production timeout, process-tree-kill, or
stderr-pump logic was modified.

## Behavior after the fix

### 1. Hung-runner timeout test still proves the parent timeout fired

`test_execute_recipe_via_rust_times_out_hung_runner` installs a stub
recipe-runner that `sleep 5`s and sets the parent timeout to 1 second via
`AMPLIHACK_RECIPE_RUNNER_TIMEOUT_SECS=1`.

The **intent-bearing assertion is unchanged**: the call must return an error
whose message contains `"timed out"`. That assertion alone proves the parent
1s timeout fired and killed the runner before its natural 5s completion.

```rust
let err = result.expect_err("hung recipe-runner must time out");
let msg = format!("{err:#}");
assert!(msg.contains("timed out"), "error should report timeout: {msg}");
```

The wall-clock ceiling is now a loose hang detector:

```rust
assert!(
    elapsed < std::time::Duration::from_secs(25),
    "parent timeout should bound hung runner well under the harness limit, \
     elapsed {elapsed:?}"
);
```

**Why 25s and not keyed to the runner's own 5s sleep:** process spawn, timeout
polling, the tree-kill, and stderr-pump teardown can add several seconds of
overhead under a saturated nextest run. That cleanup overhead is comparable to
the gap between an early timeout (1s) and natural completion (5s), so a tight
ceiling flakes. The 25s ceiling still catches the pathological case where the
timeout path fails to kill the runner and the child runs until the 30s test
harness kills it.

### 2. Non-UTF-8 stderr test still detects a pipe-hang

`test_execute_recipe_via_rust_verbose_survives_non_utf8_stderr` runs a runner
that emits non-UTF-8 bytes on stderr in verbose mode.

The **intent-bearing assertion is unchanged**: the call must return `Ok`,
proving the stderr pump neither aborts on invalid UTF-8 nor hangs the child.

```rust
result.expect("non-UTF-8 stderr must NOT abort the pump or hang the child");
```

The wall-clock ceiling is now a loose hang detector:

```rust
assert!(
    elapsed < std::time::Duration::from_secs(25),
    "non-UTF-8 stderr caused suspiciously slow run ({elapsed:?}) — \
     pump likely died and child blocked on full stderr pipe"
);
```

A healthy run is sub-second. The meaningful failure is a *pipe-hang*: if the
pump dies, the child's ~64KB stderr pipe fills and the child blocks until the
30s harness kills the test. The 25s ceiling catches that hang while tolerating
CPU contention on a saturated run.

## What did NOT change

- `test_execute_recipe_via_rust_timeout_kills_runner_process_tree` and all
  production timeout / process-tree-kill / stderr-pump logic are untouched.
- The `AMPLIHACK_RECIPE_RUNNER_TIMEOUT_SECS=1` parent timeout and the
  `RECIPE_RUNNER_RS_PATH` stub-runner override in the hung-runner test are
  retained; only the wall-clock ceilings changed.
- Neither ceiling assertion was deleted. They remain the only regression guard
  against the timeout path silently breaking, so future fixes must keep them.

## Trade-offs

A `25s` ceiling is a weaker regression guard than the previous `3s`/`5s`
bounds. This is acceptable because:

- The primary correctness assertions (`"timed out"` error, `Ok` result) are
  the real intent and remain strict.
- `25s < 30s` (the per-test harness timeout), so a genuine hang is still caught
  before the harness force-kills the test.
- The previous tight ceilings produced load-dependent false failures, which
  eroded trust in the suite more than the loosened ceiling weakens coverage.

## Verification

```bash
# Both tests in isolation
cargo nextest run -p amplihack-cli \
  test_execute_recipe_via_rust_times_out_hung_runner \
  test_execute_recipe_via_rust_verbose_survives_non_utf8_stderr

# Under a saturated full-workspace run (repeat to stress)
cargo nextest run

# Formatting and lints for the touched crate
cargo fmt -p amplihack-cli
cargo clippy -p amplihack-cli --tests
```

> **Note:** Pre-existing `issue_538_install_completeness` failures (no prebuilt
> `amplihack` binary available in the test environment) are unrelated to this
> change and reproduce on the base branch.
