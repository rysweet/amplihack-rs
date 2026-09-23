# Launch Flag Injection — Reference

When `amplihack` starts `claude`, `copilot`, `codex`, or `amplifier`, it builds
the subprocess command line by combining flags it injects automatically with any
extra arguments the user supplied. This document describes every flag that
`amplihack` injects, the conditions under which each is injected, and how users
can override the defaults.

## Contents

- [Injected flags overview](#injected-flags-overview)
- [--dangerously-skip-permissions](#-dangerously-skip-permissions)
- [--model](#-model)
- [--resume and --continue](#-resume-and-continue)
- [Extra args passthrough](#extra-args-passthrough)
- [Complete command-line assembly](#complete-command-line-assembly)
- [Python launcher parity](#python-launcher-parity)
- [Related](#related)

---

## Injected flags overview

| Flag | Injected when? | Applicable tools | Override mechanism |
|------|----------------|-------------------|-------------------|
| `--dangerously-skip-permissions` | `--skip-permissions` passed AND tool is Claude-compatible | `claude`, `rusty`, `rustyclawd`, `amplifier` | omit `--skip-permissions` |
| `--model <value>` | **only** when `AMPLIHACK_DEFAULT_MODEL` is set AND user did not pass `--model` AND tool is Claude-compatible | `claude`, `rusty`, `rustyclawd`, `amplifier` | unset `AMPLIHACK_DEFAULT_MODEL` to let the tool pick its own default |
| `--resume` | only when `amplihack launch --resume` | `launch` only (not `claude`) | pass `--resume` to the `launch` subcommand |
| `--continue` | only when `amplihack launch --continue` | `launch` only (not `claude`) | pass `--continue` to the `launch` subcommand |

---

## --dangerously-skip-permissions

`amplihack` passes `--dangerously-skip-permissions` to the tool subprocess only
when **both** conditions are met:

1. The user passed `--skip-permissions` on the amplihack command line.
2. The target tool is **Claude-compatible** (`claude`, `rusty`, `rustyclawd`, or `amplifier`).

Tools that are not Claude-compatible (`copilot`, `codex`) never receive this
flag because they do not support it.

```sh
# User explicitly opts in:
amplihack launch --skip-permissions

# amplihack spawns:
claude --dangerously-skip-permissions
```

```sh
# Without --skip-permissions, the flag is NOT injected:
amplihack claude

# amplihack spawns:
claude
```

```sh
# Non-Claude tools never receive it, even with --skip-permissions:
amplihack copilot --skip-permissions

# amplihack spawns:
copilot <extra_args...>
```

**Rationale:** The `--dangerously-skip-permissions` flag bypasses Claude's
interactive confirmation prompts. It is gated behind an explicit opt-in
(`--skip-permissions`) so that users in trusted automated environments can
suppress the prompt, while interactive sessions retain the safety check.

**Python launcher note:** The Python launcher in `amplihack/launcher/core.py`
may behave differently. Verify Python launcher behavior independently.

---

## --model

`amplihack` does **not** choose a model. When you ask for one it is passed
through to the subprocess; when you do not, nothing is passed and the tool
applies its own current default. Injection, when it happens, is only for
**Claude-compatible** tools (`claude`, `rusty`, `rustyclawd`, `amplifier`). Non-Claude
tools (`copilot`, `codex`) use their own default model selection.

### Default: no model at all

With neither `--model` on the command line nor `AMPLIHACK_DEFAULT_MODEL` in the
environment, the command line carries no `--model`:

```sh
# User runs:
amplihack claude

# amplihack spawns:
claude
```

The tool then resolves its own default, and any `"model"` you have set in
`~/.claude/settings.json` takes effect.

**Why there is no built-in default (issue #1421):** amplihack used to force
`--model opus[1m]`. That string is an alias resolved by the tool, not by
amplihack, and what it resolves to depends on the tool's version. On one
install it resolved to the retired `claude-opus-4-1-20250805`, so every agent
step failed with `API Error: 404 ... model: claude-opus-4-1-20250805` — an id
the user had never chosen and could not find in any config file or binary.
Forcing an alias also silently outranked that user's `~/.claude/settings.json`.
amplihack does not own the model catalogue and cannot keep an alias current
across tool versions, so it no longer tries.

### Pinning a model via environment variable

Set `AMPLIHACK_DEFAULT_MODEL` to pin a model for all sessions without
changing the command line. This is opt-in: unset, no model is passed.

```sh
export AMPLIHACK_DEFAULT_MODEL=sonnet
amplihack claude

# amplihack spawns:
claude --model sonnet
```

When amplihack puts a model on the command line it announces it on stderr:

```
amplihack: passing `--model sonnet` to `claude` (from AMPLIHACK_DEFAULT_MODEL).
Unset AMPLIHACK_DEFAULT_MODEL to let claude choose its own default model.
```

That line exists so a later "model not found" error names an id you can trace
back to a decision you made, rather than to an invisible constant. A value that
is empty or whitespace-only is treated as unset.

This is the recommended approach for teams or CI environments that standardise
on a particular model.

```yaml
# .github/workflows/ai-tasks.yml
env:
  AMPLIHACK_DEFAULT_MODEL: "sonnet"
  AMPLIHACK_NONINTERACTIVE: "1"

steps:
  - run: amplihack claude --print 'Run the lint checks'
    # spawns: claude --model sonnet --print 'Run the lint checks'
```

### Override via command-line flag

Pass `--model` directly to suppress injection entirely. The user-supplied value
is forwarded unchanged and `AMPLIHACK_DEFAULT_MODEL` is ignored:

```sh
amplihack claude --model haiku

# amplihack spawns:
claude --model haiku
```

Detection is substring-based: if any element of the extra args list equals
`--model`, the injection step is skipped. Partial matches (e.g. `--model-config`)
are not treated as model overrides.

### Supported model identifiers

`amplihack` does not validate the model string — any value is forwarded as-is
to the tool. Refer to the tool's own documentation for supported model names.
Examples that work at time of writing:

Aliases such as `opus[1m]`, `sonnet`, and `haiku` are resolved by the tool, and
which concrete model each maps to changes with the tool's version. This document
deliberately does not tabulate those mappings: a table of them here would go
stale exactly the way the old hardcoded default did. Ask the tool
(`claude --help`, or its release notes) for the current list.

---

## --resume and --continue

These flags are passed through only when the user explicitly requests them on
the `launch` subcommand. They are never injected automatically.

**Important:** The `claude` subcommand does **not** support `--resume` or
`--continue`. Only `launch` exposes these flags.

```sh
amplihack launch --resume --skip-permissions
# spawns: claude --dangerously-skip-permissions --resume

amplihack launch --continue --skip-permissions
# spawns: claude --dangerously-skip-permissions --continue
```

The `claude`, `copilot`, `codex`, and `amplifier` subcommands do not support
`--resume` or `--continue`.

---

## Extra args passthrough

All positional arguments and flags after the subcommand name are forwarded
verbatim to the tool subprocess after the injected flags. Order is:

```
<binary> [--dangerously-skip-permissions] [--model <value>] [--resume|--continue] <extra_args...>
```

```sh
amplihack claude --print 'Fix the failing tests' --output-format json

# amplihack spawns:
claude --print 'Fix the failing tests' --output-format json
```

There is no processing or escaping of `extra_args`. What the user types is what
the subprocess receives.

---

## Complete command-line assembly

`build_command()` in `crates/amplihack-cli/src/commands/launch.rs` assembles
the final command line. The assembly order is:

1. Binary path (resolved by `bootstrap::ensure_tool_available()`)
2. `--dangerously-skip-permissions` — only if `skip_permissions == true` **and**
   the tool is Claude-compatible (`claude`, `rusty`, `rustyclawd`, `amplifier`)
3. `--model <value>` — only if `AMPLIHACK_DEFAULT_MODEL` is set to a non-blank
   value, `--model` is not already present in `extra_args`, **and** the tool is
   Claude-compatible. There is no built-in default (issue #1421)
4. `--resume` (if requested — `launch` subcommand only)
5. `--continue` (if requested — `launch` subcommand only)
6. All `extra_args` in the order they were passed on the command line

The following examples show the full assembled command for each launch
subcommand with no extra args:

```sh
amplihack claude
# → claude

amplihack claude --skip-permissions
# → claude --dangerously-skip-permissions

amplihack copilot
# → copilot

amplihack codex
# → codex

amplihack amplifier
# → amplifier

amplihack launch --skip-permissions
# → claude --dangerously-skip-permissions

AMPLIHACK_DEFAULT_MODEL=sonnet amplihack claude
# → claude --model sonnet
```

---

## Python launcher parity

The Rust launcher's injection behaviour is designed to match the Python launcher
in `amplihack/launcher/core.py`. The following table documents the parity
contract:

| Behaviour | Python launcher | Rust launcher |
|-----------|----------------|---------------|
| `--dangerously-skip-permissions` | always injected | conditional: Claude-compatible tool AND `--skip-permissions` |
| `--model <default>` | `opus[1m]` unless `AMPLIHACK_DEFAULT_MODEL` set | **intentional divergence (#1421):** no built-in default; injected only when `AMPLIHACK_DEFAULT_MODEL` is set, Claude-compatible tools only |
| `--model` suppressed when user provides it | yes | yes |
| `--resume` passthrough | yes | `launch` subcommand only |
| `--continue` passthrough | yes | `launch` subcommand only |
| `extra_args` forwarded verbatim | yes | yes |

Intentional divergences (not bugs) are documented in
[Parity Test Scenarios](./parity-test-scenarios.md).

---

## Related

- [Environment Variables](./environment-variables.md) — `AMPLIHACK_DEFAULT_MODEL` and other variables that influence launch behaviour
- [Parity Test Scenarios](./parity-test-scenarios.md) — tier5 and tier7 test cases that verify flag injection
- [Run amplihack in Non-interactive Mode](../howto/run-in-noninteractive-mode.md) — CI configuration guide
- [Manage Tool Update Notifications](../howto/manage-tool-update-checks.md) — How the pre-launch update check interacts with the launch sequence
