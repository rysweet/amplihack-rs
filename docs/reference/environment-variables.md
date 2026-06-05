# Environment Variables — Reference

All environment variables read or written by `amplihack` during a launch (`amplihack claude`, `amplihack copilot`, `amplihack codex`, `amplihack amplifier`).

## Contents

- [Variables set by amplihack](#variables-set-by-amplihack)
  - [AMPLIHACK_AGENT_BINARY](#amplihack_agent_binary)
  - [AMPLIHACK_ASSET_RESOLVER](#amplihack_asset_resolver)
  - [AMPLIHACK_HOME](#amplihack_home)
  - [AMPLIHACK_GRAPH_DB_PATH](#amplihack_graph_db_path)
  - [AMPLIHACK_KUZU_DB_PATH](#amplihack_kuzu_db_path-backward-compatible-alias)
  - [AMPLIHACK_NONINTERACTIVE](#amplihack_noninteractive)
  - [AMPLIHACK_SESSION_ID](#amplihack_session_id)
  - [AMPLIHACK_DEPTH](#amplihack_depth)
  - [AMPLIHACK_RUST_RUNTIME](#amplihack_rust_runtime)
  - [AMPLIHACK_VERSION](#amplihack_version)
  - [AMPLIHACK_RELEASE_VERSION](#amplihack_release_version)
  - [NODE_OPTIONS](#node_options)
- [Variables injected by recipe executor](#variables-injected-by-recipe-executor)
  - [AMPLIHACK_STEP_TIMEOUT](#amplihack_step_timeout)
  - [AMPLIHACK_NONINTERACTIVE (recipe runner subprocess)](#amplihack_noninteractive-recipe-runner-subprocess)
  - [CLAUDECODE (recipe runner subprocess)](#claudecode-recipe-runner-subprocess)
  - [AMPLIHACK_RECIPE_HEARTBEAT_INTERVAL_SECONDS](#amplihack_recipe_heartbeat_interval_seconds)
  - [AMPLIHACK_RECIPE_SNIPPET_LINES](#amplihack_recipe_snippet_lines)
  - [AMPLIHACK_RECIPE_SNIPPET_BYTES](#amplihack_recipe_snippet_bytes)
  - [AMPLIHACK_RECIPE_LOG_JSONL](#amplihack_recipe_log_jsonl)
  - [NONINTERACTIVE](#noninteractive)
  - [DEBIAN_FRONTEND](#debian_frontend)
  - [CI (recipe context)](#ci-recipe-context)
- [Variables read by amplihack](#variables-read-by-amplihack)
  - [AMPLIHACK_MEMORY_BACKEND](#amplihack_memory_backend)
  - [HOME](#home)
  - [AMPLIHACK_DEFAULT_MODEL](#amplihack_default_model)
  - [AMPLIHACK_ENABLE_BLARIFY](#amplihack_enable_blarify)
  - [AMPLIHACK_BLARIFY_MODE](#amplihack_blarify_mode)
  - [AMPLIHACK_NO_UPDATE_CHECK](#amplihack_no_update_check)
  - [AMPLIHACK_PARITY_TEST](#amplihack_parity_test)
  - [AMPLIHACK_SKIP_AUTO_INSTALL](#amplihack_skip_auto_install)
  - [AMPLIHACK_TEST_FAKE_LATEST_VERSION](#amplihack_test_fake_latest_version)
  - [CI](#ci)
  - [UV_TOOL_BIN_DIR](#uv_tool_bin_dir)

---

## Variables set by amplihack

These variables are injected into every child process launched by `amplihack`. They are not inherited from the parent shell; they are built fresh on each invocation.

---

### AMPLIHACK_AGENT_BINARY

**Type:** string
**Allowed values:** `claude` | `copilot` | `codex` | `amplifier` (case-insensitive, exact match after trim)
**Default:** `copilot`
**Set by:** `EnvBuilder::with_agent_binary()` (as a back-compat read-through cache)
**Read by:** `amplihack_utils::agent_binary::resolve()` (precedence step 1)

Identifies which CLI binary the current session should use when spawning new AI sessions. As of the agent-binary-resolver refactor, this variable is **no longer the source of truth** — it is one of three precedence sources consulted by the shared resolver:

1. `AMPLIHACK_AGENT_BINARY` env var (explicit override; CI/testing/back-compat)
2. `<repo>/.claude/runtime/launcher_context.json` `launcher` field (canonical persisted state)
3. Built-in default: **`copilot`**

The launcher continues to write this variable to subprocess environments so that external consumers (notably `rysweet/amplihack-recipe-runner`) that have not yet migrated to the file-based resolver continue to work. New code inside `amplihack-rs` should call `amplihack_utils::agent_binary::resolve(&cwd)` instead of reading the env var directly.

#### Validation

Values are normalized (trim, lowercase) and matched against the allowlist `{claude, copilot, codex, amplifier}`. Values that contain `/`, `\`, `..`, null bytes, whitespace, control characters, or exceed 32 bytes are **rejected**. On rejection the resolver emits a structured `tracing::warn!` and falls through to the next precedence source.

```sh
# Start a Copilot session (the new default)
amplihack copilot

# Inside hooks, recipe steps, sub-agents:
echo $AMPLIHACK_AGENT_BINARY
# copilot

# Explicit override (CI, testing, manual selection)
AMPLIHACK_AGENT_BINARY=claude amplihack recipe run smart-orchestrator -c task_description="..."

# Invalid values are rejected and the resolver falls through
AMPLIHACK_AGENT_BINARY="../bin/evil" amplihack copilot
# warn: rejected AMPLIHACK_AGENT_BINARY (failed allowlist); falling back to launcher_context.json
```

#### Why the precedence order

- **Env var first** preserves the established escape hatch for CI/testing and lets external recipe-runner builds keep working unchanged.
- **`launcher_context.json` second** ensures that once a user runs `amplihack copilot` in a repo, every subsequent subprocess — even ones launched many hops away through `tmux`, sub-recipes, or detached hooks — picks up `copilot` without depending on env passthrough.
- **`copilot` default last** matches the project's current preferred runtime and removes the prior implicit `claude` assumption.

**Why it exists:** Recipe runner, hooks, and sub-agents are agent-agnostic and must call back into whatever tool the user actually launched. See [Active Agent Binary](./active-agent-binary.md) for the full algorithm and [Agent Binary Routing](../concepts/agent-binary-routing.md) for the architectural rationale.

**Python parity:** Python skill scripts (`amplifier-bundle/skills/pm-architect/scripts/agent_query.py`, `delegate_response.py`) implement the **same** three-step precedence and **same** allowlist; `agent_query.py::detect_runtime()` is the canonical Python entry point and is reused by `delegate_response.py`. The shell helper at `amplifier-bundle/skills/migrate/scripts/migrate.sh` re-implements the same algorithm with a `case` statement allowlist. The active binary is therefore consistent across Rust, Python, and shell code paths.

**Existing `claude` users:** repos that already have `.claude/runtime/launcher_context.json` with `"launcher": "claude"` continue to resolve to `claude` automatically — the file (precedence step 2) wins over the new `copilot` default. No migration action is required.

**Effect on startup self-update prompt:** A non-empty `AMPLIHACK_AGENT_BINARY` is also recognised by the startup self-update prompt as a subprocess-safe signal — when the variable is set, the prompt is skipped and the skip-line `amplihack: skipping update check (subprocess-safe / no TTY)` is emitted to stderr. This means delegated agent invocations never block on the prompt, even at an interactive TTY. See [Startup Self-Update Prompt — Subprocess-Safe Skip](../features/startup-update-prompt-subprocess-safe.md).

---

### AMPLIHACK_ASSET_RESOLVER

**Type:** path
**Example:** `/home/alice/.local/bin/amplihack-asset-resolver`
**Set by:** `EnvBuilder::with_asset_resolver()`

Absolute path to the native bundle-asset resolver. Child processes can execute this binary with a single relative asset path argument, for example:

```sh
"$AMPLIHACK_ASSET_RESOLVER" amplifier-bundle/recipes/smart-orchestrator.yaml
```

That returns the resolved absolute path on stdout and exits non-zero on invalid input or missing assets.

**Resolution order (first match wins):**

| Priority | Source |
|----------|--------|
| 1 | `AMPLIHACK_ASSET_RESOLVER` already set in environment |
| 2 | Sibling `amplihack-asset-resolver` next to the running `amplihack` executable |
| 3 | `PATH` lookup |
| 4 | `~/.local/bin/amplihack-asset-resolver` |
| 5 | `~/.cargo/bin/amplihack-asset-resolver` |

If no binary is found, the variable is omitted. Callers that require native resolution should treat absence as a hard setup problem rather than silently degrading.

**Why it exists:** Python's `resolve_bundle_asset.py` was a hidden runtime dependency for recipes and helper scripts. Exposing a dedicated Rust binary makes asset lookup explicit, testable, and reusable by child tools without embedding Python-specific paths.

---

### AMPLIHACK_HOME

**Type:** path
**Example:** `/home/alice/.amplihack`
**Set by:** `EnvBuilder::with_amplihack_home()`

The root directory where amplihack stores framework assets, hooks, runtime state, and helper scripts. Recipe runner uses this to locate `.claude/tools/amplihack/` and related subdirectories without requiring hardcoded paths.

**Resolution order (first match wins):**

| Priority | Source | Example result |
|----------|--------|----------------|
| 1 | `AMPLIHACK_HOME` already set in environment | value is passed through unchanged |
| 2 | `$HOME/.amplihack` | `/home/alice/.amplihack` |
| 3 | Directory containing the `amplihack` binary | `/usr/local/bin/../amplihack` |
| — | All above fail | variable is not set (silent degradation) |

```sh
# Override for a non-standard install location
export AMPLIHACK_HOME=/opt/amplihack
amplihack claude

# Verify the value a subprocess receives
AMPLIHACK_HOME=/opt/amplihack amplihack claude --print-env 2>&1 | grep AMPLIHACK_HOME
# AMPLIHACK_HOME=/opt/amplihack
```

**Security note:** The resolved path is validated to be absolute and must not contain `..` path components. Paths that fail validation are silently dropped; a warning is emitted to the trace log.

**Python parity:** Corresponds to `AMPLIHACK_HOME` propagation in the Python launcher.

---

### AMPLIHACK_GRAPH_DB_PATH

**Type:** path
**Example:** `/work/repo/.amplihack/graph_db`
**Set by:** `EnvBuilder::with_project_graph_db()`
**Read by:** `commands::memory::resolve_memory_graph_db_path()`

Overrides the code-graph database path used by Rust memory operations in launched child
processes. `amplihack launch` and the Rust recipe runner set it to the
project-local `.amplihack/graph_db` so launched sessions, hooks, and native
code-graph features operate on the same live store.

If this variable is absent, `amplihack` sets it to the project-local
`.amplihack/graph_db` directory. The legacy `AMPLIHACK_KUZU_DB_PATH`
override is accepted as an alias and translated to `AMPLIHACK_GRAPH_DB_PATH`
in the child process environment.

```sh
# Effective child-process environment for a project rooted at /work/repo
AMPLIHACK_GRAPH_DB_PATH=/work/repo/.amplihack/graph_db amplihack claude
```

**Why it exists:** Launched sessions, hooks, and native code-graph features
must all operate on the same project-local DB. Setting this variable ensures
every subprocess resolves to the correct location without relying on
filesystem detection heuristics.

---

### AMPLIHACK_KUZU_DB_PATH (backward-compatible alias)

Legacy compatibility alias for `AMPLIHACK_GRAPH_DB_PATH`.

If `AMPLIHACK_GRAPH_DB_PATH` is unset, Rust still reads `AMPLIHACK_KUZU_DB_PATH`
and also exports it for older child-process consumers. When both are present,
`AMPLIHACK_GRAPH_DB_PATH` wins.

```sh
# Backward-compatible older configuration still works
AMPLIHACK_KUZU_DB_PATH=/work/repo/.amplihack/graph_db amplihack claude
```

The alias remains because the storage engine was originally named Kuzu (now
rebranded to LadybugDB), but new automation should prefer
`AMPLIHACK_GRAPH_DB_PATH` so the public surface stays backend-neutral.

---

### AMPLIHACK_NONINTERACTIVE

**Type:** flag
**Values:** `1` (non-interactive) — absence or any other value means interactive
**Read by:** `util::is_noninteractive()`
**Set by:** `EnvBuilder::set_if(is_noninteractive(), "AMPLIHACK_NONINTERACTIVE", "1")`

Signals that the process is running in a non-interactive environment. When set to `1`, `amplihack` skips all interactive prompts and framework bootstrap guidance, preventing hangs in CI pipelines, pipes, and sandboxed environments.

```sh
# Run without interactive prompts (e.g. in CI)
AMPLIHACK_NONINTERACTIVE=1 amplihack claude --print 'Fix the lint errors'

# Pipe use also triggers non-interactive mode automatically (no TTY on stdin)
echo 'Summarize this file' | amplihack claude --print -
```

**Detection logic:**

Non-interactive mode is active when **either** condition is true:

1. `AMPLIHACK_NONINTERACTIVE=1` is set in the environment
2. `stdin` is not a TTY (detected via `std::io::IsTerminal`)

Condition 2 covers pipe usage without requiring the caller to set the variable manually.

**Effect on bootstrap:** When non-interactive mode is detected, `prepare_launcher()` returns immediately without running `check_required_tools()` or `ensure_framework_installed()`. The assumption is that CI environments are pre-provisioned and that interactive guidance output would be noise.

**Effect on update check:** Non-interactive mode also suppresses the pre-launch npm update check. No `npm` subprocesses are spawned. This is equivalent to passing `--skip-update-check` on every invocation. See [Manage Tool Update Notifications](../howto/manage-tool-update-checks.md) for details.

**Effect on startup self-update prompt:** A non-empty `AMPLIHACK_NONINTERACTIVE` (any value, not only `"1"`) also skips the `amplihack` startup self-update prompt — the `Update now? [y/N] (5s timeout):` line is never printed and stdin is never read. A single skip-line `amplihack: skipping update check (subprocess-safe / no TTY)` is emitted to stderr. See [Startup Self-Update Prompt — Subprocess-Safe Skip](../features/startup-update-prompt-subprocess-safe.md).

**Propagation:** Once detected, `AMPLIHACK_NONINTERACTIVE=1` is written into the child process environment so that nested invocations (e.g. sub-agents spawned by hooks) also behave non-interactively.

**Cross-language contract:** Only the value `"1"` triggers non-interactive mode. The strings `"true"`, `"yes"`, `"on"`, and `"TRUE"` are **not** recognised — this matches the Python launcher's behaviour.

**Python parity:** Corresponds to `AMPLIHACK_NONINTERACTIVE` check in `amplihack/cli/launch.py` (Python PRs #3103, #3066).

---

### AMPLIHACK_SESSION_ID

**Type:** string
**Example:** `rs-1741872000-12345`
**Set by:** `EnvBuilder::with_amplihack_session_id()`

A correlation ID for the current session. Used in log output and by the nesting detector to identify recursive `amplihack` invocations. Reused unchanged if already set in the environment (i.e. a nested invocation inherits the session ID of its parent).

Format: `rs-<unix_seconds>-<pid>`

---

### AMPLIHACK_DEPTH

**Type:** integer string
**Default:** `1`
**Set by:** `EnvBuilder::with_amplihack_session_id()`

Nesting depth of the current invocation. The root invocation receives `1`. Nested sessions (amplihack launched from within a Claude Code hook) inherit the value from the environment unchanged; the Python launcher increments it, but the Rust launcher propagates it as-is to match Python's observed behaviour for initial launches.

---

### AMPLIHACK_RUST_RUNTIME

**Type:** flag
**Value:** always `1`
**Set by:** `EnvBuilder::with_amplihack_vars()`

Indicates the session was started by the Rust CLI rather than the Python launcher. Hooks and recipe scripts can use this to branch on runtime differences.

```sh
# In a hook script
if [ "$AMPLIHACK_RUST_RUNTIME" = "1" ]; then
  # Rust-specific code path
fi
```

---

### AMPLIHACK_VERSION

**Type:** semver string
**Example:** `0.3.1`
**Set by:** `EnvBuilder::with_amplihack_vars()`

The version of the `amplihack-cli` binary that launched the session. Release
builds use `AMPLIHACK_RELEASE_VERSION` when it was set at compile time; local
developer builds fall back to `CARGO_PKG_VERSION`.

---

### AMPLIHACK_RELEASE_VERSION

**Type:** semver string
**Example:** `0.9.78`
**Set by:** release build environment
**Read by:** Rust compile-time `option_env!("AMPLIHACK_RELEASE_VERSION")`

Build-time override for the version embedded in released binaries. The release
workflow sets this value while compiling so `amplihack --version`, doctor
output, plugin manifests, hook context loading, and the runtime
`AMPLIHACK_VERSION` child-process variable all report the release tag version.

Local builds normally leave this unset and use `CARGO_PKG_VERSION`.

```sh
AMPLIHACK_RELEASE_VERSION=0.9.78 cargo build --release --locked --bin amplihack
./target/release/amplihack --version
```

---

### NODE_OPTIONS

**Type:** space-separated Node.js CLI flags
**Set by:** launcher startup via `memory_config.rs`, then propagated by `EnvBuilder::with_amplihack_vars()`

`amplihack` now computes a smart `--max-old-space-size=<mb>` value at top-level launcher startup based on detected system RAM, persists the consent choice in `~/.amplihack/config`, and displays the active choice on launch. The resolved value is then propagated through `EnvBuilder`.

When startup does not supply an explicit value, `EnvBuilder` still falls back to `--max-old-space-size=32768`, and if ambient `NODE_OPTIONS` already contains `--max-old-space-size=` it is preserved rather than duplicated.

---

## Variables injected by recipe executor

These variables are read, set, or removed by the recipe execution path
(`amplihack recipe run`) while launching `recipe-runner-rs` and running recipe
steps. Heartbeat, snippet, and JSONL log variables are optional user
configuration. See [Recipe Executor Environment](./recipe-executor-environment.md)
and [Recipe Runner Logging](./recipe-runner-logging.md) for full details.

---

### AMPLIHACK_STEP_TIMEOUT

**Type:** string (unsigned integer as text)
**Values:** `"0"` (disable timeouts) | `"600"` (override to 600 seconds) | any non-negative integer
**Set by:** `amplihack recipe run --step-timeout <SECONDS>`

Overrides the `timeout_seconds` value defined in individual recipe steps. When set, every step in the recipe uses this value instead of its YAML-defined timeout. A value of `"0"` disables step timeouts entirely, allowing steps to run indefinitely.

This variable is only present in the child environment when the user passes `--step-timeout` to `amplihack recipe run`. When the flag is omitted, YAML-defined `timeout_seconds` values apply as-is (though the default-workflow agent steps no longer define `timeout_seconds`).

```sh
# Override all step timeouts to 10 minutes
amplihack recipe run recipe.yaml --step-timeout 600
# Child process sees: AMPLIHACK_STEP_TIMEOUT=600

# Disable all step timeouts
amplihack recipe run recipe.yaml --step-timeout 0
# Child process sees: AMPLIHACK_STEP_TIMEOUT=0

# No override — YAML timeouts apply (agent steps have none by default)
amplihack recipe run recipe.yaml
# AMPLIHACK_STEP_TIMEOUT is NOT set in child environment
```

**Why it exists:** The default-workflow recipes no longer define `timeout_seconds` on agent steps, so agent steps run to completion without artificial time limits. This variable provides an opt-in escape hatch for CI environments that need wall-clock budgets. The env var approach is forward-compatible — `recipe-runner-rs` can adopt it independently without CLI changes.

**Security note:** The value is always a `u64` rendered as a string. No shell metacharacters are possible. The CLI rejects non-numeric input at parse time via clap's type enforcement.

---

### AMPLIHACK_NONINTERACTIVE (recipe runner subprocess)

**Type:** flag
**Value:** `1`
**Set by:** `amplihack recipe run` centralized subprocess environment

The recipe runner launch writes `AMPLIHACK_NONINTERACTIVE=1` into the
`recipe-runner-rs` child environment. This is stronger than top-level launcher
propagation: the value is forced for recipe execution even when the parent
process is interactive and the parent environment does not contain
`AMPLIHACK_NONINTERACTIVE`.

This prevents nested recipe, hook, and agent subprocesses from asking for input
or showing interactive update prompts while an automated workflow is already in
progress.

```sh
# The recipe-runner child receives AMPLIHACK_NONINTERACTIVE=1.
env -u AMPLIHACK_NONINTERACTIVE \
  amplihack recipe run default-workflow \
  -c task_description="Update generated docs" \
  -c repo_path=.
```

---

### CLAUDECODE (recipe runner subprocess)

**Type:** removed environment variable
**Removed by:** `amplihack recipe run` centralized subprocess environment

The recipe runner launch explicitly removes `CLAUDECODE` from the
`recipe-runner-rs` child environment. The variable is Claude-Code-specific
session state, not a portable amplihack routing signal. Leaving it set in a
nested recipe can make downstream agents believe they are running inside the
original Claude Code host even when the active launcher is Copilot, Codex, or
Amplifier.

Use `AMPLIHACK_AGENT_BINARY` for runtime routing. It is validated, propagated,
and documented as the active agent selector.

```sh
# The parent value is ignored for the recipe-runner child.
CLAUDECODE=1 amplihack recipe run smart-orchestrator \
  -c task_description="Review the open PR" \
  -c repo_path=.
```

---

### AMPLIHACK_RECIPE_HEARTBEAT_INTERVAL_SECONDS

**Type:** string (unsigned integer as text)
**Values:** `"0"` (disable heartbeats) | `"60"` (default) | any non-negative integer
**Read by:** `recipe-runner-rs`

Controls how often long-running recipe, agent, subprocess, and nested-recipe
steps emit heartbeat lines to stderr. The interval is rate-limited per active
step. Short steps that complete before one interval do not emit heartbeats.

```sh
# Emit a heartbeat every 15 seconds while debugging a long-running agent step
AMPLIHACK_RECIPE_HEARTBEAT_INTERVAL_SECONDS=15 \
amplihack recipe run default-workflow \
  -c task_description="Debug hanging test generation"

# Disable heartbeat lines while preserving start/completion/failure progress
AMPLIHACK_RECIPE_HEARTBEAT_INTERVAL_SECONDS=0 \
amplihack recipe run default-workflow \
  -c task_description="Run with minimal progress"
```

Equivalent config key: `recipe.heartbeat_interval_seconds` in
`~/.amplihack/config`.

---

### AMPLIHACK_RECIPE_SNIPPET_LINES

**Type:** string (unsigned integer as text)
**Default:** `20`
**Read by:** `recipe-runner-rs`

Maximum number of recent lines retained per active child source and stream.
Older lines are dropped from the rolling buffer. This bound applies to snippets
printed in failure diagnostics, included in JSON results, and written to JSONL
logs.

```sh
AMPLIHACK_RECIPE_SNIPPET_LINES=60 \
amplihack recipe run default-workflow \
  -c task_description="Diagnose compiler failure" \
  --format json > result.json
```

Equivalent config key: `recipe.snippet_lines` in `~/.amplihack/config`.

---

### AMPLIHACK_RECIPE_SNIPPET_BYTES

**Type:** string (unsigned integer as text)
**Default:** `8192`
**Read by:** `recipe-runner-rs`

Maximum bytes retained per active child source and stream. This byte bound is
enforced together with `AMPLIHACK_RECIPE_SNIPPET_LINES`; whichever limit is hit
first controls truncation.

```sh
AMPLIHACK_RECIPE_SNIPPET_BYTES=32768 \
amplihack recipe run default-workflow \
  -c task_description="Diagnose noisy subprocess output" \
  --format json > result.json
```

Equivalent config key: `recipe.snippet_bytes` in `~/.amplihack/config`.

---

### AMPLIHACK_RECIPE_LOG_JSONL

**Type:** path
**Read by:** `recipe-runner-rs`

When set, writes structured JSONL recipe events to the specified file. Events
include step lifecycle transitions, heartbeats, output snippets, and failure
context. Human-readable progress still goes to stderr.

```sh
AMPLIHACK_RECIPE_LOG_JSONL=/tmp/default-workflow.jsonl \
amplihack recipe run default-workflow \
  -c task_description="Add retry budget metrics"

jq 'select(.type == "heartbeat")' /tmp/default-workflow.jsonl
```

Equivalent config key: `recipe.log_jsonl` in `~/.amplihack/config`.

---

### NONINTERACTIVE

**Type:** string
**Value:** `1`
**Set by:** Recipe executor, `execute_shell_step()`

Signals to general-purpose tools that they should not attempt interactive prompts. Not specific to any one tool — serves as a generic non-interactive flag.

---

### DEBIAN_FRONTEND

**Type:** string
**Value:** `noninteractive`
**Set by:** Recipe executor, `execute_shell_step()`

Suppresses interactive prompts from `dpkg` and `apt`. Standard Debian/Ubuntu convention for headless package management.

---

### CI (recipe context)

**Type:** string
**Value:** `true`
**Set by:** Recipe executor, `execute_shell_step()`

Signals CI-like behavior to npm, yarn, pip, and other tools that check this variable before prompting. Note: this is set by the recipe executor for all recipe steps, independent of whether the top-level process is actually running in a CI system.

---

## Variables read by amplihack

These variables influence `amplihack`'s behaviour but are not set by it.

---

### AMPLIHACK_MEMORY_BACKEND

**Type:** string
**Values:** `sqlite` | `graph-db` | `kuzu`
**Read by:** `resolve_memory_backend_preference()`, `resolve_transfer_backend_choice()`, `resolve_backend_with_autodetect()`

Selects the memory storage backend for all memory commands: `memory tree`,
`memory clean`, `memory export`, and `memory import`. When unset, the backend
is chosen by probing the filesystem in priority order:

1. `AMPLIHACK_GRAPH_DB_PATH` path exists on disk → `graph-db`
2. `~/.amplihack/memory_graph.db` exists → `graph-db`
3. Neither exists → `sqlite` (default for new installs)

```sh
# Permanently opt into SQLite (add to shell profile)
export AMPLIHACK_MEMORY_BACKEND=sqlite

# Single-invocation override
AMPLIHACK_MEMORY_BACKEND=graph-db amplihack memory tree

# Legacy kuzu alias still works (backward-compatible)
AMPLIHACK_MEMORY_BACKEND=kuzu amplihack memory tree
```

Values outside the allowlist `['sqlite', 'kuzu', 'graph-db']` produce a
visible warning to stderr and fall back to the graph-db backend. There is no
silent acceptance of unrecognised values.

**Why it exists:** New installs default to SQLite (no native library
required). Existing installs with a populated `memory_graph.db` continue to
use graph-db automatically. This variable lets users and CI pipelines opt in
or out of either backend without modifying on-disk state.

See [Memory Backend Reference](./memory-backend.md) for the complete
backend configuration reference.

---

### HOME

**Required:** yes (for most operations)

Standard Unix home directory. Used to resolve `~/.amplihack`, `~/.npm-global`, and shell profile paths.

---

### AMPLIHACK_DEFAULT_MODEL

**Type:** string
**Default:** `opus[1m]`
**Used by:** `build_command()` in `launch.rs`

The `--model` flag passed to the launched tool when the user has not specified one explicitly. Override to use a different model variant.

```sh
AMPLIHACK_DEFAULT_MODEL=sonnet amplihack claude
# Passes: claude --model sonnet --dangerously-skip-permissions
```

If the user supplies `--model` explicitly on the command line, this variable is
ignored entirely — the user-supplied value is used as-is.

```sh
# User-supplied --model takes priority; AMPLIHACK_DEFAULT_MODEL is ignored
AMPLIHACK_DEFAULT_MODEL=sonnet amplihack claude --model haiku
# Passes: claude --model haiku --dangerously-skip-permissions
```

See [Launch Flag Injection](./launch-flag-injection.md) for the complete rules
governing how `--model` and other flags are injected into the subprocess
command line.

---

### AMPLIHACK_COPILOT_NO_REMOTE

**Type:** flag
**Values:** `1` suppresses `--remote` injection; absence or any other value keeps the default
**Used by:** `should_inject_copilot_remote()` in `commands/launch/command.rs`

When `amplihack copilot` launches the Copilot CLI, it injects `--remote` by
default to offload compute to GitHub's cloud. Set this variable to `1` to
suppress the injection and run Copilot locally.

```sh
# Disable remote mode
AMPLIHACK_COPILOT_NO_REMOTE=1 amplihack copilot
# Copilot starts without --remote
```

Users can also pass `--no-remote` directly as an extra arg; the launcher
detects this and skips injection automatically.

---

### AMPLIHACK_ENABLE_BLARIFY

**Type:** flag
**Values:** `1` enables launcher-side code-indexing checks; absence or any other value disables them
**Read by:** `commands::launch::should_prompt_blarify_indexing()`

Opt-in gate for launcher-side code indexing. When set to `1` for `amplihack claude`, the Rust launcher checks whether code-graph artifacts are missing or stale, and whether the project-local `.amplihack/graph_db` store already exists, then either prompts or follows `AMPLIHACK_BLARIFY_MODE` if that mode is set.

Without `AMPLIHACK_BLARIFY_MODE`, interactive launches offer to either:

- import an existing fresh `.amplihack/blarify.json` via `amplihack index-code` when the code-graph DB is missing, or
- generate fresh native SCIP artifacts via `amplihack index-scip` when the existing artifact is stale or no fresh import input exists.

If the variable is unset, the launcher skips all code-indexing checks and proceeds directly to the target AI tool.

```sh
# Enable launcher-side code indexing prompts for Claude launches
AMPLIHACK_ENABLE_BLARIFY=1 amplihack claude

# Generate native SCIP artifacts manually instead of waiting for the prompt
amplihack index-scip --project-path .
```

**Artifact locations:**

- `.amplihack/blarify.json` — LadybugDB import input for `amplihack index-code`
- `.amplihack/indexes/<language>.scip` — per-language native SCIP artifacts from `amplihack index-scip`
- `.amplihack/graph_db` — native code-graph store populated by `index-code` or `index-scip`

**Why it exists:** Code-graph indexing is computationally expensive and should not run on every launch. This flag keeps the behavior opt-in so only projects that benefit from code-graph enrichment pay the cost.

---

### AMPLIHACK_BLARIFY_MODE

**Type:** string
**Values:** `skip` | `sync` | `background`
**Read by:** `commands::launch::blarify_mode()`

Controls how launcher-side code indexing behaves once `AMPLIHACK_ENABLE_BLARIFY=1` has opted the project in.

- `skip` — suppress indexing work for this launch
- `sync` — run indexing in the foreground before launching Claude
- `background` — start indexing in the background and continue launching Claude immediately

If the variable is unset or has any other value, interactive launches fall back to the prompt flow and non-interactive launches do nothing.

```sh
# Always skip indexing for this launch
AMPLIHACK_ENABLE_BLARIFY=1 AMPLIHACK_BLARIFY_MODE=skip amplihack claude

# Force synchronous indexing before Claude starts
AMPLIHACK_ENABLE_BLARIFY=1 AMPLIHACK_BLARIFY_MODE=sync amplihack claude

# Allow non-interactive launches to queue background indexing
AMPLIHACK_ENABLE_BLARIFY=1 AMPLIHACK_BLARIFY_MODE=background amplihack claude --print 'summarize src'
```

**Why it exists:** Python session-start integration already used a `skip` / `sync` / `background` contract. Reintroducing the same lifecycle knob in Rust lets automated and non-interactive launches opt into indexing policy without waiting on a TTY prompt.

---

### AMPLIHACK_NO_UPDATE_CHECK

**Type:** flag
**Values:** `1` (skip update check) — absence or any other value means check is enabled
**Used by:** `update::should_skip_update_check()`,
`update::classify_skip_reason()`

Permanently disables both update-check paths for every `amplihack` invocation:

1. The pre-launch **npm tool update check** (notice for `claude`, `copilot`,
   `codex` package versions).
2. The **startup self-update prompt** (`Update now? [y/N] (5s timeout):` for
   the `amplihack` binary itself).

Unlike `AMPLIHACK_NONINTERACTIVE`, this variable suppresses only the update
checks and has no effect on bootstrap prompts or interactive behaviour. Unlike
the subprocess-safe skip signals (`CI`, `AMPLIHACK_AGENT_BINARY`,
`AMPLIHACK_NONINTERACTIVE`, `--subprocess-safe`, non-TTY stdin), this variable
**does not** emit the `amplihack: skipping update check (subprocess-safe / no
TTY)` skip-line on stderr — the suppression is silent. Use this when you want
the pre-#625 silent-skip experience.

```sh
# Add to shell profile for a permanent per-user opt-out
export AMPLIHACK_NO_UPDATE_CHECK=1
```

Equivalent to passing `--skip-update-check` on every invocation, but without
requiring the flag to be typed or aliased.

**When to prefer this over `AMPLIHACK_NONINTERACTIVE`:** Use
`AMPLIHACK_NO_UPDATE_CHECK=1` when you want to silence the update banner on a
developer workstation while keeping interactive bootstrap prompts active. Use
`AMPLIHACK_NONINTERACTIVE=1` in CI environments where all interactive output
should be suppressed.

---

### AMPLIHACK_PARITY_TEST

**Type:** flag
**Values:** `1` (parity-test mode active) — absence or any other value has no effect
**Used by:** `update::should_skip_update_check()`,
`update::classify_skip_reason()`

Suppresses both update-check paths (pre-launch npm tool notice and startup
self-update prompt) without enabling full non-interactive mode. This is useful
for automation that compares command output against a known baseline, where
update-banner stderr output would create spurious differences. Suppression is
**silent** — the `amplihack: skipping update check (subprocess-safe / no
TTY)` skip-line introduced by issue [#625](https://github.com/rysweet/amplihack-rs/issues/625)
is **not** emitted, preserving byte-identical stderr against pre-#625 baselines.

```sh
AMPLIHACK_PARITY_TEST=1 amplihack claude --print 'run tests'
```

**Custom automation scripts** that compare `amplihack` output against a known
baseline should also set this variable:

```sh
#!/usr/bin/env bash
# my-output-capture.sh
export AMPLIHACK_PARITY_TEST=1
actual=$(amplihack mode detect 2>&1)
expected="local"
[[ "$actual" == "$expected" ]] || { echo "FAIL: got $actual"; exit 1; }
```

**Isolation contract:** `AMPLIHACK_PARITY_TEST=1` suppresses exactly one
behaviour: the update check. It does not propagate into the child tool process,
does not affect bootstrap logic, and does not change exit codes or stdout output.

---

### AMPLIHACK_SKIP_AUTO_INSTALL

**Type:** flag
**Values:** any non-empty value (suppresses self-heal) — absence or empty string means the check runs
**Used by:** `self_heal::ensure_assets_match_binary_version` (via `env_bypass_set`)

Suppresses the startup-time **self-heal check** that runs before every
`amplihack` command dispatch. With the bypass active, `amplihack` does **not**
compare `crate::VERSION` to `~/.amplihack/.installed-version` and does **not**
auto-run install when the stamp is missing or stale. Explicit `amplihack install`
invocations are unaffected.

**When to set it:**

- **CI pipelines** that pre-stage assets in a setup phase and then run many
  `amplihack` commands without wanting the binary to mutate `~/.amplihack`
  mid-run.
- **Unit/integration tests** that stub out the framework asset tree and need
  to guarantee the binary will not overwrite it on first launch.
- **Sandboxed parity test harnesses** that already manage the
  `~/.amplihack` layout deterministically.

```sh
# CI: stage once, then run many commands without per-launch self-heal
amplihack install
export AMPLIHACK_SKIP_AUTO_INSTALL=1
amplihack claude --print 'run tests'
amplihack copilot --print 'run tests'
```

**Truthiness:** any non-empty value triggers the bypass (`1`, `true`,
`yes`, `please-skip`, etc.). An empty string (`AMPLIHACK_SKIP_AUTO_INSTALL=""`)
is treated as **unset** and the check still runs.

**Diagnostic on skip-with-mismatch:** when the bypass is active *and* the
stamp does not match `crate::VERSION`, `amplihack` emits one line on stderr
before dispatch:

```
amplihack: self-heal skipped (AMPLIHACK_SKIP_AUTO_INSTALL set); stamp=<old> current=<new>
```

This makes the "stale assets, intentionally" state visible in CI logs.
Matching versions produce no output.

**What it does not do:**

- Does not affect the existing `update::post_install` hook fired by
  `amplihack update`.
- Does not propagate into child tool processes (`claude`, `copilot`, etc.) —
  inherited only because it is a normal env var, not because `amplihack`
  re-exports it.
- Does not change exit codes, stdout output, or any other behaviour besides
  the self-heal decision.

See: [Self-Heal: Auto-Restage Framework Assets](../features/self-heal-asset-restage.md).

---

### AMPLIHACK_TEST_FAKE_LATEST_VERSION

**Type:** version tag string (test-only)
**Values:** any version tag accepted by `update::network::normalize_tag`
(e.g. `99.99.99`, `v0.9.3`); empty string is treated as unset.
**Used by:** `update::network::fetch_latest_release()`

Test-only short-circuit for the GitHub release lookup performed by the
startup self-update prompt. When set non-empty, `fetch_latest_release`
returns a synthetic `UpdateRelease` with the supplied tag and **no network
call** is made.

The synthetic release uses an `asset_url` on the allowlisted `github.com`
host and `checksum_url=None`, so any download path remains gated by the
existing URL allowlist and SHA-256 verification — the variable cannot be
used to redirect real downloads or bypass artifact verification. It exists
exclusively to drive the prompt code path deterministically from the
integration test suite at
`crates/amplihack-cli/tests/issue_625_update_prompt_subprocess_safe.rs`.

```sh
# Force a "newer release available" outcome without a network call
AMPLIHACK_TEST_FAKE_LATEST_VERSION=99.99.99 amplihack copilot --help
```

**Production deployments should not set this variable.** It is documented for
completeness only.

See: [Startup Self-Update Prompt — Subprocess-Safe Skip](../features/startup-update-prompt-subprocess-safe.md).

---

### CI

**Type:** flag (presence-based)
**Values:** any non-empty value (`1`, `true`, `yes`, anything) — empty string
or absence has no effect.
**Used by:** `update::classify_skip_reason()`

Conventional CI-runner marker recognised by `amplihack` as a subprocess-safe
signal. When set non-empty, the startup self-update prompt is skipped — the
`Update now? [y/N] (5s timeout):` line is never printed and stdin is never
read. A single skip-line `amplihack: skipping update check (subprocess-safe /
no TTY)` is emitted to stderr.

GitHub Actions, GitLab CI, CircleCI, Jenkins, Buildkite, and most other CI
runners set `CI=true` automatically, so this signal usually fires without any
explicit configuration.

```yaml
# .github/workflows/agent.yml — CI=true is set automatically by the runner.
- run: amplihack copilot -p "Run the test suite"
```

**Empty-string semantics:** `CI=""` does **not** trigger skip. Only non-empty
values are recognised — matching the convention used by
`commands::launch::command::resolve_subprocess_safe`.

**No effect on the npm pre-launch tool notice.** That notice is non-blocking
and is suppressed by `AMPLIHACK_NONINTERACTIVE` /
`AMPLIHACK_NO_UPDATE_CHECK` instead. See [Manage Tool Update
Notifications](../howto/manage-tool-update-checks.md).

**No effect on `--subprocess-safe` argv injection on the `copilot`
subcommand.** That feature uses its own resolver (see
[`COPILOT_SUBPROCESS_SAFE.md`](../COPILOT_SUBPROCESS_SAFE.md)); `CI` is one
of *its* signals as well, but the two paths are independent.

See: [Startup Self-Update Prompt — Subprocess-Safe Skip](../features/startup-update-prompt-subprocess-safe.md).

---

### UV_TOOL_BIN_DIR

**Type:** path
**Used by:** `bootstrap.rs` when installing `amplifier`

Override the directory where `uv tool install` places the `amplifier` binary. Defaults to `~/.local/bin`.

---

## Related

- [Agent Binary Routing](../concepts/agent-binary-routing.md) — Why `AMPLIHACK_AGENT_BINARY` exists and how recipe runner uses it
- [Run amplihack in Non-interactive Mode](../howto/run-in-noninteractive-mode.md) — CI and pipe usage guide
- [Bootstrap Parity](../concepts/bootstrap-parity.md) — How the Rust CLI matches the Python launcher's environment contract
- [Memory Backend Reference](./memory-backend.md) — `AMPLIHACK_MEMORY_BACKEND` values, storage paths, schema, and security
- [Recipe Runner Logging](./recipe-runner-logging.md) — Progress, heartbeat, snippet, and JSONL configuration
- [amplihack install](./install-command.md) — Variables read during installation
- [Startup Self-Update Prompt — Subprocess-Safe Skip](../features/startup-update-prompt-subprocess-safe.md) — How `CI`, `AMPLIHACK_AGENT_BINARY`, `AMPLIHACK_NONINTERACTIVE`, `--subprocess-safe`, and non-TTY stdin each suppress the `Update now? [y/N] (5s timeout):` prompt
- [Manage Tool Update Notifications](../howto/manage-tool-update-checks.md) — npm pre-launch tool update notice (separate code path)
