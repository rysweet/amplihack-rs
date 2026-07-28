# Bug Fix #1085 — Load-robust `bash -n` syntax-check timeout in launcher tests

> **Issue:** [#1085](https://github.com/rysweet/amplihack-rs/issues/1085)

---

## Summary

The `amplihack-cli` launcher regression tests in
`crates/amplihack-cli/src/commands/multitask/launcher_tests.rs` intermittently
failed under a saturated `cargo nextest run`. Every test that validated a
generated launcher script routed through the shared helper
`assert_script_is_lf_only_and_bash_valid`, which shells out to `bash -n`
(a no-op syntax check) via `run_output_with_timeout` with a hardcoded **2 s**
timeout:

```rust
let output = run_output_with_timeout(command, Duration::from_secs(2))
    .unwrap_or_else(|err| panic!("failed to run bash -n for {}: {err:#}", script.display()));
```

> **Note:** a prior fix (`bash_program()`) already hardened this helper against a
> *separate* flake — a `PATH`-resolution race when a concurrent test mutates the
> process-global `PATH` — by resolving `bash` to an absolute path. That fix left
> the **2 s** spawn-latency timeout untouched; this change addresses the
> remaining timeout-under-load flake and is complementary to it.

`bash -n` on a tiny launcher script completes in well under a millisecond in
isolation. Under a loaded CI runner, however, process spawn plus scheduler
latency for the child could occasionally exceed 2 s, tripping the timeout and
panicking the test — a pure resource-contention flake, not a real syntax
error. The following tests all shared the helper and therefore the flake:

| Test | Path through the flake |
| --- | --- |
| `executable_script_writer_normalizes_crlf_and_lone_cr_inputs` | Calls the helper twice (CRLF + lone-CR scripts) |
| `recipe_launcher_scripts_are_lf_only_and_bash_valid` | Calls the helper for `launcher.sh` and `run.sh` |
| `classic_launcher_script_is_lf_only_and_bash_valid` | Calls the helper for `run.sh` |

The fix replaces the magic `2 s` literal with a named constant,
`BASH_SYNTAX_CHECK_TIMEOUT = Duration::from_secs(30)`, a generous but still
**bounded** safety ceiling (>3000× a healthy `bash -n` run). Spawn latency
under contention cannot realistically approach 30 s, so the timeout no longer
fires spuriously; yet a genuinely hung `bash` is still killed and reaped rather
than hanging the suite.

This is a **test-only** change. No production launcher, script-generation, or
timeout logic was modified. Child-process reaping is unchanged and continues to
run through the production `run_output_with_timeout` path.

## Behavior after the fix

### Named, generous, bounded timeout

```rust
/// Upper bound for the `bash -n` syntax check on a generated launcher script.
///
/// A healthy `bash -n` on a tiny launcher completes in well under a
/// millisecond. This ceiling exists only to catch a genuinely hung `bash`
/// (and let `run_output_with_timeout` kill + reap it) — it must be generous
/// enough that process-spawn latency on a saturated CI runner can never trip
/// it, and bounded so a real hang cannot stall the suite.
const BASH_SYNTAX_CHECK_TIMEOUT: Duration = Duration::from_secs(30);

fn assert_script_is_lf_only_and_bash_valid(script: &Path) {
    let bytes = fs::read(script).unwrap();
    assert!(
        !bytes.contains(&b'\r'),
        "{} contains carriage returns and will fail under bash",
        script.display()
    );

    let mut command = Command::new(&bash_program());
    command.arg("-n").arg(script);
    let output = run_output_with_timeout(command, BASH_SYNTAX_CHECK_TIMEOUT)
        .unwrap_or_else(|err| panic!("failed to run bash -n for {}: {err:#}", script.display()));

    assert!(
        output.status.success(),
        "bash -n rejected {}:\nstdout:\n{}\nstderr:\n{}",
        script.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
```

**Intent-bearing assertions are unchanged.** The helper still enforces the two
things the tests exist to prove:

1. the generated script contains **no carriage returns** (`\r`), and
2. `bash -n` accepts the script's **syntax** (`output.status.success()`).

Only the timeout value changed — from an incidental `2 s` literal to a named
`30 s` ceiling whose role (hang detector, not performance assertion) is now
documented at the definition site.

### Child-process reaping is preserved

`run_output_with_timeout` (`crates/amplihack-cli/src/util.rs`) already reaps a
timed-out child via its `terminate_timed_out_child` path. Raising the ceiling
does not weaken that: if `bash` ever genuinely hangs, the child is still killed
and waited on, so no zombie process leaks and the suite cannot stall
indefinitely. The 30 s bound keeps the "never unbounded" guarantee intact.

## What did NOT change

- **Production code is untouched.** No launcher script generation
  (`write_executable_script`, `write_recipe_launcher`, `write_classic_launcher`)
  or `run_output_with_timeout` / `terminate_timed_out_child` logic was modified.
- **No new dependency.** `serial_test` was **not** added. Raising the timeout
  alone removes the flake; if a future, severely-overloaded environment still
  flaked, the fallback is to serialize via the existing in-repo `env_lock()`
  (`OnceLock<Mutex<()>>`) pattern — no new crate.
- **No assertion was deleted.** The CRLF check and the `bash -n` success check
  remain the regression guard for line-ending and syntax correctness.
- **No `print!`/`println!`** was introduced; the existing `panic!`/`assert!`
  diagnostics are retained verbatim.

## Trade-offs

A `30 s` ceiling is a looser hang detector than the previous `2 s` literal.
This is acceptable because:

- The real intent (no `\r`; `bash -n` succeeds) is asserted strictly and is
  unchanged.
- `bash -n` on these scripts is sub-millisecond, so a run approaching 30 s
  can only mean a genuine hang — which the timeout still catches and reaps.
- The previous tight literal produced load-dependent false failures that eroded
  trust in the suite far more than a generous-but-bounded ceiling does.

## Verification

```bash
# The launcher tests that route through the bash -n helper, in isolation.
cargo nextest run -p amplihack-cli \
  executable_script_writer_normalizes_crlf_and_lone_cr_inputs \
  recipe_launcher_scripts_are_lf_only_and_bash_valid \
  classic_launcher_script_is_lf_only_and_bash_valid

# Under a saturated full-workspace run, repeated to stress the old spawn race.
for i in $(seq 1 100); do \
  cargo nextest run -p amplihack-cli \
    executable_script_writer_normalizes_crlf_and_lone_cr_inputs \
    recipe_launcher_scripts_are_lf_only_and_bash_valid \
    classic_launcher_script_is_lf_only_and_bash_valid \
  || { echo "FLAKED on run $i"; break; }; done

# Formatting and lints for the touched crate.
cargo fmt -p amplihack-cli
cargo clippy -p amplihack-cli --tests
```

With a 30 s ceiling and sub-millisecond healthy runs, the 100× loop is expected
to pass every iteration regardless of machine load.
