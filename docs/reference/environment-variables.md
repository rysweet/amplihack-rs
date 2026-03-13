# Environment Variables — Reference

All environment variables read or written by `amplihack` during a launch (`amplihack claude`, `amplihack copilot`, `amplihack codex`, `amplihack amplifier`).

## Contents

- [Variables set by amplihack](#variables-set-by-amplihack)
  - [AMPLIHACK_AGENT_BINARY](#amplihack_agent_binary)
  - [AMPLIHACK_HOME](#amplihack_home)
  - [AMPLIHACK_NONINTERACTIVE](#amplihack_noninteractive)
  - [AMPLIHACK_SESSION_ID](#amplihack_session_id)
  - [AMPLIHACK_DEPTH](#amplihack_depth)
  - [AMPLIHACK_RUST_RUNTIME](#amplihack_rust_runtime)
  - [AMPLIHACK_VERSION](#amplihack_version)
  - [NODE_OPTIONS](#node_options)
- [Variables read by amplihack](#variables-read-by-amplihack)
  - [HOME](#home)
  - [AMPLIHACK_DEFAULT_MODEL](#amplihack_default_model)
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
**Set by:** `EnvBuilder::with_amplihack_vars()`

`amplihack` injects `--max-old-space-size=32768` to raise the Node.js heap limit for Claude Code's large context operations. If `NODE_OPTIONS` is already set in the environment, `amplihack` appends the flag rather than replacing it, unless `--max-old-space-size=` is already present.

---

## Variables read by amplihack

These variables influence `amplihack`'s behaviour but are not set by it.

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
# Passes: claude --model sonnet
```

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
- [amplihack install](./install-command.md) — Variables read during installation
