# Fleet Admiral Reasoning Engine

This document explains how `amplihack fleet` decides what to do with each
session: what the reasoner reads, what it decides, how confidence is derived,
and why it is structured the way it is.

## Contents

- [Overview](#overview)
- [What the reasoner sees](#what-the-reasoner-sees)
- [The five actions](#the-five-actions)
- [Confidence scoring](#confidence-scoring)
- [Reasoner backends](#reasoner-backends)
- [How the TUI dry-run works](#how-the-tui-dry-run-works)
- [Failure modes and visibility](#failure-modes-and-visibility)
- [What was deliberately left out](#what-was-deliberately-left-out)

---

## Overview

The fleet admiral reasoning engine is the part of `amplihack fleet` that looks
at a running Claude session and decides what to do next: send it a nudge, wait,
escalate to a human, restart it, or mark it complete.

The engine is implemented as `FleetSessionReasoner` in `src/commands/fleet.rs`.
It wraps a subprocess call to the configured LLM backend (Claude by default)
and parses the response as a typed `SessionDecision`.

The same engine powers three surfaces:

| Surface | How to trigger | Output |
|---------|----------------|--------|
| `fleet dry-run` | CLI subcommand | Printed dry-run report |
| `fleet scout` | CLI subcommand | Scout report + `last_scout.json` |
| TUI `d` key | Keypress in Fleet tab | Proposal notice in Detail tab |

---

## What the reasoner sees

For each session the reasoner receives a **`SessionContext`** assembled from
four sources:

| Source | How it is collected |
|--------|---------------------|
| tmux terminal output | `azlin tmux capture-pane -p` (capped at `--capture-lines`, default 50) |
| Transcript | Fetched via `azlin` SSH from the session's Claude transcript file |
| Git context | `git branch --show-current` and `gh pr list` on the remote VM |
| Agent status | Derived from terminal output patterns (see [confidence scoring](#confidence-scoring)) |

The reasoner formats all four inputs into a single prompt and sends them to the
backend under the Fleet Admiral system prompt:

```
You are a Fleet Admiral managing coding agent sessions across multiple VMs.

For each session, analyze the terminal output and transcript to decide what
to do.

Valid actions:
- send_input
- wait
- escalate
- mark_complete
- restart

Respond with JSON only:
{
  "action": "send_input|wait|escalate|mark_complete|restart",
  "input_text": "text to type (only for send_input)",
  "reasoning": "why you chose this action",
  "confidence": 0.0
}
```

The response is parsed into a `SessionDecision` struct.  Malformed JSON causes
a reasoner error (see [failure modes](#failure-modes-and-visibility)).

---

## The five actions

| Action         | Meaning                                                                 | Executed by |
| -------------- | ----------------------------------------------------------------------- | ----------- |
| `send_input`   | Type `input_text` into the session's tmux pane                         | `azlin tmux send-keys` |
| `wait`         | Do nothing; session is making progress on its own                       | No-op |
| `escalate`     | Flag the session for human review; do nothing automatically             | No-op |
| `mark_complete`| Record the session as finished; remove from active queue                | No-op |
| `restart`      | Kill the current agent process and relaunch it in the same tmux pane    | `azlin restart-session` |

`wait`, `escalate`, and `mark_complete` never produce side effects in the
current orchestration cycle.  They are included so the reasoner can express
intent that the admiral records for reporting.

`send_input` requires `confidence >= 0.6` to execute.  `restart` requires
`confidence >= 0.8`.  Actions below these thresholds are silently downgraded
to `wait`.

---

## Confidence scoring

The backend reports a confidence value in `[0.0, 1.0]`.  The admiral also
derives a heuristic confidence from terminal output patterns independent of the
LLM response, used as a sanity check:

| Pattern                       | Heuristic confidence |
| ----------------------------- | -------------------- |
| Session thinking (LLM busy)   | 1.0                  |
| Session running (tool use)    | 0.8                  |
| Session completed             | 0.9                  |
| Session stuck (> 300 s idle)  | 0.85                 |
| Session idle                  | 0.7                  |
| Session with active output    | 0.5                  |
| Session status unknown        | 0.3                  |

The heuristic confidence is logged but does not override the LLM-reported
confidence.  If the LLM reports `confidence: 0.9` for a session the heuristic
says is `Unknown`, the LLM value is used for the minimum-threshold checks.

---

## Reasoner backends

The reasoner backend is selected by `--backend` (CLI) or `NativeReasonerBackend::detect("auto")`.

| Backend | How it is invoked | When it is selected by `auto` |
|---------|-------------------|-------------------------------|
| `claude` | `claude --dangerously-skip-permissions --print` with the prompt on stdin | `AMPLIHACK_FLEET_REASONER_BINARY_PATH` is set, or `claude` is on PATH |
| `none`   | Returns a synthetic `wait` decision for every session | No `claude` binary is found |

The `none` backend is not a fallback in the degraded-service sense — it is an
explicit no-op mode used for testing and for fleet scans where you only want the
discovery and adoption phases.

**The backend subprocess is always called with `--dangerously-skip-permissions`**
because it runs inside an already-trusted environment (the user's own machine,
interacting with their own Azure VMs).  The flag is required for non-interactive
invocations of Claude.

---

## How the TUI dry-run works

Pressing `d` in the Fleet tab triggers `run_tui_dry_run()`:

1. The currently selected session is identified from `FleetTuiUiState`.
2. A `FleetSessionReasoner` is constructed with the same backend auto-detection
   as `fleet dry-run`.
3. `reasoner.reason_about_session()` is called.  The function blocks the UI
   event loop for up to the 180 s reasoner timeout.
4. On success, the resulting `SessionDecision` is stored in
   `ui_state.proposal_notice` and `ui_state.editor_decision`.
5. The cockpit switches to the Detail tab.  The proposal notice is rendered
   above the tmux capture preview:

   ```
   Proposed action: send_input (87%)
   Input: "Open a pull request for your current branch."
   Reason: Tests passing; PR is ready.
   ```

The proposal notice is **session-scoped**: it is tied to a specific
`(vm_name, session_name)` pair.  Navigating to a different session shows that
session's proposal (or nothing, if no dry-run has been run for it yet).
Returning to the original session shows the cached proposal from the last run.

---

## Failure modes and visibility

### Dry-run failure notice

When `reason_about_session()` returns `Err`, the cockpit stores the error as a
persistent **dry-run failure notice** in `proposal_notice`:

```
Reasoner error: backend subprocess exited with code 1
Press 'd' to retry.
```

The notice is:
- **Persistent across refresh cycles** — it does not disappear on the next 5 s
  tmux refresh.  It stays until the user retries (`d`) or navigates to a
  different session.
- **Category-only in the TUI** — the `Display` form of `FleetError` shows only
  the error category.  Full detail (exit code, stderr, absolute paths) goes to
  `~/.claude/runtime/logs/`.

### Apply failure notice

When the user presses `a` or `A` to apply a proposal and `execute_decision()`
returns `Err`, the cockpit stores a **persistent apply failure notice**:

```
Apply failed: tmux send-keys returned exit code 1
Last action: send_input -> amplihack-vm-01/work-session-3
```

The apply failure notice is cleared on the next successful apply to the same
session.  A second failed apply replaces the notice with the newer error.

### Why both notices are persistent

Both the dry-run failure and the apply failure notices persist deliberately:

1. The user might not be watching the screen at the moment the error occurs.
   A transient flash would be missed.
2. Persistent notices let the user understand the current state of each session
   without needing to rerun anything — they can see at a glance which sessions
   have outstanding errors.
3. Notices are session-scoped, so navigating to other sessions and back
   preserves the error context.

---

## What was deliberately left out

The following capabilities were explicitly considered and excluded from the
current implementation.  Each entry explains what was omitted, why it was
omitted, and what the implication is for users and contributors.  Reviewers
can use this list to understand scope boundaries without re-litigating past
decisions.

### No automatic retry

**What:** Failed dry-runs and failed applies do not retry automatically.
The user must press `d` or `a` again.

**Why:** Automatic retry risks generating runaway LLM calls (and cost) if the
backend is intermittently slow, or repeatedly applying a broken action to a
session before the user notices the failure.  Persistent failure notices (see
[failure modes](#failure-modes-and-visibility)) ensure the user always sees the
error and chooses when to retry.

**Implication:** If a reasoner call times out or the apply fails, the user must
take explicit action to recover.  This is intentional — the fleet admiral
should not act on sessions without human awareness.

### No confidence override

**What:** The TUI does not let the user manually set a confidence value.
Confidence is always derived from the LLM response.

**Why:** Confidence drives the minimum-threshold checks for `send_input`
(`>= 0.6`) and `restart` (`>= 0.8`).  Letting the user override confidence
would allow bypassing these guards silently.  Instead, the action type can be
changed with `t` (see
[cycle editor action choices](../howto/use-fleet-dashboard.md#cycle-editor-action-choices)),
which is the correct mechanism for overriding the reasoner's recommendation.

**Implication:** The confidence value displayed in the proposal notice always
reflects the reasoner's assessment.  A user who disagrees with the action
should use `t` to change the action, not try to manipulate confidence.

### No streaming output during reasoning

**What:** The reasoner runs to completion before the TUI updates.  The cockpit
does not show a spinner or partial output during the 180 s timeout window.

**Why:** The current architecture calls `reason_about_session()` synchronously
in the event-loop thread.  Adding streaming would require moving the reasoner
call to a background thread and plumbing progress events through the `mpsc`
channel — non-trivial work with risk of introducing race conditions in the
shared `FleetTuiUiState`.

**Implication:** The TUI appears unresponsive for up to 180 s after pressing
`d`.  This is a known limitation.  A future version may spawn the reasoner in
a background thread and render progress events.  Do not add a fake spinner that
simply polls on a timer — wait for the proper async architecture.

### No per-session backend configuration

**What:** Every session is reasoned about by the same backend binary.  There
is no per-session or per-VM override of `--backend` at the TUI level.

**Why:** Supporting per-session backend configuration in the TUI state would
significantly increase the complexity of `FleetTuiUiState` and the proposal
serialisation path.  The use case (different LLM backends for different
sessions) was not a stated requirement for this release.

**Implication:** All sessions in a single dashboard run use the same backend.
Users who need per-session backends can run separate `fleet dry-run --backend`
invocations from the CLI.

---

**See also**

- [Run Fleet Scout and Advance on Azure VMs](../howto/run-fleet-scout-and-advance.md)
- [How to Use the Fleet Dashboard](../howto/use-fleet-dashboard.md)
- [Fleet Dashboard Architecture](../concepts/fleet-dashboard-architecture.md)
- [amplihack fleet — CLI reference](../reference/fleet-command.md)
