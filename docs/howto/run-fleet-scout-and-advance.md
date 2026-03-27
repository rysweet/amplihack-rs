# How to Run Fleet Scout and Advance on Azure VMs

`amplihack fleet scout` and `amplihack fleet advance` let you inspect and act
on Claude sessions running across multiple Azure VMs in one command.  Use them
when you need to check what your agents are doing and push them forward without
opening a separate terminal for each VM.

## Contents

- [Prerequisites](#prerequisites)
- [Run a scout report](#run-a-scout-report)
- [Advance sessions interactively](#advance-sessions-interactively)
- [Automate advance with --force](#automate-advance-with---force)
- [Save reports to disk](#save-reports-to-disk)
- [Use incremental scouting](#use-incremental-scouting)
- [Target a single VM or session](#target-a-single-vm-or-session)
- [Run the continuous orchestration loop](#run-the-continuous-orchestration-loop)
- [Output format reference](#output-format-reference)
- [Troubleshoot common problems](#troubleshoot-common-problems)

---

## Prerequisites

| Requirement | Check |
|-------------|-------|
| `azlin` installed and on `PATH` | `azlin --version` |
| `claude` installed and on `PATH` | `claude --version` (or set `AMPLIHACK_FLEET_REASONER_BINARY_PATH`) |
| At least one Azure VM registered in `azlin` | `azlin list` |
| Active tmux sessions on the target VMs | `azlin list` shows sessions |

If `azlin` is not on `PATH`, set `AZLIN_PATH`:

```sh
export AZLIN_PATH=/usr/local/bin/azlin
```

---

## Run a scout report

```sh
amplihack fleet scout
```

Scout runs three phases:

1. **Discover** — calls `azlin list` to find all running VMs and their tmux
   sessions.
2. **Adopt** — registers discovered sessions in the local task queue
   (`~/.claude/runtime/fleet/queue.json`).
3. **Reason** — calls the LLM backend for each session and prints a scout
   report.

**Sample output**

```
Phase 1: Discovering fleet sessions...
Phase 2: Adopting sessions...
  amplihack-vm-01: adopted 2 sessions
Total adopted: 2
Phase 3: Reasoning about sessions...
  Reasoning: amplihack-vm-01/work-session-1...
  Reasoning: amplihack-vm-01/work-session-2...

============================================================
FLEET SCOUT REPORT
============================================================
VMs discovered: 2
Running VMs: 1
Sessions analyzed: 2
Adopted sessions: 2
Actions:
  send_input: 1
  wait: 1

  amplihack-vm-01/work-session-1 [running] -> send_input (87%)
    Branch: feature/auth-refactor
    PR: https://github.com/org/amplihack-rs/pull/42
    Project: amplihack-rs
    Reason: Tests passing; agent should open the PR now.
    Input: "Open a pull request for your current branch."

  amplihack-vm-01/work-session-2 [idle] -> wait (70%)
    Branch: main
    Reason: No active task; waiting for a new assignment.
```

The report is also cached at `~/.claude/runtime/fleet/last_scout.json` for
use by `--incremental` runs.

---

## Advance sessions interactively

After reviewing the scout report, run `advance` to act on the recommendations:

```sh
amplihack fleet advance
```

`advance` reasons about each session (Phase 1 again, freshly) and then asks
you to confirm before executing any `send_input` or `restart` action:

```
Phase 1: Discovering fleet sessions...

Phase 2: Reasoning and executing actions...

  [amplihack-vm-01/work-session-1] reasoning...
    -> send_input: "Open a pull request for your current branch." (conf=87%) Execute? [Y/n]
```

Press `Enter` or `y` to execute.  Press `n` to skip that session.

`wait`, `escalate`, and `mark_complete` actions are no-ops — they are never
executed and never prompt.

**Advance report**

After all sessions are processed, `advance` prints a summary:

```
============================================================
FLEET ADVANCE REPORT
============================================================
Sessions analyzed: 2
  send_input: 1
  wait: 1

  [OK] amplihack-vm-01/work-session-1 -> send_input
  [SKIPPED] amplihack-vm-01/work-session-2 -> wait
```

`[OK]` — action executed successfully.
`[SKIPPED]` — action was a no-op, or you answered `n` at the prompt.
`[ERROR]` — action was attempted but failed (see troubleshooting below).

---

## Automate advance with --force

In CI or scheduled jobs, skip the interactive prompts entirely:

```sh
amplihack fleet advance --force
```

With `--force`, every recommended `send_input` and `restart` is executed
immediately without asking.  Use this only in automation — it does not
distinguish between high-confidence and low-confidence recommendations.

**Example: nightly sweep in a cron job**

```sh
# Run a force advance every night at 02:00
0 2 * * * amplihack fleet advance --force --save ~/logs/advance-$(date +%Y%m%d).json
```

---

## Save reports to disk

Both `scout` and `advance` accept `--save <PATH>` to write the full JSON
report to a file in addition to printing the human-readable output:

```sh
amplihack fleet scout --save ~/fleet-scout.json
amplihack fleet advance --save ~/fleet-advance.json
```

**Scout JSON schema (excerpt)**

```json
{
  "timestamp": "2026-03-16T02:00:00Z",
  "running_vm_count": 1,
  "session_count": 2,
  "adopted_count": 2,
  "skip_adopt": false,
  "decisions": [
    {
      "vm": "amplihack-vm-01",
      "session": "work-session-1",
      "status": "running",
      "branch": "feature/auth-refactor",
      "pr": "https://github.com/org/amplihack-rs/pull/42",
      "action": "send_input",
      "confidence": 0.87,
      "reasoning": "Tests passing; agent should open the PR now.",
      "input_text": "Open a pull request for your current branch.",
      "error": null,
      "project": "amplihack-rs",
      "objectives": []
    }
  ]
}
```

---

## Use incremental scouting

On busy fleets with many sessions, pass `--incremental` to skip sessions whose
status has not changed since the last scout run:

```sh
amplihack fleet scout --incremental
```

`--incremental` reads `~/.claude/runtime/fleet/last_scout.json` and compares
each session's current status against the cached value.  Sessions whose status
matches the cache are skipped (using the previous decision), saving LLM calls.

```
Incremental mode: loaded 3 previous statuses
  Skipping (unchanged): amplihack-vm-01/work-session-2 [idle]
  Reasoning: amplihack-vm-01/work-session-1...
```

Start fresh (ignore the cache) by omitting `--incremental` or deleting
`~/.claude/runtime/fleet/last_scout.json`.

---

## Target a single VM or session

Restrict any operation to a specific VM or session to limit scope:

```sh
# Scope to one VM
amplihack fleet scout --vm amplihack-vm-01

# Scope to one session on one VM
amplihack fleet advance --vm amplihack-vm-01 --session work-session-1

# Skip adoption and report on already-adopted sessions only
amplihack fleet scout --skip-adopt
```

---

## Run the continuous orchestration loop

For hands-off operation, use `fleet start` to run the admiral in a loop:

```sh
# Adopt existing sessions, then loop every 60 s indefinitely:
amplihack fleet start --adopt

# Run 10 cycles and stop:
amplihack fleet start --max-cycles 10

# Poll every 2 minutes, cap sessions at 2 per VM:
amplihack fleet start --interval 120 --max-agents-per-vm 2
```

Press `Ctrl-C` to stop the loop cleanly.

To verify one cycle without a loop:

```sh
amplihack fleet run-once
```

---

## Output format reference

The rendered output from `fleet scout` and `fleet advance` follows a fixed
structure.  This section annotates each field so you can parse the output in
scripts or understand what each line means.

### Scout report structure

```
============================================================   <- section separator (60 = signs)
FLEET SCOUT REPORT                                             <- report title
============================================================
VMs discovered: 2          <- total VMs returned by azlin list
Running VMs: 1             <- VMs with at least one active tmux session
Sessions analyzed: 2       <- sessions the LLM reasoner was called for
Adopted sessions: 2        <- sessions registered in the local task queue
Actions:                   <- summary of recommended action types
  send_input: 1            <- number of sessions where action = send_input
  wait: 1                  <- number of sessions where action = wait
                           <- (other action types appear here when present)

  amplihack-vm-01/work-session-1 [running] -> send_input (87%)
  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^           ^^^^^^^^^^  ^^^
  |                                           |            confidence (LLM-reported, 0–100%)
  |                                           recommended action
  <VM_NAME>/<SESSION_NAME> [<STATUS>]

    Branch: feature/auth-refactor    <- git branch on the remote VM (absent if unknown)
    PR: https://github.com/…/pull/42 <- open PR URL from `gh pr list` (absent if none)
    Project: amplihack-rs            <- repository name (absent if unknown)
    Reason: Tests passing; …         <- LLM one-line explanation
    Input: "Open a pull request…"    <- text that would be typed (send_input only)
```

**Session status labels**

| Label       | Meaning                                                                 |
| ----------- | ----------------------------------------------------------------------- |
| `running`   | Terminal output shows recent LLM activity or tool-use output            |
| `idle`      | Terminal output is static; no recent activity                           |
| `stuck`     | No terminal output for more than `--stuck-threshold` seconds (default 300 s) |
| `completed` | Terminal output contains a completion pattern                           |
| `unknown`   | Terminal output could not be classified                                 |

### Advance report structure

```
============================================================
FLEET ADVANCE REPORT
============================================================
Sessions analyzed: 3          <- sessions reasoner was called for
  send_input: 2               <- actions of each type
  wait: 1

  [OK] amplihack-vm-01/work-session-1 -> send_input
  ^^^^^                                  ^^^^^^^^^^
  |                                      action that was executed
  outcome tag (see table below)

  [SKIPPED] amplihack-vm-01/work-session-2 -> wait
  [ERROR] amplihack-vm-02/work-session-3 -> send_input: tmux send-keys failed
                                                         ^^^^^^^^^^^^^^^^^^^^^
                                                         error description (category only)
```

**Outcome tags**

| Tag         | Meaning                                                                      |
| ----------- | ---------------------------------------------------------------------------- |
| `[OK]`      | Action was executed successfully                                              |
| `[SKIPPED]` | Action was a no-op (`wait`, `escalate`, `mark_complete`), or you answered `n` at the confirmation prompt |
| `[ERROR]`   | Action was attempted but failed; see error description and check `fleet observe` or log files |

**Exit code note:** `advance` exits `0` even when individual sessions show
`[ERROR]`.  Check the report text or the `--save` JSON output to identify
failures programmatically.

---

## Troubleshoot common problems

### `azlin not found`

```
Error: azlin not found. Install azlin or set AZLIN_PATH.
```

Install `azlin` or set `AZLIN_PATH`:

```sh
export AZLIN_PATH=/path/to/azlin
```

### No sessions discovered

`azlin list` found no running VMs or no tmux sessions.  Check:

```sh
azlin list
```

If VMs are running but sessions are missing, ensure the sessions are inside
a tmux pane named consistently with the `azlin` session registry.

### Reasoner produces `[ERROR]` for every session

The LLM backend cannot be reached.  Check:

```sh
# Verify the binary is reachable:
claude --version

# Or point to a specific binary:
export AMPLIHACK_FLEET_REASONER_BINARY_PATH=/usr/local/bin/claude
```

If running in a non-interactive environment, ensure `claude` does not require
a TTY for the inference path used by the fleet reasoner.

### Advance shows `[ERROR]` for a session

`send_input` or `restart` failed.  Common causes:

| Error message                      | Cause                                                  |
| ---------------------------------- | ------------------------------------------------------ |
| `tmux send-keys returned exit code 1` | The tmux session exited between discovery and advance |
| `azlin ssh failed`                 | The VM is unreachable over SSH                        |
| `restart failed: ...`              | The agent binary could not be relaunched on the VM     |

Run `amplihack fleet observe <VM_NAME>` to tail current session output and
diagnose the root cause.

---

**See also**

- [amplihack fleet — CLI reference](../reference/fleet-command.md)
- [Fleet Admiral Reasoning Engine](../concepts/fleet-admiral-reasoning.md)
- [Fleet Dashboard Architecture](../concepts/fleet-dashboard-architecture.md)
