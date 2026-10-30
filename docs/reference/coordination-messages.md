---
title: Coordination Messages
description: The [coord] marker, which stops the workflow classification reminder from routing session-to-session messages into a recipe.
last_updated: 2026-10-29
review_schedule: quarterly
owner: amplihack
doc_type: reference
---

# Coordination Messages

A prompt that starts with `[coord]` is coordination traffic between sessions. The `workflow-classification-reminder` UserPromptSubmit hook gives it a short note instead of the routing prompt, so the receiving agent answers or acts on it without starting dev-orchestrator, smart-orchestrator or a recipe (issue #1742). A marked message that asks for a code change still goes through dev-orchestrator and `default-workflow`.

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
[coord] fix the parser for #12
```

The first three receive the coordination note. The last two start with a change verb, so they are work requests and receive the routing prompt (see [Explicit work requests](#explicit-work-requests)).

amplihack has no dedicated send command. Any tool that types into a session, such as `tmux send-keys`, works:

```bash
tmux send-keys -t lane-3 '[coord] report your lane status' Enter
```

## Marker rules

The constant is `COORDINATION_PREFIX` in `crates/amplihack-hooks/src/workflow_classification/coordination.rs`, with the value `[coord]`. The only function that reads it is `strip_coordination_prefix`.

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

An unmarked prompt takes the same path through the hook as before this feature: the same gates, the same throttle and the same routing prompt file. That file has changed, however. The status and hand-over exception added to rule 8, and five new calibration lines, apply to every unmarked new-topic prompt in every session, under Claude Code and the Copilot CLI; see [Routing prompt calibration](#routing-prompt-calibration).

## Explicit work requests

A marked body that asks for work is not treated as coordination traffic. It receives the routing prompt, and the agent classifies it as usual. The rule is `is_explicit_work_request`, applied to the body only. A body is a work request when it starts with a change verb, then whitespace, then any text:

```text
(?i)^(build|fix|implement|develop|add|create|refactor|update|write|work\s+on|start)\s+\S
```

The verbs are the routing prompt's IMPLEMENT signal (build, fix, add, create, refactor, update, write) plus implement, develop, work on and start. A unit test reads the IMPLEMENT signal from `routing_prompt.txt` and fails if a verb there is missing from the rule.

| Body | Work request |
| --- | --- |
| `build #1740` | Yes |
| `fix issue 12` | Yes |
| `work on PR #57` | Yes |
| `fix the parser for #12` | Yes |
| `refactor #57` | Yes |
| `update PR 57` | Yes |
| `implement retry in installer.rs` | Yes |
| `fix landed on PR 12` | Yes, although it is a report; see below |
| `update: CI passed on PR 12` | No: the verb is not followed by whitespace |
| `building #1740` | No: only the listed verbs count |
| `fixes #3` | No |
| `please build #1740` | No: the verb is not first; see below |
| `report your lane status` | No |
| `handing PR 57 over to you` | No |

The rule errs towards routing. A report that starts with a verb, such as `fix landed on PR 12`, receives the routing prompt, whose rule 8 then classifies a result report as Q&A; the cost is one classification. The opposite error would let a code change bypass `default-workflow`.

A code request that the rule misses, such as `please build #1740` or `can you fix the parser`, receives the note. The note does not forbid the workflow for code changes: it tells the agent to use dev-orchestrator when the message asks for one. See [The coordination note](#the-coordination-note).

A work request is routed even when the throttle would otherwise suppress the routing prompt. It records `last_classified_turn`, so the throttle then applies to the following unmarked prompts as usual.

A marked body that is an explicit `/dev` command, such as `[coord] /dev fix the parser`, behaves like an unmarked `/dev` command: the hook emits nothing and the command runs.

## The coordination note

The note is the constant `COORDINATION_NOTE`. Its full text is:

```text
[coordination] This message starts with the [coord] marker and is coordination traffic between sessions. Answer it or act on it directly, without dev-orchestrator, smart-orchestrator or a recipe. If it asks for a code change, use dev-orchestrator as usual.
```

The note keeps recipes away from replies, reports, acknowledgements and single commands such as retrying an install. It does not keep them away from code changes. The routing prompt's downstream invariant is that development work always runs through `default-workflow`, and a marked request for a code change that the work-request rule does not recognise must still meet it.

The note is emitted on every marked prompt that is not a work request or `/dev` command. It is not throttled. It never contains `auto-intent-router`, `launching dev-orchestrator`, `Invoke Skill(skill="dev-orchestrator")` or `[auto-routed]`. The body of the message is never copied into the hook output.

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

This is a change to the routing prompt itself, so it applies to every unmarked new-topic prompt in every amplihack session, under Claude Code and the Copilot CLI, and not only to coordination traffic. It is kept to the one exception that issue #1742 asked for. Any other ambiguous prompt, such as "the login page looks wrong", still falls to DEV.

Rule 8 keeps its unqualified DEV default and gains one sentence, the exception for status, result and hand-over messages. It reads:

```text
8. Ambiguous → **DEV** (safe default). A one-line status question, result report, acknowledgement or PR hand-over is not ambiguous: classify it as **Q&A**, or **OPS** when it asks for one direct action such as retrying a command.
```

These five calibration lines follow the existing ones. The prompt file contains them exactly as shown, including the two-space indent:

```text
  "report your lane status" = Q&A. "what's your status?" = Q&A.
  "CI passed on PR 12" = Q&A.
  "handing PR 57 over to you" = Q&A (acknowledge, no workflow).
  "PR 134 is merged, retry the install" = OPS.
  "build #1740" = DEV. A leading [coord] marker does not change the classification.
```

The closing line is unchanged:

```text
When in doubt, choose DEV. False positive costs minutes. False negative costs quality.
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

Provenance takes precedence over the marker. A marked prompt inside a recipe run, including `[coord] build #1740`, produces `{}`. The marker can only replace the routing prompt with the note, or bypass the throttle for a work request; it never starts a recipe itself.

Turn resolution runs before the provenance gate. When the runtime reports no turn number, step 2 increments `observed_turns` and saves the state file, so a prompt stopped at step 3 or 4 can still raise `observed_turns` by one. It never changes `last_rule` or `last_classified_turn`. When the runtime does report a turn number, these gates leave the state file untouched.

An empty `session_id` is treated as `unknown-session`, as a missing one already is.

## Outcomes and `last_rule`

Each outcome is recorded in the state file's `last_rule` field. These values are a stable contract for log readers; renaming one is a breaking change.

| Path | Output | `last_rule` | `last_classified_turn` |
| --- | --- | --- | --- |
| Marked; body is an explicit `/dev` command | `{}` | `explicit_dev_command` | unchanged |
| Marked; body is a work request (starts with a change verb) | routing prompt, throttle ignored | `coordination_explicit_dev` | set to the current turn |
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

- The marker is advisory. No marked prompt leads directly to a recipe; the strongest effect, for a work request, is the same routing prompt that an unmarked new topic receives.
- The provenance, nested-session and workflow-active gates run before the marker is read.
- The prefix comparison cannot panic on short, whitespace-only or multibyte input.
- The work-request pattern is a literal regex compiled once. The `regex` crate matches in linear time.
- Neither the prompt nor the body is written to the state file or the debug log, and neither is copied into the hook output, so a peer session cannot forge hook context through a coordination message.

## Related

- [UserPromptSubmit Hook API](./user-prompt-submit-hook-api.md#workflow-classification-reminder): the other hooks run on each prompt
- [Dev-Orchestrator Routing Contract](./dev-orchestrator-routing.md): what happens once a prompt is routed to dev-orchestrator
- [Workflow Classifier](./workflow-classifier.md): how the orchestrator classifies a routed task
