# Launch Flag Matrix — Reference

How `amplihack` builds the subprocess command line for each supported AI
tool. Covers `--dangerously-skip-permissions`, `--model`, `--allow-all`,
and extra-args passthrough.

## Contents

- [Current implementation](#current-implementation)
- [Capability matrix](#capability-matrix)
- [Flag injection rules](#flag-injection-rules)
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
// command.rs, line 43
let is_claude_compatible = matches!(
    binary.name.as_str(),
    "claude" | "rusty" | "rustyclawd" | "amplifier"
);
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

Current behavior, derived from `command.rs`:

| Flag | claude | rusty | rustyclawd | amplifier | copilot | codex |
|---|:---:|:---:|:---:|:---:|:---:|:---:|
| `--dangerously-skip-permissions` | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ |
| `--model` (auto-inject) | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ |
| `--allow-all` (auto-inject) | ❌ | ❌ | ❌ | ❌ | ✅ | ❌ |
| `--resume` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `--continue` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `--plugin-dir` (UVX only) | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| `--add-dir` (UVX only) | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| Extra args passthrough | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |

### Override environment variables

| Variable | Effect |
|---|---|
| `AMPLIHACK_DEFAULT_MODEL` | Override the default model (default: `opus[1m]`) |
| `AMPLIHACK_COPILOT_NO_ALLOW_ALL` | Set to `1` to suppress `--allow-all` injection for Copilot |

## Flag injection rules

1. **`--dangerously-skip-permissions`**: Injected only when the user passes
   `--skip-permissions` AND the tool is Claude-compatible. Never injected
   by default (SEC-2).

2. **`--model`**: Injected for Claude-compatible tools unless the user
   already supplied `--model` in extra args. Defaults to `opus[1m]` or
   the value of `AMPLIHACK_DEFAULT_MODEL`.

3. **`--allow-all`**: Injected only for `copilot` unless suppressed by env
   var or the user already provided any `--allow-all*` flag.

4. **UVX plugin args**: `--plugin-dir` and `--add-dir` are injected only
   for `claude` when running in a UVX deployment.

5. **Extra args**: All remaining arguments are passed through unchanged,
   appended after injected flags.

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
| Claude default | `claude`, no extra args | `--model opus[1m]` |
| Claude skip-perms | `claude`, `--skip-permissions` | `--dangerously-skip-permissions --model opus[1m]` |
| Copilot default | `copilot`, no extra args | `--allow-all` |
| Copilot suppressed | `copilot`, `AMPLIHACK_COPILOT_NO_ALLOW_ALL=1` | (no flags) |
| Codex default | `codex`, no extra args | (no flags) |
| User model override | `claude`, `--model sonnet` | `--model sonnet` (no duplicate) |

## Related

- [Launch Flag Injection](./launch-flag-injection.md) — Detailed reference for the existing injection logic
- [Environment Variables](./environment-variables.md) — All env vars read by amplihack
- [Agent Binary Routing](../concepts/agent-binary-routing.md) — How `AMPLIHACK_AGENT_BINARY` routes callbacks
