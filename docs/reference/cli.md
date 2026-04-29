# amplihack CLI Reference

Complete command-line reference for the `amplihack` top-level command.

## Contents

- [Synopsis](#synopsis)
- [Global Flags](#global-flags)
- [Subcommands](#subcommands)
- [Exit Codes](#exit-codes)
- [Environment Variables](#environment-variables)
- [Examples](#examples)

---

## Synopsis

```
amplihack [--version] [--help] <subcommand> [<args>]
```

Running `amplihack` with no subcommand launches Claude Code directly. In
user-facing docs, prefer the explicit `amplihack claude` form;
`amplihack launch` remains a compatibility alias.

---

## Global Flags

These flags are accepted before any subcommand.

| Flag           | Description                                       |
| -------------- | ------------------------------------------------- |
| `--version`    | Print `amplihack <version>` to stdout and exit 0. |
| `--help`, `-h` | Print a brief usage summary and exit 0.           |

### `--version`

Prints the installed version string and exits immediately. No network requests, no configuration loading.

```bash
amplihack --version
# amplihack 0.9.2
```

The version string comes from the `__version__` attribute in `amplihack/__init__.py`, which is set from `pyproject.toml` at build time. It follows [Semantic Versioning](https://semver.org/).

---

## Subcommands

| Subcommand   | Description                                                                                 |
| ------------ | ------------------------------------------------------------------------------------------- |
| `version`    | Show amplihack version.                                                                     |
| `install`    | Install amplihack agents and tools to `~/.claude`.                                          |
| `uninstall`  | Remove amplihack agents and tools from `~/.claude`.                                         |
| `update`     | Self-update the amplihack binary, then automatically run `install` to refresh framework assets. Pass `--skip-install` (alias `--no-install`) for a binary-only update. |
| `claude`     | Launch Claude Code. Preferred explicit launcher in user-facing docs.                        |
| `launch`     | Compatibility alias for `claude`; `amplihack` with no subcommand also launches Claude Code. |
| `RustyClawd` | Launch RustyClawd (Rust implementation).                                                    |
| `copilot`    | Launch GitHub Copilot CLI.                                                                  |
| `codex`      | Launch OpenAI Codex CLI.                                                                    |
| `amplifier`  | Launch Microsoft Amplifier with amplihack bundle.                                           |
| `uvx-help`   | Get help with UVX deployment.                                                               |
| `plugin`     | Install, uninstall, and list amplihack plugins.                                             |
| `memory`     | Manage the amplihack memory backend.                                                        |
| `new`        | Generate a new goal-seeking agent.                                                          |
| `recipe`     | Run, list, validate, and inspect workflow recipes.                                          |
| `mode`       | Claude installation mode commands.                                                          |
| `fleet`      | Manage coding agents across VMs.                                                            |

See the documentation for each subcommand:

- [Recipe CLI Reference](./recipe-cli-reference.md)
- [Memory CLI Reference](./memory-cli-reference.md)
- [Plugin CLI Reference](#)

### `update`

Self-updates the `amplihack` binary by downloading the latest release from
GitHub (with SHA-256 verification), atomically replacing the running executable,
and then **automatically running `amplihack install`** to refresh the framework
assets staged under `~/.amplihack/.claude` so the on-disk agents, hooks, and
prompts match the newly installed binary.

**Synopsis**

```
amplihack update [--skip-install | --no-install]
```

**Flags**

| Flag             | Description                                                                                                                       |
| ---------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `--skip-install` | Skip the automatic post-update `install` step (binary-only update — legacy behavior). Alias: `--no-install`.                      |

**Behavior**

1. Check GitHub for a newer release. If none, exit with a "already at latest"
   message.
2. Download the platform-specific archive and verify its SHA-256, then
   atomically replace the running binary.
3. **Unless `--skip-install` was passed**, invoke the same code path as
   `amplihack install` (in-process — no subprocess) to re-stage framework
   assets to `~/.amplihack/.claude`. This is non-interactive.

If step 2 fails, the install step does **not** run and the original binary
remains in place. If step 3 fails, the new binary is already installed; you
can re-run `amplihack install` manually to retry the asset refresh.

**Examples**

```bash
# Default: update binary and refresh framework assets in one step
amplihack update

# Binary-only update (preserve old framework assets on disk)
amplihack update --skip-install

# Equivalent alias
amplihack update --no-install

# Manually refresh framework assets (e.g. after a --skip-install update,
# or to recover from a failed post-update install step)
amplihack install
```

**Why install runs automatically**

A binary-only update can leave the on-disk agents, hooks, prompts, and recipes
out of sync with the new binary, producing confusing behavior (missing skills,
stale prompts, hook signature mismatches). Running `install` immediately after
a successful update keeps the binary and the staged framework consistent.

**Startup-prompt updates**

When `amplihack` prompts you to update at startup ("Update now? [y/N]") and you
accept, the same flow runs: binary swap followed by automatic `install`. There
is no way to pass `--skip-install` through the startup prompt — answer `N` and
run `amplihack update --skip-install` manually if you want the legacy behavior
in that situation.

---

## Exit Codes

| Code | Meaning                                                               |
| ---- | --------------------------------------------------------------------- |
| `0`  | Completed successfully (or `--version` / `--help` printed).           |
| `1`  | User error (bad argument, missing config). Stderr contains a message. |
| `2`  | Internal error. Stderr contains a traceback.                          |

---

## Environment Variables

These variables are read at startup. All are optional.

| Variable                   | Default         | Effect                                                                                                                                                                                                                                                        |
| -------------------------- | --------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `AMPLIHACK_AGENT_BINARY`   | set by launcher | Identifies the active tool in the child process environment. Set automatically to `claude`, `copilot`, `codex`, or `amplifier` by the corresponding launcher before spawning the subprocess. Read by skills and hooks to adapt behaviour to the active agent. |
| `AMPLIHACK_DEBUG`          | unset           | Set to `true` to print debug messages during CLI execution.                                                                                                                                                                                                   |
| `AMPLIHACK_ENABLE_BLARIFY` | unset           | Set to `1` to enable blarify code-graph indexing.                                                                                                                                                                                                             |
| `AMPLIHACK_HOME`           | `~/.amplihack`  | Override the root directory for staged framework files and runtime data. Set automatically by each launcher when not already present in the environment; an existing value is always preserved.                                                               |
| `AMPLIHACK_LOG_LEVEL`      | `WARNING`       | Python logging level for the launcher (`DEBUG`, `INFO`, `WARNING`, `ERROR`).                                                                                                                                                                                  |

---

## Examples

### Check the installed version

```bash
amplihack --version
# amplihack 0.9.2
```

### Launch an interactive Claude session

Prefer `amplihack claude` in user-facing docs. `amplihack launch` remains
supported as a compatibility alias.

```bash
amplihack claude
# or simply:
amplihack
# compatibility alias:
amplihack launch
```

### Launch an interactive Copilot session

```bash
amplihack copilot
```

### Run a workflow recipe non-interactively

```bash
amplihack recipe run default-workflow \
  --context '{"task_description": "Add input validation to the login endpoint"}'
```

### Enable blarify code indexing for a session

```bash
AMPLIHACK_ENABLE_BLARIFY=1 amplihack claude
```

---

## See Also

- [Getting Started](../tutorials/amplihack-tutorial.md)
- [Recipe CLI Reference](./recipe-cli-reference.md)
- [Blarify Code Indexing](../howto/enable-blarify.md)
- [Configuration Guide](#)
