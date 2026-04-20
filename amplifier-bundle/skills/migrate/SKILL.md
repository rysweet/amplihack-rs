---
name: amplihack:migrate
description: Move the active amplihack CLI session (Copilot/Claude/Amplifier) to a fresh azlin-managed VM, preserving auth, plugins, skills, plan.md, todos, and conversation history. Resumes the session in a detached tmux on the destination host.
version: 1.0.0
author: amplihack
activation_keywords:
  - amplihack:migrate
  - migrate session
  - move session to host
  - resume on azlin
min_tokens: 800
max_tokens: 2000
---

# Migrate Session Skill

Move the currently running amplihack CLI session to a fresh azlin-managed VM
and resume it there in a detached tmux. One command end-to-end.

> ⚠️ Do not activate this skill on any natural-language prompt that merely
> contains the word "migrate" (e.g. "migrate from Python", "data migration",
> "memory backend migration"). Activate only when the user explicitly types
> the `/amplihack:migrate` slash command or asks to "move the current session
> to <host>".

## When to Use

- Local host is low on disk/RAM under heavy orchestrator load
- You want to move long-running OODA work to a beefier Azure VM
- You need to hand off the session to an unattended cloud host so the laptop
  can sleep
- You are switching from a constrained dev container to a full Linux VM

## Usage

```text
/amplihack:migrate <hostname>
/amplihack:migrate <hostname> --session <session-id>
```

The destination `<hostname>` must be an azlin-managed VM (reachable via
`azlin connect` / `azlin cp`).

## What It Does

1. Detects the active CLI session via env var → newest session-state dir.
2. Bootstraps the destination (idempotent): node, npm, gh, uv, copilot,
   amplihack — skips tools already installed at a matching version.
3. Builds a selective `zstd`-compressed tarball containing only:
   - `~/.config/`, `~/.copilot/skills/`, `~/.amplihack/`, `~/.simard/`,
     `~/.ssh/`
   - The **active** `~/.copilot/session-state/<id>/` directory only
4. Ships the tarball to the destination via `azlin cp`, extracts it.
5. Verifies `gh auth`, `copilot --version`, and session-state integrity.
6. Runs a final delta `rsync` of the active session-state to capture
   events written during the transfer.
7. Starts the CLI in a detached tmux on the destination:
   - copilot: `copilot --resume <id>`
   - claude: `claude --resume <id>` (best-effort)
   - amplifier: prints manual-attach instructions (v1)

Prints an `azlin connect -y <hostname>:session-<id>` command you can paste on
your laptop to attach.

## What Is Not Migrated

- `~/src/*` source trees (fresh-clone on the new host instead)
- Caches: `**/target/`, `**/.venv/`, `**/node_modules/`, `~/.cache/`,
  `**/__pycache__/`
- Inactive sessions under `~/.copilot/session-state/`
- Log files > 10 MB

## Security Note

Unlike the `remote-work` skill, this migration **intentionally** copies
credentials (`~/.ssh`, `~/.config/gh/hosts.yml`) to the destination. The
skill prints a warning before transfer. Only use with trusted destinations.

## Invocation

The slash command resolves to this skill's helper script, which is staged by
the bundle installer into `~/.amplihack/.claude/skills/migrate/scripts/`.

```bash
bash "$AMPLIHACK_HOME/.claude/skills/migrate/scripts/migrate.sh" <hostname> [--session <id>]
```

The script is idempotent: re-running against the same destination skips
already-installed toolchain, overwrites the active session-state on the
destination (source is authoritative until resume), and re-extracts
`~/.config/` / `~/.amplihack/` (cheap; ensures freshness).

## See Also

- Full documentation: [docs/skills/migrate.md](../../../docs/skills/migrate.md)
- `azlin` skill — destination provisioning and transport
- `remote-work` skill — related but does not copy credentials
