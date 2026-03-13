# Environment Variables Reference

This document is the authoritative reference for every environment variable that the
amplihack-rs launcher reads from or injects into child processes.

## Contents

- [Variables injected by the launcher](#variables-injected-by-the-launcher)
  - [AMPLIHACK_SESSION_ID](#amplihack_session_id)
  - [AMPLIHACK_DEPTH](#amplihack_depth)
  - [AMPLIHACK_RUST_RUNTIME](#amplihack_rust_runtime)
  - [AMPLIHACK_VERSION](#amplihack_version)
  - [NODE_OPTIONS](#node_options)
  - [AMPLIHACK_AGENT_BINARY](#amplihack_agent_binary)
  - [AMPLIHACK_HOME](#amplihack_home)
  - [AMPLIHACK_NONINTERACTIVE](#amplihack_noninteractive)
- [Variables read by the launcher](#variables-read-by-the-launcher)
- [EnvBuilder chain order](#envbuilder-chain-order)

---

## Variables injected by the launcher

The `EnvBuilder` in `crates/amplihack-cli/src/env_builder.rs` constructs the child
process environment. Methods are called in the canonical order shown in
[EnvBuilder chain order](#envbuilder-chain-order).

---

### AMPLIHACK_SESSION_ID

| Property | Value |
|----------|-------|
| Set by | `EnvBuilder::with_amplihack_session_id()` |
| Format | `<unix_timestamp_ms>-<pid>` |
| Example | `1741824000000-12345` |

Unique identifier for the top-level launch session. Used for log correlation across
the Rust binary, recipe runner, and Python hooks.

> **Note:** Entropy is intentionally low (timestamp + PID). This identifier is for
> correlation only, not authentication. Do not use it as a security token.

---

### AMPLIHACK_DEPTH

| Property | Value |
|----------|-------|
| Set by | `EnvBuilder::with_amplihack_session_id()` |
| Format | Decimal integer string |
| Default | `"1"` (first launch; incremented by each nested spawn) |
| Example | `"1"`, `"2"`, `"3"` |

Nesting level of the current launch. Starts at `"1"` on the first launch and
is incremented each time `amplihack` spawns a child `amplihack` process.
Prevents runaway recursive launches.

---

### AMPLIHACK_RUST_RUNTIME

| Property | Value |
|----------|-------|
| Set by | `EnvBuilder::with_amplihack_vars()` |
| Value | `"1"` (always) |

Signals to downstream Python and recipe-runner code that the parent process is the
Rust CLI binary, not the legacy Python installer. Used for Python-side feature flags.

---

### AMPLIHACK_VERSION

| Property | Value |
|----------|-------|
| Set by | `EnvBuilder::with_amplihack_vars()` |
| Value | The `CARGO_PKG_VERSION` of `amplihack-cli` at compile time |
| Example | `"0.3.1"` |

Propagated to child processes so hooks and the recipe runner can log or gate behaviour
on the CLI version without shelling back to `amplihack --version`.

---

### NODE_OPTIONS

| Property | Value |
|----------|-------|
| Set by | `EnvBuilder::with_amplihack_vars()` |
| Value | `"--no-deprecation"` (appended to any existing value) |

Suppresses Node.js deprecation warnings emitted by the Claude Code CLI. Appended
rather than overwritten so existing `NODE_OPTIONS` from the parent environment are
preserved.

---

### AMPLIHACK_AGENT_BINARY

| Property | Value |
|----------|-------|
| Set by | `EnvBuilder::with_agent_binary(tool)` |
| Allowed values | `"claude"`, `"copilot"`, `"codex"`, `"amplifier"` |
| Example | `"claude"` |
| Python parity | `os.environ["AMPLIHACK_AGENT_BINARY"]` read by Python recipe runner |

Identifies which agent CLI binary the user invoked (e.g. `claude`, `copilot`). The
recipe runner is agent-agnostic and reads this variable to select the correct
sub-command syntax when spawning the agent.

**Validation:** In debug and test builds a `debug_assert!` verifies the value is in
the allowlist `{"claude", "copilot", "codex", "amplifier"}`. Release builds skip this
check for performance; the allowlist is enforced at the CLI argument parsing layer.

See [Agent Binary Routing](../concepts/agent-binary-routing.md) for a full explanation
of why this variable is needed.

---

### AMPLIHACK_HOME

| Property | Value |
|----------|-------|
| Set by | `EnvBuilder::with_amplihack_home()` |
| Default | `$HOME/.amplihack` (see resolution order below) |
| Example | `"/home/alice/.amplihack"` |

Points to the amplihack home directory where helper scripts, templates, and recipe
files are stored. The child-process recipe runner reads this variable to locate
resources without re-running the resolution logic.

**Resolution order** (first success wins):

1. `AMPLIHACK_HOME` already set in the parent environment → **no-op**, keep the
   existing value.
2. `HOME` environment variable is set → use `$HOME/.amplihack`.
3. `std::env::current_exe()` succeeds → use the parent directory of the running
   executable as the home root.
4. All options fail → `AMPLIHACK_HOME` is **not set** in the child environment.
   Silent degradation; the consumer is expected to handle the missing variable.

**Security constraints:**

- Paths containing `..` (Parent Directory) components are rejected and a
  `tracing::warn` is emitted. This prevents a `HOME=/../../etc` attack from
  propagating a traversal path to child processes.
- Non-absolute resolved paths are rejected with the same warning.
- The launcher does **not** check whether the resolved path exists on disk; the
  consumer is responsible for creating it.
- The launcher does **not** canonicalize the path (canonicalization follows symlinks,
  which is unnecessary here and has surprising behaviour on some platforms).

**Override example:**

```sh
# Use a custom home directory
AMPLIHACK_HOME=/opt/amplihack amplihack launch claude
```

---

### AMPLIHACK_NONINTERACTIVE

| Property | Value |
|----------|-------|
| Set by | `EnvBuilder::set_if(is_noninteractive(), "AMPLIHACK_NONINTERACTIVE", "1")` |
| Trigger value | `"1"` only |
| Other values | `"0"`, `"false"`, `"yes"`, `"on"` — all treated as **falsy** |

Signals that the current session is running in a non-interactive environment (CI,
pipes, scripts). When set, `bootstrap::prepare_launcher` returns immediately without
prompting the user or running framework setup that requires a terminal.

**Detection logic** (`util::is_noninteractive`):

```
AMPLIHACK_NONINTERACTIVE == "1"  →  non-interactive
stdin is not a TTY               →  non-interactive (e.g. piped input)
otherwise                        →  interactive
```

Detection is evaluated once at startup in the Rust binary and propagated to child
processes so the recipe runner and Python hooks observe the same value without
re-evaluating TTY state.

> **Security note:** `AMPLIHACK_NONINTERACTIVE` is a **UX convenience flag**, not a
> security gate. Setting it to `"1"` does not grant additional privileges or bypass
> access controls. Authentication and authorisation decisions must not rely on this
> variable.

**Cross-language contract:** Only the string `"1"` triggers non-interactive mode.
Python code that reads `AMPLIHACK_NONINTERACTIVE` must use `== "1"` and must **not**
use truthiness checks (`"true"`, `"yes"`, `"on"` are falsy in this contract).

**Propagation:** The value is only written to the child environment when the parent
detects non-interactive mode. This ensures interactive sessions do not accidentally
suppress interactive behaviour in child processes.

See [Run in Non-Interactive Mode](../howto/run-in-noninteractive-mode.md) for
practical CI examples.

---

## Variables read by the launcher

These variables influence launcher behaviour but are read from the **parent**
environment rather than injected.

| Variable | Read by | Effect |
|----------|---------|--------|
| `AMPLIHACK_HOME` | `with_amplihack_home()` | If set, skip resolution; propagate as-is |
| `AMPLIHACK_NONINTERACTIVE` | `util::is_noninteractive()` | If `"1"`, activate non-interactive mode |
| `HOME` | `with_amplihack_home()` | Fallback home directory base |
| `RUST_LOG` | `tracing` subscriber | Controls log verbosity |

---

## EnvBuilder chain order

The following shows the canonical order in which `EnvBuilder` methods are called in
`crates/amplihack-cli/src/commands/launch.rs`. All keys set by this chain are
**disjoint** — no method overwrites another's key.

```rust
EnvBuilder::new()
    .with_amplihack_session_id()   // AMPLIHACK_SESSION_ID, AMPLIHACK_DEPTH
    .with_amplihack_vars()         // AMPLIHACK_RUST_RUNTIME, AMPLIHACK_VERSION, NODE_OPTIONS
    .with_agent_binary(tool)       // AMPLIHACK_AGENT_BINARY
    .with_amplihack_home()         // AMPLIHACK_HOME
    .set_if(                       // AMPLIHACK_NONINTERACTIVE
        is_noninteractive(),
        "AMPLIHACK_NONINTERACTIVE",
        "1",
    )
    .build()
```

> **Logging safety:** The full env `HashMap` is never logged. Only `env.len()` (the
> count of set variables) is recorded in debug output. The map may contain inherited
> secrets from the parent process.
