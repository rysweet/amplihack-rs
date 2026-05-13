# `amplihack copilot` — Subprocess-Safe Defaults

**Issue:** [#621](https://github.com/rysweet/amplihack-rs/issues/621)
**Status:** Shipped
**Scope:** `crates/amplihack-cli` — `Copilot` subcommand only (Codex / Amplifier subcommands unchanged)

## Overview

`amplihack copilot` automatically detects when it is running as a delegated
subprocess (no controlling TTY, or invoked by another agent) and adjusts its
default behavior so that headless callers — engineer subprocesses, recipe-runner
agents, CI shell scripts — can write files, commit, and open pull requests
without permission-denied failures.

This eliminates the previously-required workaround in caller repositories of
appending `--allow-all-tools --allow-all-paths` to every Copilot CLI invocation
manually (see [Simard PR #1720](https://github.com/rysweet/Simard/pull/1720)).
All callers — including future agents and human shell invocations from CI —
now get correct sandbox behavior automatically.

## What "Subprocess-Safe Context" Means

A `Copilot` invocation is treated as **subprocess-safe** if **any** of the
following are true:

| Signal | Detection |
| --- | --- |
| Explicit user opt-in | `--subprocess-safe` flag passed |
| Delegated agent | `AMPLIHACK_AGENT_BINARY` env var is set to a non-empty value |
| Non-interactive marker | `AMPLIHACK_NONINTERACTIVE=1` is set |
| Headless I/O | Any of `stdin` / `stdout` / `stderr` is **not** a TTY |

Detection happens once at dispatch time. The resolved value is propagated to
all downstream code (including the docker launcher) so behavior is consistent
end-to-end.

> **Distinct from `is_noninteractive()`:** Subprocess-safe detection examines
> all three standard streams plus `AMPLIHACK_AGENT_BINARY`, while the older
> `is_noninteractive()` helper only examined `stdin`. Callers of
> `is_noninteractive()` keep their existing semantics; subprocess-safe is a
> separate, stricter signal scoped to the Copilot subcommand.

## Behavior Changes

When subprocess-safe context is active, `amplihack copilot` automatically:

1. **Injects `--allow-all-tools`** into the underlying `copilot` CLI argv.
2. **Injects `--allow-all-paths`** into the underlying `copilot` CLI argv.
3. **Defaults `--no-reflection` to ON** (suppresses reflection on a delegated
   session — a subprocess delegate has no value in running reflection on its
   own work).

When subprocess-safe context is **not** active (interactive TTY, no flag,
no agent-binary env), none of these granular flags are injected. The
preexisting default `--allow-all` injection (issue #303) is unaffected — see
[Layering with `--allow-all`](#layering-with---allow-all) below.

### Argv injection order

Injected flags are **prepended** before any user-supplied trailing args, so
duplicates passed by the caller take precedence (typical CLI semantics).

### Duplicate suppression

The injection is idempotent. If the user already passed any of the following,
no duplicate is added:

- `--allow-all-tools` → suppresses injection of `--allow-all-tools`
- `--allow-all-paths` → suppresses injection of `--allow-all-paths`
- `--allow-all` (broader) → suppresses injection of **both** granular flags

### Reflection precedence

`--reflection` (new opt-in) and `--no-reflection` are mutually exclusive
(enforced by clap `conflicts_with`). The effective decision uses this
precedence:

1. **`--reflection` passed** → reflection ON (overrides everything, including
   subprocess-safe).
2. **`--no-reflection` passed** → reflection OFF.
3. **Subprocess-safe context active (and no explicit reflection flag)** →
   reflection OFF (default flip).
4. **Otherwise** → reflection ON (preexisting default).

## Usage

### Headless / CI invocation (auto-detected)

```bash
# stdout/stderr piped — auto-detected as subprocess-safe
amplihack copilot -p "Fix the failing test in src/auth.rs" 2>&1 | tee log.txt
```

No flags required. The `copilot` CLI receives `--allow-all-tools` and
`--allow-all-paths` automatically; reflection is suppressed.

### Delegated agent invocation (env-detected)

```bash
# Caller sets AMPLIHACK_AGENT_BINARY to indicate this is a delegated subprocess
AMPLIHACK_AGENT_BINARY=copilot amplihack copilot -p "Implement the design spec at docs/SPEC.md"
```

Subprocess-safe defaults activate even on a TTY, because the env var indicates
this process is acting on behalf of a parent agent.

### Explicit opt-in (interactive shell)

```bash
# Force subprocess-safe defaults even at an interactive TTY
amplihack copilot --subprocess-safe -p "Refactor this module"
```

Useful when scripting against `amplihack copilot` from an interactive shell
and you want the headless defaults applied unconditionally.

### Opt back into reflection while subprocess-safe

```bash
# Subprocess-safe is auto-active (CI), but you want reflection ON anyway
amplihack copilot --reflection -p "Long-running investigation task"
```

The `--reflection` flag is the only way to re-enable reflection in a
subprocess-safe context. It overrides both the auto-detected default flip and
any propagated `--no-reflection`.

### Interactive shell, no overrides (unchanged behavior)

```bash
# At a real terminal, no env vars set — subprocess-safe NOT active
amplihack copilot -p "Help me explore this codebase"
```

The `copilot` CLI receives only the preexisting `--allow-all` default (from
issue #303). No granular `--allow-all-tools` / `--allow-all-paths` are
injected. Reflection is ON (preexisting default).

## Configuration Reference

### Flags on `amplihack copilot`

| Flag | Type | Default | Description |
| --- | --- | --- | --- |
| `--subprocess-safe` | bool | `false` | Force subprocess-safe defaults (even on TTY). Implies `--allow-all-tools`, `--allow-all-paths`, and `--no-reflection`. |
| `--reflection` | bool | `false` | Force reflection ON. Conflicts with `--no-reflection`. Overrides subprocess-safe default. |
| `--no-reflection` | bool | `false` | Force reflection OFF. Conflicts with `--reflection`. |

(All other flags — `--docker`, `--allow-all-paths`, etc. — behave as before.)

### Environment Variables

| Variable | Effect |
| --- | --- |
| `AMPLIHACK_AGENT_BINARY` | If set non-empty → triggers subprocess-safe context. (Set by parent agent runtimes — Claude Code, recipe-runner, Copilot CLI agent dispatch — to identify the active binary.) |
| `AMPLIHACK_NONINTERACTIVE` | If `=1` → triggers subprocess-safe context. |
| `AMPLIHACK_COPILOT_NO_ALLOW_ALL` | If `=1` → suppresses the preexisting `--allow-all` blanket injection (#303). **Not weakened** by subprocess-safe. (See [layering](#layering-with---allow-all) below.) |
| `RUST_LOG=debug` | Emits a `tracing::debug!` line documenting the resolved subprocess-safe + reflection decision and which signals fired. |

### Inspecting the decision

Run with `RUST_LOG=debug` to see the audit log:

```bash
RUST_LOG=debug amplihack copilot --subprocess-safe -p "test" 2>&1 | grep amplihack_cli
# DEBUG amplihack_cli::commands: copilot dispatch subprocess_safe_resolved=true
#   (signals: explicit_flag=true, agent_binary=None, all_streams_tty=false)
#   no_reflection_effective=true (reason: subprocess_safe default)
```

> **Note:** The exact `tracing::debug!` line format above is illustrative; the
> final field names and wording are determined during implementation (Step 7).
> The contract is that the resolved `subprocess_safe` decision, the signals
> that fired, and the effective `no_reflection` decision are all observable at
> `debug` level — not the precise string layout.

## Layering with `--allow-all`

`amplihack copilot` already injected the blanket `--allow-all` flag for every
invocation by default since [#303](https://github.com/rysweet/amplihack-rs/issues/303).
That behavior is **unchanged** by this feature.

When subprocess-safe is also active, the resulting `copilot` argv contains
**both** the blanket `--allow-all` and the granular `--allow-all-tools` /
`--allow-all-paths`. This is intentional, defense-in-depth, and accepted by
the `copilot` CLI without conflict. The granular flags satisfy the literal
contract of subprocess-safe (so callers can audit argv for the specific
tokens) without disturbing the broader `--allow-all` default.

| Context | `--allow-all` | `--allow-all-tools` | `--allow-all-paths` |
| --- | :---: | :---: | :---: |
| Interactive TTY (no flag, no env) | ✅ (preexisting #303) | ❌ | ❌ |
| Subprocess-safe active | ✅ (preexisting #303) | ✅ (new) | ✅ (new) |
| Subprocess-safe + `AMPLIHACK_COPILOT_NO_ALLOW_ALL=1` | ❌ (opt-out) | ✅ (new) | ✅ (new) |
| User passed `--allow-all` themselves | ✅ (user) | ❌ (suppressed by superset) | ❌ (suppressed by superset) |

## Docker Mode

`--subprocess-safe` is propagated through `build_docker_launcher_args`.
Resolution happens at the outer amplihack-cli layer **before** docker dispatch,
so the resolved decision (including any auto-detection result) is carried
into the container. No additional configuration is needed inside the docker
image.

```bash
# Auto-detection fires on the host; flag is propagated into the container
AMPLIHACK_AGENT_BINARY=copilot amplihack copilot --docker -p "Run the tests"
```

## Examples

### CI script that delegates to `amplihack copilot`

```yaml
# .github/workflows/agent.yml
- name: Run amplihack copilot agent
  env:
    AMPLIHACK_AGENT_BINARY: copilot   # Mark as delegated
    GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
  run: |
    amplihack copilot -p "Fix the issue described in $ISSUE_BODY"
    # No --allow-all-tools / --allow-all-paths needed — auto-injected.
    # No --no-reflection needed — auto-defaulted.
```

### Engineer subprocess (replaces Simard PR #1720 workaround)

Before:

```rust
// Simard engineer/launcher.rs (workaround)
let argv = vec![
    "amplihack", "copilot",
    "--allow-all-tools",   // <-- workaround
    "--allow-all-paths",   // <-- workaround
    "--no-reflection",     // <-- workaround
    "-p", &task,
];
```

After:

```rust
// Simard engineer/launcher.rs (cleanup)
std::env::set_var("AMPLIHACK_AGENT_BINARY", "copilot");
let argv = vec!["amplihack", "copilot", "-p", &task];
// All three flags now auto-applied by amplihack-rs.
```

> **Note:** The Simard workaround removal is a separate follow-up PR in that
> repository. The change in amplihack-rs is backward-compatible — existing
> callers that pass the granular flags explicitly continue to work (duplicate
> suppression handles them).
>
> **Edition footnote:** `std::env::set_var` is `unsafe` under the Rust 2024
> edition. The wrapping required at the call site (`unsafe { ... }` block,
> or a safer alternative such as setting the env var in the parent process
> before spawn) is determined by the consuming crate's edition — Simard's
> own toolchain dictates the exact form. amplihack-rs only reads the env
> var; it does not constrain how callers write it.

### Recipe-runner subprocess agent

```bash
# Inside a recipe step
amplihack recipe run my-workflow -c agent_binary=copilot
# Internally launches `amplihack copilot ...` with AMPLIHACK_AGENT_BINARY=copilot;
# subprocess-safe defaults activate automatically.
```

## Migration Guide

### For amplihack callers (Simard, recipes, CI scripts)

If your code currently appends `--allow-all-tools`, `--allow-all-paths`, or
`--no-reflection` to `amplihack copilot` invocations as a workaround:

1. **Set `AMPLIHACK_AGENT_BINARY=copilot`** before invoking, OR pass
   `--subprocess-safe` explicitly.
2. **Remove the workaround flags** from your argv (they are now redundant —
   though keeping them is harmless thanks to duplicate suppression).
3. **Verify** with `RUST_LOG=debug` that `subprocess_safe_resolved=true` and
   the granular flags appear in the launched `copilot` argv.

### For interactive `amplihack copilot` users

**No action required.** Interactive TTY behavior is unchanged. The new
defaults only fire when at least one subprocess-safe signal is present.

### For users who want the new flags in interactive shells

Pass `--subprocess-safe` (or set `AMPLIHACK_NONINTERACTIVE=1` / use a non-TTY
shell wrapper).

## Out of Scope

This feature does **not**:

- Modify the `copilot` CLI itself (no upstream changes).
- Modify the Codex or Amplifier subcommands (Copilot subcommand only;
  trivial extension if requested in a follow-up).
- Modify the auto-mode helper path (`commands/auto_mode/helpers.rs:214`),
  which already injects `--allow-all` separately. Documented as a follow-up.
- Introduce any new sandbox / permission models — it composes existing
  `copilot` CLI flags.
- Add telemetry beyond a single `tracing::debug!` decision audit at the
  dispatch boundary.

## Security Considerations

- **Zero attack-surface delta.** The preexisting `--allow-all` default (#303)
  already runs `copilot` with full permissions in every context. Subprocess-safe
  adds redundant granular flags only in subprocess contexts.
- **Trust model unchanged.** Anyone who can set `AMPLIHACK_AGENT_BINARY` or
  redirect stdio already controls process startup; subprocess-safe inherits
  that trust posture, never escalates it.
- **Reflection auto-disable is a safety improvement.** Prevents nested
  infinite recursion when amplihack invokes itself (the bug that motivated
  issue #621).
- **`AMPLIHACK_COPILOT_NO_ALLOW_ALL=1` opt-out is preserved.** The blanket
  `--allow-all` opt-out continues to suppress the broader flag even when
  subprocess-safe is active. (The granular `--allow-all-tools` /
  `--allow-all-paths` are still injected — they are the explicit contract of
  subprocess-safe — but the broader `--allow-all` is suppressed if the user
  has opted out.)
- **No `unsafe` blocks; no `unwrap()` on env reads; no runtime-derived argv
  tokens.** The injected flag tokens are compile-time `&'static str` literals.

## See Also

- [`COPILOT_CLI.md`](COPILOT_CLI.md) — Full Copilot integration overview
- [`AUTOMODE_SAFETY.md`](AUTOMODE_SAFETY.md) — Automode safety guide
- [Issue #621](https://github.com/rysweet/amplihack-rs/issues/621) — Source
  issue
- [Issue #303](https://github.com/rysweet/amplihack-rs/issues/303) — Preexisting
  `--allow-all` default
- [Simard PR #1720](https://github.com/rysweet/Simard/pull/1720) — Original
  workaround (now removable)
