# Remote Sessions CLI Reference

`amplihack remote` runs amplihack work on Azure VMs through the native Rust
`amplihack-remote` crate. It supports one-shot remote execution and detached
tmux-backed sessions that can continue after the local terminal disconnects.

This command is implemented by the Rust CLI. The historical Python remote tool
is not installed or required.

## Contents

- [Command overview](#command-overview)
- [amplihack remote exec](#amplihack-remote-exec)
- [amplihack remote list](#amplihack-remote-list)
- [amplihack remote start](#amplihack-remote-start)
- [amplihack remote output](#amplihack-remote-output)
- [amplihack remote kill](#amplihack-remote-kill)
- [amplihack remote status](#amplihack-remote-status)
- [Configuration](#configuration)
- [State file](#state-file)
- [Exit codes](#exit-codes)
- [Troubleshooting](#troubleshooting)

## Command overview

```bash
amplihack remote <subcommand> [options] [arguments]
```

| Subcommand | Purpose |
| ---------- | ------- |
| `exec` | Run one amplihack command synchronously on a remote VM and integrate results locally |
| `list` | List tracked detached sessions |
| `start` | Start one or more detached remote sessions |
| `output` | Capture output from a detached tmux session |
| `kill` | Terminate a detached session and release pool capacity |
| `status` | Show VM pool and session counts |

No other `remote` subcommands are part of the supported interface.

## `amplihack remote exec`

Run one amplihack command on a remote Azure VM, wait for it to finish, retrieve
logs and git state, integrate results into the local repository, and clean up the
VM unless cleanup is disabled or the VM is preserved for debugging.

```bash
amplihack remote exec <COMMAND> <PROMPT> [OPTIONS] [-- <AZLIN_ARGS>...]
```

### Arguments

| Argument | Values | Description |
| -------- | ------ | ----------- |
| `COMMAND` | `auto`, `ultrathink`, `analyze`, `fix` | Amplihack command mode to run remotely |
| `PROMPT` | non-empty string | Task prompt passed to the remote amplihack command |
| `AZLIN_ARGS` | any azlin arguments | Extra arguments forwarded to azlin after `--` |

### Options

| Option | Default | Description |
| ------ | ------- | ----------- |
| `--max-turns <N>` | `10` | Maximum agent turns. Must be from `1` through `50`. |
| `--vm-size <SKU>` | `Standard_D2s_v3` | Azure VM SKU for synchronous execution. |
| `--vm-name <NAME>` | none | Reuse a specific VM. |
| `--keep-vm` | `false` | Keep the VM after execution. |
| `--no-reuse` | `false` | Always provision a fresh VM. |
| `--timeout <MINUTES>` | `120` | Maximum remote execution time. Must be from `5` through `480`. |
| `--region <REGION>` | azlin default | Azure region for provisioning. |
| `--port <PORT>` | none | Reuse an existing local bastion tunnel port. |

### Behavior

`ANTHROPIC_API_KEY` must be present in the local environment. The key is copied
into the remote command environment so the agent process can run on the VM. The
CLI validates this before packaging the repository or provisioning a VM.

`exec` runs the seven-step synchronous workflow:

1. Validate that the current directory is a git repository and credentials are present.
2. Package repository context and scan for secrets.
3. Provision or reuse a VM through azlin.
4. Transfer the context archive to the VM.
5. Run `amplihack claude --<COMMAND> --max-turns <N> -- -p "<PROMPT>"` remotely.
6. Retrieve logs and git state, then integrate results locally.
7. Clean up the VM unless `--keep-vm` is set.

If the remote command exits non-zero without timing out, the VM is preserved for
debugging even when `--keep-vm` was not passed. Timed-out executions follow the
normal cleanup rule.

### Examples

```bash
# Run the standard workflow remotely.
amplihack remote exec auto "implement user authentication"

# Give a complex task more turns.
amplihack remote exec ultrathink "analyze issue #536 and submit a PR" --max-turns 30

# Preserve a named VM for inspection after the run.
amplihack remote exec fix "repair the failing install smoke test" \
  --vm-name amplihack-azureuser-debug \
  --keep-vm

# Forward azlin-specific arguments after --.
amplihack remote exec analyze "inspect quota usage" -- --subscription "Engineering"
```

## `amplihack remote list`

List detached remote sessions tracked in the local remote state file.

```bash
amplihack remote list [OPTIONS]
```

### Options

| Option | Default | Description |
| ------ | ------- | ----------- |
| `--status <STATUS>` | all statuses | Filter by `pending`, `running`, `completed`, `failed`, or `killed`. |
| `--json` | `false` | Print sessions as formatted JSON. |

### Human output

```text
SESSION                        VM                               STATUS     AGE      PROMPT
------------------------------------------------------------------------------------------------------------------------
sess-20260502-203014-4f2a      amplihack-azureuser-20260502     running    12m      implement remote CLI parser
sess-20260502-195500-9b17      amplihack-azureuser-20260502     completed  1h       update API documentation

Total: 2 session(s)
```

If no sessions match, the command prints:

```text
No remote sessions found.
```

### JSON output

`--json` prints an array of session objects:

```json
[
  {
    "session_id": "sess-20260502-203014-4f2a",
    "vm_name": "amplihack-azureuser-20260502",
    "workspace": "/workspace/sess-20260502-203014-4f2a",
    "tmux_session": "sess-20260502-203014-4f2a",
    "prompt": "implement remote CLI parser",
    "command": "auto",
    "max_turns": 10,
    "status": "running",
    "memory_mb": 32768,
    "created_at": "2026-05-02T20:30:14Z",
    "started_at": "2026-05-02T20:31:02Z",
    "completed_at": null,
    "exit_code": null
  }
]
```

## `amplihack remote start`

Start one or more prompts as detached tmux sessions on pooled Azure VMs.

```bash
amplihack remote start [OPTIONS] <PROMPT>...
```

### Arguments

| Argument | Description |
| -------- | ----------- |
| `PROMPT` | One or more non-empty task prompts. Quote prompts that contain spaces. |

### Options

| Option | Default | Description |
| ------ | ------- | ----------- |
| `--command <MODE>` | `auto` | Command mode: `auto`, `ultrathink`, `analyze`, or `fix`. |
| `--max-turns <N>` | `10` | Maximum turns for each detached session. |
| `--size <TIER>` | `l` | Pool VM tier: `s`, `m`, `l`, or `xl`. |
| `--region <REGION>` | `AZURE_REGION`, then `eastus` | Azure region for pool allocation. |
| `--port <PORT>` | none | Reuse an existing local bastion tunnel port. |

### VM tiers

Each detached session receives a 32 GB Node heap by setting
`NODE_OPTIONS=--max-old-space-size=32768` inside the tmux session.

| Tier | Azure VM SKU | Session capacity | Intended use |
| ---- | ------------ | ---------------- | ------------ |
| `s` | `Standard_D8s_v3` | 1 | One isolated task |
| `m` | `Standard_E8s_v5` | 2 | Two normal tasks |
| `l` | `Standard_E16s_v5` | 4 | Default parallel work |
| `xl` | `Standard_E32s_v5` | 8 | Large batches of independent tasks |

### Behavior

For each prompt, `start` packages the current repository, allocates a VM with
available capacity or provisions a new VM, transfers context, launches a tmux
session, marks the session `running`, and prints the session ID.

`ANTHROPIC_API_KEY` must be present in the local environment. The key is injected
into the remote tmux command environment for the detached agent process. The CLI
validates this before packaging the repository or allocating VM capacity.

### Examples

```bash
# Start one detached session.
amplihack remote start "implement the remote list command"

# Start three sessions on the default L-size pool.
amplihack remote start \
  "add parser tests for remote exec" \
  "write remote API docs" \
  "remove stale Python remote files"

# Run a deeper analysis in a specific region.
amplihack remote start \
  --command ultrathink \
  --max-turns 30 \
  --size xl \
  --region eastus \
  "audit issue #536 for missing parity"
```

## `amplihack remote output`

Capture output from a detached session's tmux pane.

```bash
amplihack remote output <SESSION_ID> [OPTIONS]
```

### Arguments

| Argument | Description |
| -------- | ----------- |
| `SESSION_ID` | Session ID from `amplihack remote start` or `amplihack remote list`. |

### Options

| Option | Default | Description |
| ------ | ------- | ----------- |
| `--lines <N>` | `100` | Number of lines to capture from the tmux pane. |
| `--follow` | `false` | Refresh output every 5 seconds until interrupted. |

### Output

```text
=== Session: sess-20260502-203014-4f2a ===
Status: running
VM: amplihack-azureuser-20260502
Prompt: implement remote CLI parser
================================================================================
Step 5: Implement the Solution
  Updating crates/amplihack-cli/src/cli_subcommands.rs
  Adding parser coverage for remote start
```

If `--follow` is set, the command clears the terminal before each refresh and
prints:

```text
[Following output... Press Ctrl+C to stop]
```

If the session ID is unknown, the command exits with code `3` and suggests
running `amplihack remote list`.

## `amplihack remote kill`

Terminate a detached session and release its VM pool slot.

```bash
amplihack remote kill <SESSION_ID> [OPTIONS]
```

### Options

| Option | Default | Description |
| ------ | ------- | ----------- |
| `--force` | `false` | Mark the session killed and release capacity even if the remote tmux kill command fails. |

### Behavior

`kill` runs `tmux kill-session -t <SESSION_ID>` through
`azlin connect <VM_NAME>`, marks the session `killed`, records its completion
time, and releases the session from the VM pool.

Without `--force`, a failed remote tmux kill stops the command and leaves state
unchanged. With `--force`, the command prints a warning and continues updating
local state.

### Examples

```bash
# Gracefully terminate a session.
amplihack remote kill sess-20260502-203014-4f2a

# Release local state even if the VM is unreachable.
amplihack remote kill sess-20260502-203014-4f2a --force
```

## `amplihack remote status`

Show VM pool utilization and detached session counts.

```bash
amplihack remote status [OPTIONS]
```

### Options

| Option | Default | Description |
| ------ | ------- | ----------- |
| `--json` | `false` | Print status as formatted JSON. |

### Human output

```text
=== Remote Session Pool Status ===

VMs: 1 total
  amplihack-azureuser-20260502 (Standard_E16s_v5, eastus)
    Sessions: 2/4 (50% capacity)
      - sess-20260502-203014-4f2a (running)
      - sess-20260502-203500-8a1c (running)

Sessions: 2 total
  Running: 2
  Completed: 0
  Failed: 0
  Killed: 0
  Pending: 0
```

### JSON output

```json
{
  "pool": {
    "total_vms": 1,
    "total_capacity": 4,
    "active_sessions": 2,
    "available_capacity": 2,
    "vms": [
      {
        "name": "amplihack-azureuser-20260502",
        "size": "Standard_E16s_v5",
        "region": "eastus",
        "capacity": 4,
        "active_sessions": 2,
        "available_capacity": 2
      }
    ]
  },
  "sessions": {
    "running": 2,
    "completed": 0,
    "failed": 0,
    "killed": 0,
    "pending": 0
  },
  "total_sessions": 2
}
```

## Configuration

### Required tools

| Tool | Purpose |
| ---- | ------- |
| `azlin` | Provision, connect to, and destroy Azure VMs |
| `az` | Azure authentication used by azlin |
| `tmux` | Detached session host on each remote VM |
| `amplihack` | Local and remote CLI binary |

### Environment variables

| Variable | Required | Default | Description |
| -------- | -------- | ------- | ----------- |
| `ANTHROPIC_API_KEY` | yes, for `exec` and `start` | none | API key injected into remote agent processes. |
| `AZURE_REGION` | no | `eastus` for `start`; azlin default for `exec` | Default Azure region when `--region` is not supplied. |
| `AMPLIHACK_REMOTE_STATE` | no | `~/.amplihack/remote-state.json` | Path to the remote state file. |
| `NODE_OPTIONS` | no | set remotely to `--max-old-space-size=32768` for detached sessions | Node heap size for remote agent processes. |

### Secret scanning

Remote execution packages the current repository before transfer. Secret scanning
runs before packaging and aborts if potential secrets are found. Remove the
secrets from the repository or ignore files before retrying.

## State file

By default, remote state is stored at:

```text
~/.amplihack/remote-state.json
```

The state file tracks both sessions and pooled VMs:

```json
{
  "sessions": {
    "sess-20260502-203014-4f2a": {
      "session_id": "sess-20260502-203014-4f2a",
      "vm_name": "amplihack-azureuser-20260502",
      "workspace": "/workspace/sess-20260502-203014-4f2a",
      "tmux_session": "sess-20260502-203014-4f2a",
      "prompt": "implement remote CLI parser",
      "command": "auto",
      "max_turns": 10,
      "status": "running",
      "memory_mb": 32768,
      "created_at": "2026-05-02T20:30:14Z",
      "started_at": "2026-05-02T20:31:02Z",
      "completed_at": null,
      "exit_code": null
    }
  },
  "vm_pool": {
    "amplihack-azureuser-20260502": {
      "vm": {
        "name": "amplihack-azureuser-20260502",
        "size": "Standard_E16s_v5",
        "region": "eastus"
      },
      "capacity": 4,
      "active_sessions": ["sess-20260502-203014-4f2a"],
      "region": "eastus"
    }
  }
}
```

Detached session state includes `memory_mb`. Sessions launched by `remote start`
record `32768`, matching the 32 GB Node heap set with
`NODE_OPTIONS=--max-old-space-size=32768`.

State writes use an advisory lock so concurrent `remote` commands do not corrupt
the JSON file.

## Exit codes

| Code | Meaning |
| ---- | ------- |
| `0` | Success |
| `1` | Runtime error, validation error, provisioning error, transfer error, execution error, or integration error |
| `2` | CLI parse error from Clap |
| `3` | Session ID not found for `output` or `kill` |
| `130` | Interrupted by Ctrl+C |

For `exec`, a completed remote process that exits non-zero returns the remote
process exit code.

## Troubleshooting

### `ANTHROPIC_API_KEY` is missing

`exec` and `start` require the API key before packaging repositories or
provisioning VMs:

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
amplihack remote exec auto "write tests for the remote API"
amplihack remote start "write tests for the remote API"
```

### Session is stuck in `pending`

Check output first:

```bash
amplihack remote output sess-20260502-203014-4f2a --lines 200
```

If the session never starts and the VM is reachable, terminate it and restart
with the same prompt:

```bash
amplihack remote kill sess-20260502-203014-4f2a --force
amplihack remote start "write tests for the remote API"
```

### VM is unreachable

Confirm the VM exists and azlin can connect:

```bash
azlin list
azlin connect amplihack-azureuser-20260502 "tmux ls"
```

If the VM was deleted outside amplihack, use `--force` to release the local
session and pool state:

```bash
amplihack remote kill sess-20260502-203014-4f2a --force
```

### State file is invalid JSON

Move the state file aside and rebuild state from active sessions manually:

```bash
mv ~/.amplihack/remote-state.json ~/.amplihack/remote-state.json.bak
amplihack remote status
```

Detached sessions already running on VMs continue running, but untracked sessions
will not appear in `amplihack remote list` until recreated in state.
