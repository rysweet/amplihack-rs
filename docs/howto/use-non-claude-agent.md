# How to Use amplihack with a Non-Claude Agent

amplihack is agent-agnostic. It runs agent steps under one of four CLIs:
`claude` (Claude Code), `copilot` (GitHub Copilot CLI), `codex` or
`amplifier`. Inside one of those CLIs' sessions it uses that CLI. To choose
one explicitly, set the `AMPLIHACK_AGENT_BINARY` environment variable.

## Prerequisites

- amplihack installed and configured
- Your target agent CLI installed and in `PATH`

## Set the agent binary

```bash
export AMPLIHACK_AGENT_BINARY=copilot
```

The value must be one of `claude`, `copilot`, `codex` or `amplifier`. Anything
else, including a path to a binary, is rejected and ignored, never coerced
into a different name. The rules are under Allowlist & Validation in
[Active Agent Binary](../reference/active-agent-binary.md).

All subprocess orchestration — including nested agents spawned by the recipe
runner, fleet, multi-task, and auto_mode — will use this binary.

### Examples

```bash
# Use Claude Code
export AMPLIHACK_AGENT_BINARY=claude

# Use GitHub Copilot CLI (if installed)
export AMPLIHACK_AGENT_BINARY=copilot
```

## Verify propagation

After setting the variable, ask amplihack which CLI it will use and why:

```bash
amplihack agent-binary
# copilot (env)
```

When the recipe runner launches nested agent steps, it inherits this variable.
You will see `AMPLIHACK_AGENT_BINARY` in the child process environment rather
than a hardcoded `claude` invocation.

## Fallback behaviour

If `AMPLIHACK_AGENT_BINARY` is not set, or is rejected, amplihack uses the
session markers of the CLI it is running in (for example `CLAUDECODE` or
`COPILOT_CLI`), then a fresh `.claude/runtime/launcher_context.json`, then the
default, `copilot`. `amplihack recipe run` and `amplihack agent-binary` print a
notice on stderr whenever the answer was inferred rather than chosen. The full
precedence is in
[Active Agent Binary](../reference/active-agent-binary.md#resolution-precedence).

## Python API (knowledge builder)

If you call the knowledge builder directly from Python, note that newer code
uses `agent_cmd` and older examples may still refer to `claude_cmd`:

```rust
# Old (deprecated)
orchestrator = KnowledgeOrchestrator(claude_cmd="claude")

# New
orchestrator = KnowledgeOrchestrator(agent_cmd="claude")
```

## Nested agent sessions

The dev-orchestrator recipe runner preserves `AMPLIHACK_AGENT_BINARY` when
launching sub-recipes. If you set the variable before your first `/dev` call,
all workstreams — including parallel ones — inherit it.

### Nested Copilot prompt compatibility

Nested Copilot launches still need a small compatibility bridge because the
Rust recipe runner can emit Claude-style prompt flags that Copilot CLI does
not accept directly.

When `AMPLIHACK_AGENT_BINARY=copilot`, amplihack prepends a narrow wrapper to
`PATH` for nested recipe-runner launches only. That wrapper:

- rewrites `--system-prompt` and `--append-system-prompt` into one merged `-p`
  prompt for Copilot CLI
- drops Claude-only `--dangerously-skip-permissions`
- converts Claude-style `--disallowed-tools` into a no-tools prompt instruction
  without reintroducing `--allow-all-tools`
- preserves explicit Copilot permission flags when they are already present
- injects broad Copilot permission defaults only when the nested launch did not
  provide explicit permission flags
- leaves top-level `amplihack copilot` launcher behavior unchanged

This is an adapter for the current nested launch contract, not a general
argument normalization layer for every Copilot invocation.

## Smart-orchestrator classify step

The `/dev` command uses the smart-orchestrator recipe, which begins with a
`classify-and-decompose` step. This step invokes the agent binary to classify
the task as Q&A, Operations, Investigation, or Development.

When `AMPLIHACK_AGENT_BINARY=copilot` (or any `*copilot*`/`*codex*` binary),
the classify step automatically:

- Omits `--dangerously-skip-permissions`, `--disallowed-tools`, and
  `--append-system-prompt` (Claude-only flags that Copilot rejects)
- Uses `--allow-all-tools` instead
- Injects the classifier constraint directly into the prompt text

When the agent binary is `claude` (or unset), the existing Claude-specific flags
are passed as before.

If the classify step fails (non-zero exit), the recipe prints the agent binary
name, exit code, and stderr to help diagnose the issue. Common causes include
the binary not being installed, a missing API key, or network problems.

## See Also

- [Dev-Orchestrator Tutorial](../tutorials/dev-orchestrator-tutorial.md#execution-modes)
- [Tutorial: Enable the Copilot parity control plane](../tutorials/copilot-parity-control-plane.md)
- [How to Configure the Copilot Parity Control Plane](./configure-copilot-parity-control-plane.md)
- [Copilot Parity Control Plane Reference](../reference/hook-specifications.md)
