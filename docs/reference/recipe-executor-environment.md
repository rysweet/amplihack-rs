# Recipe Executor Environment — Reference

Complete reference for the environment variables, prerequisite checks, and context propagation performed by the recipe executor before and during step execution.

## Contents

- [Recipe runner subprocess launch](#recipe-runner-subprocess-launch)
- [Recipe context environment export](#recipe-context-environment-export)
- [Shell step environment injection](#shell-step-environment-injection)
- [Agent step context augmentation](#agent-step-context-augmentation)
- [Prerequisite validation](#prerequisite-validation)
- [Interaction with AMPLIHACK_NONINTERACTIVE](#interaction-with-amplihack_noninteractive)

---

## Recipe runner subprocess launch

`amplihack recipe run` launches `recipe-runner-rs` through the centralized
Rust subprocess environment builder. The child process is prepared for
non-interactive nested execution before the runner receives any recipe context.

| Variable | Child value | Behavior |
|----------|-------------|----------|
| `AMPLIHACK_NONINTERACTIVE` | `1` | Always forced for the recipe-runner subprocess, even when the parent shell is interactive |
| `AMPLIHACK_RECIPE_RUN_ID` | Generated UUID | Stable correlation identity for this `amplihack recipe run` invocation |
| `CLAUDECODE` | *(removed)* | Explicitly unset so nested agents do not detect a parent Claude Code session |
| `AMPLIHACK_AGENT_BINARY` | Active launcher binary | Propagates the current runtime selection, such as `copilot`, `claude`, `codex`, or `amplifier` |
| `AMPLIHACK_HOME` | Resolved framework home | Gives the runner access to installed recipes, hooks, skills, and agents |
| `AMPLIHACK_ASSET_RESOLVER` | Resolved native resolver path, when available | Lets nested tools resolve bundled assets without hardcoded paths |
| `AMPLIHACK_GRAPH_DB_PATH` | Project-local graph DB path | Keeps launched hooks and memory/code-graph features on the same project store |
| `PAGER` / pager-related defaults | Pager-safe values | Prevents child commands from blocking on interactive pagers |

This contract applies to the runner subprocess itself. Shell and agent steps
inside the recipe receive the additional step-level variables described below.

### Example

```bash
# Parent has Claude Code session state and no explicit non-interactive flag.
CLAUDECODE=1 env -u AMPLIHACK_NONINTERACTIVE \
  amplihack recipe run default-workflow \
  -c task_description="Summarize the current repository" \
  -c repo_path=.
```

The `recipe-runner-rs` child sees `AMPLIHACK_NONINTERACTIVE=1` and
`AMPLIHACK_RECIPE_RUN_ID=<uuid>`, and it does not see `CLAUDECODE`. Nested agent
launches therefore use the active agent routing contract instead of inheriting
Claude-specific session behavior from the parent environment.

---

## Recipe context environment export

In addition to the fixed subprocess variables above, `amplihack recipe run`
exports **every recipe context variable** to the `recipe-runner-rs` subprocess
as an environment variable. This lets bash steps read context directly — for
example `$TASK_DESCRIPTION` and `$REPO_PATH` — instead of only through
`{{placeholder}}` substitution. Because the export is inherited by every nested
shell and sub-recipe step, the values work under `set -u` and deep inside
composed workflows.

| Aspect | Behavior |
|--------|----------|
| Source | The merged recipe context (`context` block + `-c/--context` flags + inferred values) |
| Name | Context key, ASCII-uppercased (`task_description` → `TASK_DESCRIPTION`) |
| Value | Context value, unchanged |
| Validity | Names must match `^[A-Z_][A-Z0-9_]*$`; invalid keys are skipped (name-only `WARN`) |
| Safety | Reserved/dangerous names (`PATH`, `LD_PRELOAD`, `BASH_ENV`, `IFS`, `AMPLIHACK_*`, …) are never exported |
| Precedence | Lowest — applied before the subprocess builder and `AMPLIHACK_RECIPE_RUN_ID`, so builder-managed and correlation variables always win |

### Example

```bash
amplihack recipe run env-aware-recipe \
  -c task_description="Add validation for empty display names" \
  -c repo_path=.
```

A bash step of `env-aware-recipe` can then run:

```bash
set -euo pipefail
echo "task: $TASK_DESCRIPTION"   # Add validation for empty display names
echo "repo: $REPO_PATH"          # .
```

The full contract — transform rules, the reserved-name denylist, the
`context_env_pairs` API, precedence guarantees, and the security model — is in
[Recipe Context Environment Export](./recipe-context-environment.md).

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
| Recipe runner subprocess | `AMPLIHACK_NONINTERACTIVE=1`, `AMPLIHACK_RECIPE_RUN_ID=<uuid>` | `amplihack recipe run` centralized subprocess environment |
| Recipe shell steps | `CI=true`, `NONINTERACTIVE=1`, `DEBIAN_FRONTEND=noninteractive` | Recipe executor (always) |
| Recipe agent steps | `NONINTERACTIVE=1` in context map | Recipe executor (always) |

The runner subprocess contract aligns the top-level recipe launch with the
existing step-level non-interactive behavior. Recipe execution is automated and
must never prompt for input.

---

## Related

- [amplihack recipe](./recipe-command.md) — Full CLI reference for the recipe subcommand
- [Recipe Context Environment Export](./recipe-context-environment.md) — Exporting recipe context variables to bash steps (uppercased, denylist, precedence)
- [Environment Variables](./environment-variables.md) — All variables read or injected by amplihack
- [Recipe Run Correlation Reference](./recipe-run-correlation.md) — Stable recipe run IDs and pointer events
- [Recipe Execution Flow](../concepts/recipe-execution-flow.md) — Step execution architecture
- [Troubleshoot Recipe Execution](../howto/troubleshoot-recipe-execution.md) — Diagnosing common recipe failures
