# Migrate Session Skill (`/amplihack:migrate`)

Move the currently running amplihack CLI session (Copilot, Claude Code, Amplifier)
to a fresh azlin-managed Azure VM and resume it there in a detached tmux. One
command end-to-end.

## Overview

Mid-session host migration used to be a manual ~15-step procedure (bootstrap
toolchain → selective tarball → `bastion cp` → extract → delta sync → resume).
The `/amplihack:migrate <hostname>` skill collapses that into a single command.

Typical use cases:

- Local host running out of disk / RAM under heavy orchestrator load
- Moving long-running work to a beefier Azure VM
- Handing off the session to an unattended cloud host so the laptop can sleep
- Switching from a constrained dev container to a full Linux VM

## Usage

```text
> /amplihack:migrate ia2
[migrate] Detected active session: b127f92a-eb5c-4a3a-9a21-0e74ca5a6f28 (copilot)
[migrate] Session-state dir: /home/me/.copilot/session-state/b127f92a-… (6 MB)
[migrate] This migration will copy credentials (.ssh, .config/gh/hosts.yml) to ia2.
[migrate] Bootstrapping ia2 (node, npm, gh, uv, copilot, amplihack)… ok
[migrate] Building selective tarball → /tmp/amplihack-migrate-….tar.zst
[migrate] Tarball size: 421 MB
[migrate] Checking destination disk availability…
[migrate] Copying to ia2:/tmp/amplihack-migrate.tar.zst … ok
[migrate] Extracting on ia2 … ok
[migrate] Verifying destination: gh auth + copilot + session-state… ok
[migrate] Final delta sync of session-state … ok
[migrate] Starting tmux 'session-b127f92a-…' on ia2: copilot --resume b127f92a-…
[migrate] Migration complete.
✓ Session resumed on ia2 in tmux 'session-b127f92a-…'.
  Attach with: azlin connect -y ia2:session-b127f92a-…
```

### Options

| Option            | Purpose                                                    |
| ----------------- | ---------------------------------------------------------- |
| `<hostname>`      | Required. Destination azlin VM hostname.                   |
| `--session <id>`  | Override auto-detection; use a specific session id.        |
| `--dry-run`       | Print what would happen; do not transfer or resume.        |

## What Gets Migrated

The tarball contains exactly these paths:

- `~/.config/` (includes `gh/hosts.yml` for GitHub CLI auth)
- `~/.copilot/skills/`
- `~/.amplihack/`
- `~/.simard/`
- `~/.ssh/`
- **Only the active** `~/.copilot/session-state/<id>/` (or analogous for claude)

Exclusions (pruned from the tarball):

- `~/src/*` source trees
- `**/target/`, `**/.venv/`, `**/node_modules/`, `**/__pycache__/`
- `~/.cache/`
- Inactive sessions under `session-state/`
- Files tagged as `CACHEDIR.TAG` by `--exclude-caches-under`

## What Is Not Migrated

- **Source code checkouts** (`~/src/*`) — fresh-clone on the new host instead.
- **Bidirectional sync / live mirroring** — single-shot migration only.
- **Non-azlin destinations** (raw SSH, other clouds) — may come in v2.

## How It Works

### 1. Session detection

Cascade (first hit wins):

1. `--session <id>` flag
2. Env var (`$COPILOT_SESSION_ID`, `$CLAUDE_SESSION_ID`)
3. Newest-mtime directory under the CLI's session-state root
4. Error with clear message if none found

The CLI type is inferred from `$AMPLIHACK_AGENT_BINARY`, falling back to a
parent-process-chain scan.

### 2. Destination bootstrap (idempotent)

`bootstrap-dest.sh` is streamed over `azlin connect` and installs any missing
toolchain: node (>= 18), gh, uv, @github/copilot, amplihack. Already-installed
tools are skipped via `command -v <tool>` guards.

### 3. Selective tarball

`tar --use-compress-program=zstd` with the include/exclude lists above. The
`zstd` codec gives ~3× faster compression than gzip at comparable ratios.

### 4. Pre-flight disk check

The destination must have at least **2× the tarball size** free. If not, the
skill aborts and preserves the local tarball for manual recovery.

### 5. Ship → extract

`azlin cp` ships the tarball; the destination extracts via
`tar --use-compress-program=unzstd -xpf … -C /` and removes the tarball.

### 6. Verification

On the destination:

- `gh auth status` — warns (non-fatal) if not authenticated
- `<cli> --version` — must succeed
- Session-state dir must exist

### 7. Final delta sync

An `rsync -a --delete` over the azlin SSH transport captures events written to
the active session-state during the tarball transfer. This is the second of
the **two-pass sync** design.

Accepted risk: events written during the final ~1–2s rsync window can still be
lost. There is no source-side event suspension.

### 8. Resume in detached tmux

The destination launches `copilot --resume <id>` (or `claude --resume <id>`)
inside a new detached tmux session named `session-<id>`. The skill prints the
`azlin connect -y <host>:<tmux-name>` command for the user to attach.

For the Amplifier CLI, resume is currently TBD — the skill migrates the
filesystem and prints manual-attach instructions instead of failing.

## Failure Modes

| Failure                                              | Handling                                                      |
| ---------------------------------------------------- | ------------------------------------------------------------- |
| `azlin` not installed on source                      | Abort with install hint (source dep check)                    |
| Destination unreachable                              | Surface `azlin connect` stderr; no partial state              |
| `gh auth` fails post-transfer                        | Warn but continue (user can re-auth on destination)           |
| Insufficient disk on destination                     | Abort before ship; preserve local tarball                     |
| Bootstrap (`apt`/`uv`/`npm`) failure                 | Abort; print remediation; tarball not built yet               |
| `tar` fails                                          | Abort; cleanup                                                |
| Delta rsync fails                                    | Warn; do not abort (initial tarball already transferred)      |
| `tmux new-session` already exists on destination     | Treated as success; user attaches to existing session         |

## Manual Recovery

If the skill aborts mid-way, you can finish manually:

```bash
# 1. ship whatever tarball exists in /tmp/
azlin cp /tmp/amplihack-migrate-*.tar.zst <host>:/tmp/

# 2. extract on destination
azlin connect -y <host> -- tar --use-compress-program=unzstd -xpf /tmp/amplihack-migrate-*.tar.zst -C /

# 3. start the session manually
azlin connect -y <host>:session-<id> -- copilot --resume <id>
```

## Security

This skill **intentionally** copies credentials (`~/.ssh`, `~/.config/gh/hosts.yml`)
to the destination. A warning is printed before transfer. Only migrate to hosts
you fully trust (e.g., your own azlin-managed VMs). Do not use this skill with
shared or untrusted destinations.

## Scope Boundaries

**In scope (v1)**:

- Copilot migration end-to-end
- Claude Code migration best-effort (resume flag verified at runtime)
- Amplifier migration of filesystem only (resume step TBD)
- azlin-only destinations

**Not in v1**:

- `~/src/*` source-tree migration
- Bidirectional sync / live mirroring
- Raw SSH / non-azlin destinations
- Source-side cleanup after successful migration

## See Also

- **Remote work skill** — related skill that does **not** copy credentials.
- [azlin](https://github.com/rysweet/azlin) — destination provisioning and transport.
