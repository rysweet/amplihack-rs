---
title: Launch flag matrix
type: reference
updated: 2026-10-05
---

# Launch Flag Matrix — Reference

How `amplihack` builds the subprocess command line for each supported AI
tool. Covers `--dangerously-skip-permissions`, `--model`, `--allow-all`,
and extra-args passthrough.

## Contents

- [Current implementation](#current-implementation)
- [Capability matrix](#capability-matrix)
- [Flag injection rules](#flag-injection-rules)
- [Codex invocation modes](#codex-invocation-modes)
- [Proposed design: type-safe refactoring](#proposed-design-type-safe-refactoring)
- [Related](#related)

---

## Current implementation

Flag logic lives in `crates/amplihack-cli/src/commands/launch/command.rs`.
The current approach uses ad-hoc string matching and standalone functions
rather than a unified type system.

### Tool identification

Claude-compatible tools are identified by a `matches!` expression:

```rust
// command.rs
pub(super) fn is_claude_compatible_tool(binary_name: &str) -> bool {
    matches!(binary_name, "claude" | "rusty" | "rustyclawd" | "amplifier")
}
```

### Copilot `--allow-all` injection

A standalone function determines whether to inject `--allow-all` for
Copilot:

```rust
// command.rs, line 87
pub(crate) fn should_inject_copilot_allow_all(extra_args: &[String]) -> bool {
    if std::env::var("AMPLIHACK_COPILOT_NO_ALLOW_ALL").as_deref() == Ok("1") {
        return false;
    }
    let already_present = extra_args.iter().any(|a| {
        a == "--allow-all"
            || a == "--allow-all-tools"
            || a == "--allow-all-paths"
            || a == "--allow-all-urls"
    });
    !already_present
}
```

## Capability matrix

Launcher flag injection behavior is derived from `command.rs`. Codex resume uses native subcommands. Native capabilities are distinct from launcher-injected flags; see [Codex invocation modes](#codex-invocation-modes).

| Flag | claude | rusty | rustyclawd | amplifier | copilot | codex |
|---|:---:|:---:|:---:|:---:|:---:|:---:|
| `--dangerously-skip-permissions` | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ |
| `--model` (auto-inject) | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ |
| `--allow-all` (auto-inject) | ❌ | ❌ | ❌ | ❌ | ✅ | ❌ |
| `--remote` (auto-inject) | ❌ | ❌ | ❌ | ❌ | ✅ | ❌ |
| `--resume` | ✅ | ✅ | ✅ | ✅ | ✅ | Native `resume` subcommand |
| `--continue` | ✅ | ✅ | ✅ | ✅ | ✅ | Native `resume --last` |
| `--plugin-dir` (UVX auto-inject only) | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| `--add-dir` (UVX auto-inject only) | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| Extra args passthrough | ✅ | ✅ | ✅ | ✅ | ✅ | Mode/conflict checks; managed recipe restrictions below |

The UVX rows describe automatic launcher injection only. Codex accepts explicit native `--add-dir` in the supported modes below; the launcher does not inject it for prompt staging.

### Override environment variables

| Variable | Effect |
|---|---|
| `AMPLIHACK_DEFAULT_MODEL` | Sets the model passed to Claude-compatible tools. Unset means `claude-opus-5[1m]`; empty or whitespace-only passes no `--model`; a dotted Claude id is rewritten to hyphens. See [`AMPLIHACK_DEFAULT_MODEL`](./environment-variables.md#amplihack_default_model) |
| `AMPLIHACK_COPILOT_NO_ALLOW_ALL` | Set to `1` to suppress `--allow-all` injection for Copilot |
| `AMPLIHACK_COPILOT_NO_REMOTE` | Set to `1` to suppress `--remote` injection for Copilot |

## Flag injection rules

1. **`--dangerously-skip-permissions`**: Injected only when the user passes
   `--skip-permissions` AND the tool is Claude-compatible. Never injected
   by default (SEC-2).

2. **`--model`**: Injected for Claude-compatible tools when the user did not
   already supply `--model` in extra args, unless `AMPLIHACK_DEFAULT_MODEL` is
   set to an empty value outside the LiteLLM gateway path. The value is `AMPLIHACK_LITELLM_MODEL` on the LiteLLM
   gateway path, otherwise `AMPLIHACK_DEFAULT_MODEL` (a dotted Claude id
   rewritten to hyphens, issue #1527), otherwise the concrete id
   `claude-opus-5[1m]`. The default is a concrete id rather than an alias
   because an alias is resolved by the tool and goes stale with the tool's
   version (issue #1421).

3. **`--allow-all`**: Injected only for `copilot` unless suppressed by env
   var or the user already provided any `--allow-all*` flag.

4. **`--remote`**: Injected only for `copilot` unless suppressed by
   `AMPLIHACK_COPILOT_NO_REMOTE=1` or the user already passed `--remote`
   or `--no-remote`.

5. **UVX plugin args**: `--plugin-dir` and `--add-dir` are injected only
   for `claude` when running in a UVX deployment.

6. **Extra args**: Remaining arguments are passed through unchanged,
   appended after injected flags, subject to Codex mode/conflict validation
   and the managed recipe restrictions in [Codex invocation modes](#codex-invocation-modes).
   Claude and Copilot retain their existing passthrough behavior.

## Proposed design: type-safe refactoring

The following types are **design specifications for future implementation**.
They do not exist in the current codebase.

### `AgentBinary` enum

```rust
// Proposed — not yet implemented
pub enum AgentBinary {
    Claude,
    Rusty,
    RustyClawd,
    Amplifier,
    Copilot,
    Codex,
}

impl AgentBinary {
    pub fn from_name(name: &str) -> Option<Self> { /* ... */ }
    pub fn is_claude_compatible(&self) -> bool { /* ... */ }
    pub fn supports_flag(&self, flag: Flag) -> bool { /* ... */ }
}
```

### `FlagSet` struct

```rust
// Proposed — not yet implemented
pub struct FlagSet {
    flags: Vec<Flag>,
}

impl FlagSet {
    /// Build the correct flag set for a given binary + user options.
    pub fn for_binary(binary: &AgentBinary, opts: &LaunchOpts) -> Self { /* ... */ }

    /// Append flags to a Command.
    pub fn apply(&self, cmd: &mut Command) { /* ... */ }
}
```

The goal is to replace the scattered `if`/`matches!` logic with a single
matrix lookup, making it impossible to add a new tool without specifying
its full flag capabilities.

### Test table (proposed)

| Test case | Input | Expected flags |
|---|---|---|
| Claude default | `claude`, no extra args | `--model claude-opus-5[1m]` |
| Claude skip-perms | `claude`, `--skip-permissions` | `--dangerously-skip-permissions --model claude-opus-5[1m]` |
| Claude pinned model | `claude`, `AMPLIHACK_DEFAULT_MODEL=sonnet` | `--model sonnet` |
| Copilot default | `copilot`, no extra args | `--allow-all --remote` |
| Copilot suppressed | `copilot`, `AMPLIHACK_COPILOT_NO_ALLOW_ALL=1` + `AMPLIHACK_COPILOT_NO_REMOTE=1` | (no flags) |
| Copilot no-remote | `copilot`, `--no-remote` | `--allow-all` (no `--remote` injected) |
| Codex default | `codex`, no extra args | (no flags) |
| User model override | `claude`, `--model sonnet` | `--model sonnet` (no duplicate) |

## Related

- [Launch Flag Injection](./launch-flag-injection.md) — Detailed reference for the existing injection logic
- [Environment Variables](./environment-variables.md) — All env vars read by amplihack
- [Agent Binary Routing](../concepts/agent-binary-routing.md) — How `AMPLIHACK_AGENT_BINARY` routes callbacks

## Codex invocation modes

The following table describes the mode-specific Codex integration. Native capabilities are selected by mode and fresh/resume operation. No model is injected by default. Caller model choices are forwarded using native `--model`; `-p` means profile.

### Semantic argument parsing

Mode detection consumes supported root options and their values before identifying
the actual `exec` (alias `e`) or `resume` command. Separate, inline and supported
attached option values retain their native meaning. A model or profile value
literally named `exec` or `resume` does not select that mode. A root `--` ends
command detection: subsequent command-like tokens are positional input. Argument
bytes and explicit model choices are preserved; ambiguous or unsupported syntax
is rejected before spawning.

Root options before `exec` select the same complete stdin transport as exec-first
arguments, including explicit stdin delivery and prompts above the interactive
96 KiB limit. Interactive delivery retains terminal stdin and its existing input
limits. Resume validation uses the parsed command position, including root options
before `resume` and `exec resume`.

### Mode capabilities

| Capability | Interactive fresh | Interactive resume | Exec fresh | Exec resume |
| --- | --- | --- | --- | --- |
| Prompt transport | Positional; terminal stdin retained | Positional; terminal stdin retained | Complete stdin with EOF | Complete stdin with EOF |
| Model | Caller-selected | Caller-selected | Caller-selected | Caller-selected |
| Profile / add-dir / sandbox | Native caller options | Native caller options | Native caller options | Unsupported local combinations rejected in v1 |
| Final response file | Not used | Not used | `--output-last-message` | `--output-last-message` |

Resume uses a session ID or `--last`; conflicting selections fail. Managed recipes reject competing positional prompts, `--prompt`, output-file overrides, `--json`, and mode-changing passthrough. These restrictions preserve complete instructions and the final-message contract. Raw native CLI support can be broader than the managed integration. No directory is made writable solely for prompt staging.

See [Codex installation and usage](../howto/install-codex-plugin.md) for examples, configuration preservation, native hook trust, and runner delivery.
