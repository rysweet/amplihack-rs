# Recipe Executor Environment — Reference

Complete reference for the environment variables, prerequisite checks, and context propagation performed by the recipe executor before and during step execution.

## Contents

- [Shell step environment injection](#shell-step-environment-injection)
- [Agent step context augmentation](#agent-step-context-augmentation)
- [Prerequisite validation](#prerequisite-validation)
- [Interaction with AMPLIHACK_NONINTERACTIVE](#interaction-with-amplihack_noninteractive)

---

## Shell step environment injection

Every shell step executed by `amplihack recipe run` receives the following environment variables, regardless of what the parent process environment contains.

| Variable | Value | Source |
|----------|-------|--------|
| `HOME` | `$HOME` from parent, or `/root` if unset | `std::env::var("HOME")` with fallback |
| `PATH` | `$PATH` from parent, or `/usr/local/bin:/usr/bin:/bin` if unset | `std::env::var("PATH")` with fallback |
| `NONINTERACTIVE` | `1` | Hardcoded |
| `DEBIAN_FRONTEND` | `noninteractive` | Hardcoded |
| `CI` | `true` | Hardcoded |

**Behavior:**

- Variables are set via `std::process::Command::env()`, which adds to (not replaces) the inherited environment.
- `HOME` and `PATH` preserve the parent value when available. The fallback values prevent failures in minimal containers where these may be unset.
- `NONINTERACTIVE`, `DEBIAN_FRONTEND`, and `CI` are always set to their non-interactive values. There is no way to override them for individual steps.

**Source:** `crates/amplihack-recipe/src/executor.rs`, `execute_shell_step()`.

---

## Agent step context augmentation

Every agent step receives an augmented context map before the agent backend is invoked. Two entries are added if not already present:

| Context key | Value | Source |
|-------------|-------|--------|
| `working_directory` | `self.config.working_dir` | Recipe executor configuration |
| `NONINTERACTIVE` | `1` | Hardcoded |

**Behavior:**

- Uses `entry().or_insert_with()` semantics: if the recipe YAML already defines `working_directory` or `NONINTERACTIVE` in the step's context, those values take precedence.
- The augmented context is passed to `self.agent_backend.run_agent()`.
- The `working_directory` value tells the agent where to locate and write files. Without it, agents may operate in an unexpected directory.

**Source:** `crates/amplihack-recipe/src/executor.rs`, `execute_agent_step()`.

---

## Prerequisite validation

Before executing a shell step, the executor checks whether referenced tools are available.

### Python check

**Trigger:** The expanded shell command contains the substring `python3` or `python ` (with trailing space).

**Check:** Runs `python3 --version` with stdout/stderr suppressed. If the command fails (exit non-zero or binary not found), the step is aborted immediately.

**Error message:**

```
Shell step '<step-id>' requires python3 but it is not installed or not on PATH.
Recipe steps should use deterministic Rust tools instead of Python sidecars.
```

**Design rationale:** Recipes can run for hours. A Python dependency missing at step N means N-1 steps ran for nothing. The pre-flight check fails the step in under 1 second instead of after hours of wasted work.

**Limitations:**

- Only `python3` and `python ` are checked. Other interpreters (`ruby`, `node`, etc.) are not validated.
- The check runs before variable expansion if the raw command text already contains the substring. If a `$RECIPE_VAR_*` variable introduces a Python reference after expansion, the check still applies because it runs on the expanded command.

**Source:** `crates/amplihack-recipe/src/executor.rs`, `execute_shell_step()`.

---

## Interaction with AMPLIHACK_NONINTERACTIVE

The recipe executor's environment injection is independent of the top-level `AMPLIHACK_NONINTERACTIVE` variable described in [Environment Variables](./environment-variables.md#amplihack_noninteractive).

| Scope | Variable | Set by |
|-------|----------|--------|
| Top-level CLI launch | `AMPLIHACK_NONINTERACTIVE=1` | User or CI configuration |
| Recipe shell steps | `CI=true`, `NONINTERACTIVE=1`, `DEBIAN_FRONTEND=noninteractive` | Recipe executor (always) |
| Recipe agent steps | `NONINTERACTIVE=1` in context map | Recipe executor (always) |

The recipe executor always sets non-interactive flags for its child processes, even when the top-level CLI is running interactively. This is by design: recipe steps are automated and should never prompt for input.

---

## Related

- [amplihack recipe](./recipe-command.md) — Full CLI reference for the recipe subcommand
- [Environment Variables](./environment-variables.md) — All variables read or injected by amplihack
- [Recipe Execution Flow](../concepts/recipe-execution-flow.md) — Step execution architecture
- [Troubleshoot Recipe Execution](../howto/troubleshoot-recipe-execution.md) — Diagnosing common recipe failures
