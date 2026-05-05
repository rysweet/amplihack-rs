# Subprocess-Level Env Isolation for Integration Tests

## Overview

Integration tests that exercise env-var-driven behaviour (such as
`AMPLIHACK_AGENT_BINARY`) must never use `std::env::set_var` /
`std::env::remove_var` inside the test process.  Cargo runs tests in parallel
by default and those calls mutate a single global table, causing races that
produce non-deterministic results.

The canonical pattern in this repo replaces in-process env mutation with
**subprocess-level env isolation**: the parent test spawns a copy of the
current test binary as a child probe with the exact env state it needs,
reads stdout, and asserts on the printed result.

---

## Pattern: `run_probe` + child probe tests

### How it works

1. **Child probe tests** are ordinary `#[test]` functions whose names start
   with `probe_`.  Each probe calls the function under test and `println!`s the
   result — nothing else.

2. **`run_probe(test_name, env_override)`** is a private helper that:
   - Resolves the path to the running test binary with `std::env::current_exe()`.
   - Spawns it with `--exact <test_name> --nocapture` so only that one probe
     runs in the child process.
   - Always calls `cmd.env_remove("AMPLIHACK_AGENT_BINARY")` first, then
     optionally calls `cmd.env("AMPLIHACK_AGENT_BINARY", val)` for the override
     case, ensuring clean isolation regardless of the parent process's env.
   - Collects `stdout` and returns the first non-harness line (the printed
     value).

3. **Contract tests** call `run_probe` with the appropriate env and assert on
   the returned string.

```
Contract test (parent)
  └─ run_probe("probe_default_no_env", None)
       └─ spawns: test-binary --exact probe_default_no_env --nocapture
            └─ AMPLIHACK_AGENT_BINARY removed from child env
            └─ child calls active_agent_binary(), prints result
       └─ parent reads stdout, asserts == "copilot"
```

### Reference implementation

```rust
use std::process::Command;

/// Spawn the current test binary to run a single child probe test with
/// full env isolation.  `env_override` sets AMPLIHACK_AGENT_BINARY when
/// `Some`; `None` removes it entirely.
fn run_probe(test_name: &str, env_override: Option<&str>) -> String {
    let exe = std::env::current_exe().expect("could not resolve current test exe");
    let mut cmd = Command::new(&exe);
    cmd.args(["--exact", test_name, "--nocapture"]);
    cmd.env_remove("AMPLIHACK_AGENT_BINARY");
    if let Some(val) = env_override {
        cmd.env("AMPLIHACK_AGENT_BINARY", val);
    }
    let output = cmd.output().expect("failed to spawn child probe");
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find(|l| {
            let t = l.trim();
            !t.is_empty()
                && !t.starts_with("running")
                && !t.starts_with("test ")
                && !t.starts_with("test result")
        })
        .unwrap_or("")
        .trim()
        .to_string()
}

// Child probes ──────────────────────────────────────────────────────────────

#[test]
fn probe_default_no_env() {
    println!("{}", active_agent_binary());
}

#[test]
fn probe_claude_override() {
    println!("{}", active_agent_binary());
}

#[test]
fn probe_invalid_override() {
    println!("{}", active_agent_binary());
}

// Contract tests ────────────────────────────────────────────────────────────

#[test]
fn default_is_copilot_not_claude() {
    assert_eq!(run_probe("probe_default_no_env", None), "copilot");
}

#[test]
fn explicit_claude_override_still_works() {
    assert_eq!(run_probe("probe_claude_override", Some("claude")), "claude");
}

#[test]
fn rejected_override_falls_back_to_copilot() {
    assert_eq!(run_probe("probe_invalid_override", Some("not-a-real-binary")), "copilot");
}
```

---

## `active_agent_binary` contract

`amplihack_cli::env_builder::helpers::active_agent_binary()` implements the
following resolution precedence and is the subject under test:

| Priority | Source | Notes |
|----------|--------|-------|
| 1 | `AMPLIHACK_AGENT_BINARY` env var | Allowlist-validated; invalid values are silently discarded |
| 2 | `<repo>/.claude/runtime/launcher_context.json` `launcher` field | Present in nested agent sessions |
| 3 | Built-in default | Always `"copilot"` |

### Allowlisted binary names

The following values are accepted; any other value is treated as absent and
falls through to the next priority level:

- `amplifier`
- `claude`
- `codex`
- `copilot`

### Contracts verified by the test suite

| Test | Env state | Expected result |
|------|-----------|-----------------|
| `default_is_copilot_not_claude` | `AMPLIHACK_AGENT_BINARY` unset | `"copilot"` |
| `explicit_claude_override_still_works` | `AMPLIHACK_AGENT_BINARY=claude` | `"claude"` |
| `rejected_override_falls_back_to_copilot` | `AMPLIHACK_AGENT_BINARY=not-a-real-binary` | `"copilot"` |

---

## Running the tests

```bash
cargo test -p amplihack-cli --test active_agent_binary_default_test --locked
```

All six tests (three probes + three contract tests) must pass.  The probes are
also runnable individually for debugging:

```bash
# Run a single probe in isolation with an explicit env:
AMPLIHACK_AGENT_BINARY=claude \
  cargo test -p amplihack-cli --test active_agent_binary_default_test \
  -- probe_claude_override --exact --nocapture
```

---

## When to use this pattern

Use subprocess-level env isolation whenever:

- The function under test reads env vars at call time (not at startup).
- The test suite runs in parallel (the default).
- Correctness depends on the *absence* of a variable, not just its value.

Do **not** use `std::env::set_var` / `std::env::remove_var` in integration
tests that may run concurrently with other tests.  Even a `Mutex` guard only
prevents races within the same process; it cannot prevent the OS from inheriting
an unintended value from a simultaneously-running parallel test.

---

## Anti-patterns (do not use)

```rust
// ❌ WRONG — races under --test-threads > 1
static ENV_MUTEX: Mutex<()> = Mutex::new(());

#[test]
fn some_test() {
    let _guard = ENV_MUTEX.lock().unwrap();
    std::env::set_var("AMPLIHACK_AGENT_BINARY", "claude");
    let result = active_agent_binary();
    std::env::remove_var("AMPLIHACK_AGENT_BINARY");
    assert_eq!(result, "claude");
}
```

```rust
// ❌ WRONG — remove_var in one test racing with set_var in another
#[test]
fn clear_and_test() {
    std::env::remove_var("AMPLIHACK_AGENT_BINARY");
    assert_eq!(active_agent_binary(), "copilot"); // may observe another test's value
}
```
