---
title: Coordination Messages
description: The [coord] marker, which stops the workflow classification reminder from routing session-to-session messages into a recipe.
last_updated: 2026-10-29
review_schedule: quarterly
owner: amplihack
doc_type: reference
---

# Coordination Messages

A prompt that starts with `[coord]` is coordination traffic between sessions. The `workflow-classification-reminder` UserPromptSubmit hook gives it a short note instead of the routing prompt, so the receiving agent answers or acts on it without starting dev-orchestrator, smart-orchestrator or a recipe (issue #1742).

The marker is read by code. The hook does not infer coordination traffic from wording, sender or tmux pane.

## Contents

- [Sending a coordination message](#sending-a-coordination-message)
- [Marker rules](#marker-rules)
- [Explicit work requests](#explicit-work-requests)
- [The coordination note](#the-coordination-note)
- [Routing prompt calibration](#routing-prompt-calibration)
- [Gate order](#gate-order)
- [Outcomes and `last_rule`](#outcomes-and-last_rule)
- [State file](#state-file)
- [Security properties](#security-properties)
- [Related](#related)

## Sending a coordination message

Put `[coord]` at the start of the text you type or paste into another session:

```text
[coord] report your lane status
[coord] CI passed on PR 1739, nothing for you to do
[coord] handing PR 1741 over to you
[coord] build #1740
```

The first three receive the coordination note. The last one is an explicit work request and receives the routing prompt (see [Explicit work requests](#explicit-work-requests)).

amplihack has no dedicated send command. Any tool that types into a session, such as `tmux send-keys`, works:

```bash
tmux send-keys -t lane-3 '[coord] report your lane status' Enter
```

## Marker rules

The constant is `COORDINATION_PREFIX` in `crates/amplihack-hooks/src/workflow_classification.rs`, with the value `[coord]`. The only function that reads it is `strip_coordination_prefix`.

1. Leading whitespace is trimmed from the prompt.
2. The first seven bytes are compared with `[coord]`, ignoring ASCII case.
3. If they match, the rest of the prompt, with leading whitespace trimmed, is the body. The body may be empty.

A marker joined to the next word still matches. A marker anywhere other than the start of the prompt does not.

| Prompt | Marked | Body |
| --- | --- | --- |
| `[coord] report your lane status` | Yes | `report your lane status` |
| `  [COORD] status?` | Yes | `status?` |
| `[Coord] status?` | Yes | `status?` |
| `[coord]status` | Yes | `status` |
| `[coord]` | Yes | empty |
| `please [coord] status` | No | |
| `[coordination] status` | No: the seventh byte is `i`, not `]` | |
| `[coord ] status` | No | |
| `(coord) status` | No | |
| `[coo` | No: shorter than the marker | |
| `[coörd] x` | No | |

An unmarked prompt is classified exactly as it was before this feature.

## Explicit work requests

A marked body that asks for specific work is not treated as coordination traffic. It receives the routing prompt, and the agent classifies it as usual. The rule is `is_explicit_work_request`, applied to the body only:

```text
(?i)^(build|fix|implement|develop|work\s+on|start)\s+(#\d+|issue\s+#?\d+|pr\s+#?\d+)\b
```

The verb must come first and the reference must follow it directly. A reference needs `#`, `issue` or `PR`.

| Body | Explicit work request |
| --- | --- |
| `build #1740` | Yes |
| `fix issue 12` | Yes |
| `work on PR #57` | Yes |
| `implement pr 3 now` | Yes |
| `Start #1742` | Yes |
| `fix the parser for #12` | No: words between the verb and the reference |
| `building #1740` | No: only the listed verbs count |
| `fixes #3` | No |
| `build 1740` | No: no `#`, `issue` or `PR` |
| `fix issue#12` | No: `issue` must be followed by a space |
| `report your lane status` | No |

An explicit work request is routed even when the throttle would otherwise suppress the routing prompt. It records `last_classified_turn`, so the throttle then applies to the following unmarked prompts as usual.

A marked body that is an explicit `/dev` command, such as `[coord] /dev fix the parser`, behaves like an unmarked `/dev` command: the hook emits nothing and the command runs.

## The coordination note

The note is the constant `COORDINATION_NOTE`. Its full text is:

```text
[coordination] This message starts with the [coord] marker and is coordination traffic between sessions. Answer it or act on it directly. Do not start dev-orchestrator, smart-orchestrator or any recipe for it.
```

The note is emitted on every marked prompt that is not an explicit work request or `/dev` command. It is not throttled. It never contains `auto-intent-router`, `launching dev-orchestrator`, `Invoke Skill(skill="dev-orchestrator")` or `[auto-routed]`. The body of the message is never copied into the hook output.

The note uses the same output shape as the routing prompt, with the text at the top level for the Copilot CLI and under `hookSpecificOutput` for Claude Code:

```json
{
  "additionalContext": "[coordination] This message starts with the [coord] marker ...",
  "hookSpecificOutput": {
    "hookEventName": "UserPromptSubmit",
    "additionalContext": "[coordination] This message starts with the [coord] marker ..."
  }
}
```

To see it, pipe a prompt into the hook from a project directory, outside any recipe run:

```bash
env -u AMPLIHACK_RECIPE_RUN_ID -u AMPLIHACK_SESSION_DEPTH \
  sh -c 'printf "%s" "{\"hook_event_name\":\"UserPromptSubmit\",\"prompt\":\"[coord] report your lane status\",\"session_id\":\"lane-3\"}" \
  | amplihack-hooks workflow-classification-reminder'
```

Removing `[coord]` from the same prompt at turn 0 returns the routing prompt instead.

## Routing prompt calibration

The routing prompt (`crates/amplihack-hooks/src/routing_prompt.txt`) treats short coordination messages that arrive without the marker as Q&A or OPS, not as ambiguous DEV requests.

Rule 8 reads:

```text
8. Ambiguous request for code changes → **DEV** (safe default). A one-line status question, result report, acknowledgement or PR hand-over is not ambiguous: classify it as **Q&A**, or **OPS** when it asks for one direct action such as retrying a command.
```

These five calibration lines follow the existing ones. The prompt file contains them exactly as shown, including the two-space indent:

```text
  "report your lane status" = Q&A. "what's your status?" = Q&A.
  "CI passed on PR 12" = Q&A.
  "handing PR 57 over to you" = Q&A (acknowledge, no workflow).
  "PR 134 is merged, retry the install" = OPS.
  "build #1740" = DEV. A leading [coord] marker does not change the classification.
```

The closing line reads:

```text
When a request for code changes is ambiguous, choose DEV. False positive costs minutes. False negative costs quality.
```

A local override at `.claude/tools/amplihack/hooks/templates/routing_prompt.txt` takes precedence over the built-in prompt and does not receive these lines automatically. Copy them into the override by hand. The marker branch does not read the routing prompt, so marked messages receive the note whether or not the override is updated.

## Gate order

`process()` applies these checks in order. The first one that decides the outcome wins.

1. Empty prompt: emit nothing.
2. Turn resolution: use the runtime's `turnCount` or `turn_count`, or the session's own `observed_turns` counter when none is reported.
3. Provenance: if `AMPLIHACK_RECIPE_RUN_ID` is set and non-empty, the prompt was written by a recipe step. Emit nothing and record no outcome.
4. Nested session or active workflow: emit nothing and record no outcome.
5. Marker: apply the marked rows of the table below.
6. Topic decision for unmarked prompts: explicit `/dev` command, new topic or throttled.
7. Emit the result.

Provenance takes precedence over the marker. A marked prompt inside a recipe run, including `[coord] build #1740`, produces `{}`. The marker can only replace the routing prompt with the note, or bypass the throttle for an explicit work request; it never starts a recipe itself.

Turn resolution runs before the provenance gate. When the runtime reports no turn number, step 2 increments `observed_turns` and saves the state file, so a prompt stopped at step 3 or 4 can still raise `observed_turns` by one. It never changes `last_rule` or `last_classified_turn`. When the runtime does report a turn number, these gates leave the state file untouched.

An empty `session_id` is treated as `unknown-session`, as a missing one already is.

## Outcomes and `last_rule`

Each outcome is recorded in the state file's `last_rule` field. These values are a stable contract for log readers; renaming one is a breaking change.

| Path | Output | `last_rule` | `last_classified_turn` |
| --- | --- | --- | --- |
| Marked; body is an explicit `/dev` command | `{}` | `explicit_dev_command` | unchanged |
| Marked; body is an explicit work request | routing prompt, throttle ignored | `coordination_explicit_dev` | set to the current turn |
| Marked; anything else | coordination note, every turn | `coordination_marker` | unchanged |
| Unmarked explicit `/dev` command | `{}` | `explicit_dev_command` | unchanged |
| Unmarked new topic | routing prompt | `classification` | set to the current turn |
| Unmarked, not a new topic | `{}` | `throttled` | unchanged |

The note path leaves `last_classified_turn` unchanged because no classification took place, so the next human prompt is judged as it would have been without the coordination message.

The provenance, nested-session and workflow-active gates record no outcome: `last_rule` and `last_classified_turn` keep their previous values. Only `observed_turns` can change on those paths, as described under [Gate order](#gate-order). If a custom routing prompt is blank, the hook emits `{}`, but the outcome has already been recorded.

## State file

The hook keeps one file per session at:

```text
<project>/.claude/runtime/classification_state/<session>.json
```

`<session>` is the session ID with every character outside `[A-Za-z0-9_-]` replaced by `_`. An example after a coordination note at turn 7:

```json
{
  "last_classified_turn": 2,
  "session_id": "lane-3",
  "observed_turns": 0,
  "last_rule": "coordination_marker"
}
```

| Field | Type | Meaning |
| --- | --- | --- |
| `last_classified_turn` | integer | Turn at which the routing prompt was last emitted |
| `session_id` | string | Session ID as received |
| `observed_turns` | integer, default `0` | The hook's own prompt counter, used when the runtime sends no turn number |
| `last_rule` | string or absent, default absent | The `last_rule` value of the most recent recorded outcome |

Files written before this feature have no `last_rule` field and still load. Older binaries ignore the new field.

State writes are best-effort. A write failure is logged at debug level with the error and path, never the prompt, and the hook still returns its output. An unreadable or malformed file is treated as empty, which leans towards emitting the routing prompt, not suppressing it.

## Security properties

- The marker is advisory. No marked prompt leads directly to a recipe; the strongest effect, for an explicit work request, is the same routing prompt that an unmarked new topic receives.
- The provenance, nested-session and workflow-active gates run before the marker is read.
- The prefix comparison cannot panic on short, whitespace-only or multibyte input.
- The work-request pattern is a literal regex compiled once. The `regex` crate matches in linear time.
- Neither the prompt nor the body is written to the state file or the debug log, and neither is copied into the hook output, so a peer session cannot forge hook context through a coordination message.

## Related

- [UserPromptSubmit Hook API](./user-prompt-submit-hook-api.md#workflow-classification-reminder): the other hooks run on each prompt
- [Dev-Orchestrator Routing Contract](./dev-orchestrator-routing.md): what happens once a prompt is routed to dev-orchestrator
- [Workflow Classifier](./workflow-classifier.md): how the orchestrator classifies a routed task
