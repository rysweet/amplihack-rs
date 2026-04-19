# amplihack RustyClawd — Command Reference

## Synopsis

```
amplihack RustyClawd [OPTIONS] [ARGS]...
```

## Description

Launches the RustyClawd tool via the native Rust launcher path. RustyClawd is a
Claude-compatible tool, so it receives `--dangerously-skip-permissions` (when
`--skip-permissions` is set) and `--model` injection from the launch flag
injection system.

The command name is case-sensitive: use `RustyClawd` exactly (not `rustyclawd`
or `rusty-clawd`). This is set via an explicit `#[command(name = "RustyClawd")]`
attribute.

## Options

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--append <TEXT>` | string | — | Append instructions to a running auto mode session and exit. The session receives the text without restarting. |
| `--no-reflection` | bool | `false` | Disable post-session reflection analysis. |
| `--subprocess-safe` | bool | `false` | Skip shared launcher staging and environment updates for subprocess delegates. Use when RustyClawd is launched as a child of another amplihack process. |
| `--auto` | bool | `false` | Run in autonomous agentic mode with iterative loop execution. |
| `--max-turns <N>` | integer | `10` | Maximum number of turns for auto mode. Must be ≥ 1. Only meaningful when `--auto` is set. |
| `--ui` | bool | `false` | Enable interactive UI mode for auto mode. Only meaningful when `--auto` is set. |

## Trailing Arguments

All positional arguments after the flags are forwarded verbatim to the
RustyClawd/Claude binary:

```sh
amplihack RustyClawd --print 'Fix the failing tests'
# Forwards: --print 'Fix the failing tests'
```

Hyphen-prefixed values are allowed in trailing arguments.

## Examples

```sh
# Launch RustyClawd interactively
amplihack RustyClawd

# Run in auto mode with 5 turns
amplihack RustyClawd --auto --max-turns 5

# Append to a running session
amplihack RustyClawd --append 'Also fix the linting errors'

# Launch without reflection analysis
amplihack RustyClawd --no-reflection
```

## Flag Injection

RustyClawd is Claude-compatible. The launch flag injection system applies:

- `--dangerously-skip-permissions` is injected when `--skip-permissions` is
  passed to the parent amplihack command
- `--model` is injected unless the user supplies `--model` in trailing args

See [Launch Flag Injection](./launch-flag-injection.md) for details.

## Exit Codes

| Code | Meaning |
|------|---------|
| `0` | Success |
| `1` | Error |

## Related

- [Launch Flag Injection](./launch-flag-injection.md) — How flags are injected into Claude-compatible tools
- [completions Command](./completions-command.md) — Tab-completion includes `RustyClawd`
