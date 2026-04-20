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
  - [NODE_OPTIONS](#node_options)
- [Variables injected by recipe executor](#variables-injected-by-recipe-executor)
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
  - [UV_TOOL_BIN_DIR](#uv_tool_bin_dir)

---

## Variables set by amplihack

These variables are injected into every child process launched by `amplihack`. They are not inherited from the parent shell; they are built fresh on each invocation.

---

### AMPLIHACK_AGENT_BINARY

**Type:** string
**Values:** `claude` | `copilot` | `codex` | `amplifier`
**Set by:** `EnvBuilder::with_agent_binary()`

Identifies which CLI binary was used to start the current session. Downstream consumers — the recipe runner, hooks, and sub-agents — read this variable to know which tool to invoke when they need to spawn a new AI session.

```sh
# Start a Claude session
amplihack claude

# Inside Claude Code hooks, the recipe runner sees:
echo $AMPLIHACK_AGENT_BINARY
# claude

# Start a Copilot session
amplihack copilot

# Inside hooks:
echo $AMPLIHACK_AGENT_BINARY
# copilot
```

**Why it exists:** The recipe runner is agent-agnostic; it must call back into whatever tool launched it. Without this variable, the runner would have to guess the binary name or require manual configuration. See [Agent Binary Routing](../concepts/agent-binary-routing.md) for the full rationale.

**Python parity:** Corresponds to `AMPLIHACK_AGENT_BINARY` set by the Python launcher in `amplihack/cli/launch.py`.

---

### AMPLIHACK_ASSET_RESOLVER

**Type:** path
**Example:** `/home/alice/.local/bin/amplihack-asset-resolver`
**Set by:** `EnvBuilder::with_asset_resolver()`

Absolute path to the native bundle-asset resolver. Child processes can execute this binary with a single relative asset path argument, for example:

```sh
"$AMPLIHACK_ASSET_RESOLVER" amplifier-bundle/tools/orch_helper.py
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

The version of the `amplihack-cli` crate that launched the session. Taken from `CARGO_PKG_VERSION` at compile time.

---

### NODE_OPTIONS

**Type:** space-separated Node.js CLI flags
**Set by:** launcher startup via `memory_config.rs`, then propagated by `EnvBuilder::with_amplihack_vars()`

`amplihack` now computes a smart `--max-old-space-size=<mb>` value at top-level launcher startup based on detected system RAM, persists the consent choice in `~/.amplihack/config`, and displays the active choice on launch. The resolved value is then propagated through `EnvBuilder`.

When startup does not supply an explicit value, `EnvBuilder` still falls back to `--max-old-space-size=32768`, and if ambient `NODE_OPTIONS` already contains `--max-old-space-size=` it is preserved rather than duplicated.

---

## Variables injected by recipe executor

These variables are set by the recipe executor (`amplihack recipe run`) in every shell step's child process. They are always set — there is no opt-out. See [Recipe Executor Environment](./recipe-executor-environment.md) for full details.

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
**Used by:** `update::should_skip_update_check()`

Permanently disables the pre-launch npm tool update check for every `amplihack`
invocation. Unlike `AMPLIHACK_NONINTERACTIVE`, this variable suppresses only the
update check and has no effect on bootstrap prompts or interactive behaviour.

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
**Used by:** `update::should_skip_update_check()`

Suppresses the pre-launch npm update check without enabling full
non-interactive mode. Set by the parity test harness in every sandbox it
creates so that update-banner stderr output does not produce spurious
Python↔Rust divergences.

```sh
# Set automatically by the parity harness — shown here for documentation only
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
- [amplihack install](./install-command.md) — Variables read during installation
