# Remote Sessions Tutorial

This tutorial shows how to use `amplihack remote` to run work on Azure VMs,
monitor detached sessions, and collect results from long-running agent tasks.

For complete command details, see the [Remote Sessions CLI Reference](./CLI_REFERENCE.md).
For the Rust library API, see the [amplihack-remote API reference](../reference/amplihack-remote-api.md).

## Prerequisites

Before starting:

1. Authenticate Azure.

   ```bash
   az login
   az account set --subscription "Engineering"
   ```

2. Confirm azlin can reach Azure.

   ```bash
   azlin list
   ```

3. Export the API key used by remote agent processes.

   ```bash
   export ANTHROPIC_API_KEY="sk-ant-..."
   ```

4. Work from a git repository.

   ```bash
   git status --short
   ```

## Tutorial 1: Run a one-shot remote command

Use `exec` when you want one remote run to finish before the command returns.

### Step 1: Start the remote run

```bash
amplihack remote exec auto "add parser tests for amplihack remote start"
```

Expected progress:

```text
[1/7] Validating environment...
[2/7] Packaging context...
[3/7] Provisioning VM...
[4/7] Transferring context...
[5/7] Executing remote command...
[6/7] Retrieving results...
[7/7] Cleaning up...
```

### Step 2: Give complex work more turns

```bash
amplihack remote exec ultrathink \
  "analyze issue #536 and verify all Python remote behavior is covered" \
  --max-turns 30 \
  --timeout 240
```

`--max-turns` must be between `1` and `50`. `--timeout` must be between `5` and
`480` minutes.

### Step 3: Preserve the VM for debugging

```bash
amplihack remote exec fix \
  "debug the failing remote install smoke test" \
  --keep-vm \
  --vm-name amplihack-azureuser-debug
```

Use `--keep-vm` when you need to inspect logs or tmux state manually after the
run. Remote non-timeout failures also preserve the VM automatically.

## Tutorial 2: Start and monitor a detached session

Use `start` when work should continue after your terminal exits.

### Step 1: Start a detached session

```bash
amplihack remote start "implement remote list JSON output"
```

Expected output:

```text
Starting 1 remote session(s)...
   Command: auto
   VM Size: L (4 concurrent sessions)
   Region: eastus

[1/1] Starting session: implement remote list JSON output...
  -> Packaging context...
  -> Allocating VM...
  -> Transferring context...
  -> Launching tmux session...
  Session started: sess-20260502-203014-4f2a

Successfully started 1 session(s):
  - sess-20260502-203014-4f2a

Use 'amplihack remote output <session-id>' to view progress
```

### Step 2: List sessions

```bash
amplihack remote list
```

Expected output:

```text
SESSION                        VM                               STATUS     AGE      PROMPT
------------------------------------------------------------------------------------------------------------------------
sess-20260502-203014-4f2a      amplihack-azureuser-20260502     running    2m       implement remote list JSON output

Total: 1 session(s)
```

### Step 3: Capture output

```bash
amplihack remote output sess-20260502-203014-4f2a --lines 200
```

Expected output:

```text
=== Session: sess-20260502-203014-4f2a ===
Status: running
VM: amplihack-azureuser-20260502
Prompt: implement remote list JSON output
================================================================================
Step 4: Research and Design
  Inspecting crates/amplihack-remote/src/session.rs
```

### Step 4: Follow output

```bash
amplihack remote output sess-20260502-203014-4f2a --follow
```

The command refreshes every 5 seconds. Press Ctrl+C to stop following; the remote
session keeps running.

## Tutorial 3: Run multiple sessions in parallel

The VM pool reuses VMs while capacity is available.

```bash
amplihack remote start \
  "wire remote exec in amplihack-cli" \
  "write remote parser tests" \
  "document amplihack-remote public API"
```

With the default `--size l`, all three sessions can share one
`Standard_E16s_v5` VM because the L tier has four session slots.

Check utilization:

```bash
amplihack remote status
```

Expected output:

```text
=== Remote Session Pool Status ===

VMs: 1 total
  amplihack-azureuser-20260502 (Standard_E16s_v5, eastus)
    Sessions: 3/4 (75% capacity)
      - sess-20260502-203014-4f2a (running)
      - sess-20260502-203122-1ab9 (running)
      - sess-20260502-203135-d094 (running)

Sessions: 3 total
  Running: 3
  Completed: 0
  Failed: 0
  Killed: 0
  Pending: 0
```

If every VM in the requested region is full, `start` provisions another VM.

## Tutorial 4: Use JSON for automation

Use `--json` with `list` and `status` when scripting.

### Step 1: Find running sessions

```bash
amplihack remote list --json \
  | jq -r '.[] | select(.status == "running") | .session_id'
```

Example output:

```text
sess-20260502-203014-4f2a
sess-20260502-203122-1ab9
```

### Step 2: Watch until all sessions finish

```bash
#!/usr/bin/env bash
set -euo pipefail

while true; do
  running=$(
    amplihack remote list --json \
      | jq '[.[] | select(.status == "running" or .status == "pending")] | length'
  )

  echo "$(date -Is): $running active session(s)"

  if [ "$running" -eq 0 ]; then
    break
  fi

  sleep 60
done
```

### Step 3: Save final output

```bash
for session in $(amplihack remote list --json | jq -r '.[].session_id'); do
  amplihack remote output "$session" --lines 1000 > "$session.log"
done
```

## Tutorial 5: Stop a stuck session

### Step 1: Inspect the session

```bash
amplihack remote output sess-20260502-203014-4f2a --lines 200
```

### Step 2: Try a normal kill

```bash
amplihack remote kill sess-20260502-203014-4f2a
```

Expected output:

```text
Killing session: sess-20260502-203014-4f2a
  Tmux session terminated on amplihack-azureuser-20260502
  Session marked as KILLED
  VM capacity released

Session 'sess-20260502-203014-4f2a' has been terminated.
```

### Step 3: Force local cleanup if the VM is gone

If the VM was deleted or cannot be reached, release local state explicitly:

```bash
amplihack remote kill sess-20260502-203014-4f2a --force
```

`--force` continues after a remote tmux kill failure, marks the session killed,
and releases the VM pool slot.

## Tutorial 6: Choose the right command

| Situation | Command |
| --------- | ------- |
| You want a blocking run that retrieves and integrates results before returning | `amplihack remote exec` |
| You want work to continue after disconnecting | `amplihack remote start` |
| You need a quick status table | `amplihack remote list` |
| You need logs from a specific session | `amplihack remote output` |
| You need to stop a runaway session | `amplihack remote kill` |
| You need pool capacity and counts | `amplihack remote status` |

## Cleanup after long-running work

Review output before killing completed sessions:

```bash
amplihack remote list --status completed
amplihack remote output sess-20260502-203014-4f2a --lines 1000
```

Then release tracked sessions you no longer need:

```bash
amplihack remote kill sess-20260502-203014-4f2a --force
```

If you need to inspect a VM manually:

```bash
azlin connect amplihack-azureuser-20260502 "tmux ls"
azlin connect amplihack-azureuser-20260502 "tmux capture-pane -t sess-20260502-203014-4f2a -p -S -200"
```
