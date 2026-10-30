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
  - [Work requests and the throttle](#work-requests-and-the-throttle)
- [Reports that start with a verb](#reports-that-start-with-a-verb)
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
[coord] fix landed on PR 1739
[coord] build #1740 failed on main
[coord] build #1740
[coord] fix the parser for #12
```

The first five receive the coordination note. The fourth and fifth start with a change verb, but use it as a noun in a report (see [Reports that start with a verb](#reports-that-start-with-a-verb)). The last two start with a change verb used as a request, so they are work requests. Each receives the routing prompt when the same text without the marker would, and the note while the throttle holds (see [Explicit work requests](#explicit-work-requests)).

Start a report with its subject, not with a change verb: `PR 12 merged`, not `fix #12 merged`. A report that does not start with a change verb always receives the note. A report that does start with one receives the note only when the hook can tell it is a report: it must state its outcome with `is`, `was`, `are`, `were`, `has` or `had`, or with one of the [outcome words](#reports-that-start-with-a-verb). `fix #12 rolled back` uses neither, so it is taken for a work request and receives what the same text without the marker would: the routing prompt on a new topic, and the note while the throttle holds. `PR 12 rolled back` receives the note at any turn.

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

An unmarked prompt passes the same gates as before this feature and receives the same routing prompt file. Two things differ. The routing prompt itself has changed: the status and hand-over exception added to rule 8, and five new calibration lines, apply to every unmarked new-topic prompt in every session, under Claude Code and the Copilot CLI; see [Routing prompt calibration](#routing-prompt-calibration). The throttle no longer counts a classification that never took place; see [State file](#state-file).

## Explicit work requests

A marked body that asks for work is not treated as coordination traffic. It is decided as the same text without the marker would be (see [Work requests and the throttle](#work-requests-and-the-throttle)), and on a new topic it receives the routing prompt and the agent classifies it as usual. The rule is `is_explicit_work_request`, applied to the body only. A body is a work request when it starts with a change verb, then whitespace, then any text:

```text
(?i)^(build|fix|implement|develop|add|create|refactor|update|write|work\s+on|start)\s+\S
```

and is not a report that starts with a change verb used as a noun (see [Reports that start with a verb](#reports-that-start-with-a-verb)).

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
| `fix failed tests on PR 12` | Yes: `failed` follows the verb and is followed by a noun |
| `fix landed on PR 12` | No: a report |
| `build failed on PR 12` | No: a report |
| `fix #12 merged` | No: a report that names its object before its outcome |
| `fix for #12 landed` | No: a report that names its object before its outcome |
| `build #1740 is green` | No: a report |
| `fix the tests that failed` | No: taken for a report; see [Errors fall towards the note](#errors-fall-towards-the-note) |
| `fix #12 rolled back` | Yes: a report, but `rolled back` is not an outcome word; see [Sending a coordination message](#sending-a-coordination-message) |
| `update: CI passed on PR 12` | No: the verb is not followed by whitespace |
| `building #1740` | No: only the listed verbs count |
| `fixes #3` | No |
| `please build #1740` | No: the verb is not first; see below |
| `report your lane status` | No |
| `handing PR 57 over to you` | No |

A code request that the rule misses, such as `please build #1740`, `can you fix the parser` or `fix the tests that failed`, receives the note. The note does not forbid the workflow for code changes: it tells the agent to use dev-orchestrator when the message asks for one. See [The coordination note](#the-coordination-note).

A marked body that is an explicit `/dev` command, such as `[coord] /dev fix the parser`, behaves like an unmarked `/dev` command: the hook emits nothing and the command runs.

### Work requests and the throttle

A work request passes through the same topic decision as an unmarked prompt, applied to the body: the explicit `/dev` check, the rule that routes turns 0 and 1, the new-topic and follow-up wording checks, and the three-turn window after the last classification.

- On a new topic it receives the routing prompt, records `coordination_explicit_dev` and sets `last_classified_turn`, so the window then applies to the following prompts as usual.
- When the throttle holds it back, it receives the coordination note and records `coordination_throttled`. The same text without the marker would receive nothing. The note's last sentence tells the agent to use dev-orchestrator for a code change, so a throttled request still reaches the workflow.

A marked message therefore never receives the routing prompt when the same text without the marker would not. This holds whatever the wording rules get wrong: a report that they take for a request, such as `fix #12 rolled back`, is throttled like the unmarked report. A unit test sends each of a set of bodies with and without the marker to two sessions in the same state and checks this.

The follow-up wording check looks for words such as `and` and `also` in the first three words of the body, so `[coord] fix and test the parser` receives the note from turn 2 on, where `fix and test the parser` would receive nothing.

## Reports that start with a verb

A coordinator's report often opens with a change verb used as a noun: `build failed on PR 12`, `fix #12 merged`, `build #1740 is green`. These bodies match the work-request pattern, but they are reports and receive the note. The rule is `VERB_LED_REPORT_PATTERNS` in `coordination.rs`: four patterns, applied only to bodies that match the work-request pattern. A body is a report when any of them matches:

```text
(?i)^update\s+on\b
(?is)^(?:build|fix|implement|develop|add|create|refactor|update|write|work\s+on|start)\s.*\b(?:is|was|are|were|has|had)(?:[\s.,;:!?]|$)
(?i)^(?:build|fix|implement|develop|add|create|refactor|update|write|work\s+on|start)\s+(?:passed|failed|landed|merged|succeeded|shipped|finished|completed|started|worked|done|reverted|deployed|released|pushed|broke|broken|green|red)(?:\s*$|\s*[.,;:!?]|\s+(?:on|in|for|at|to|into|with|after|again|and|but)\b)
(?is)^(?:build|fix|implement|develop|add|create|refactor|update|write|work\s+on|start)\s+\S+\s.*\b(?:passed|failed|landed|merged|succeeded|shipped|finished|completed|started|worked|done|reverted|deployed|released|pushed|broke|broken|green|red)(?:[\s.,;:!?]|$)
```

The patterns, in the order shown, match a body when:

1. It opens with `update on`, as in `update on PR 57: merged`.
2. A word after the change verb is `is`, `was`, `are`, `were`, `has` or `had`, followed by whitespace, punctuation or the end of the body: `build is green`, `build #1740 is green`, `fix for #12 has landed`.
3. The word straight after the change verb is an outcome word that ends the body or is followed by punctuation (`.,;:!?`) or by `on`, `in`, `for`, `at`, `to`, `into`, `with`, `after`, `again`, `and` or `but`: `build failed on PR 12`, `fix landed.`
4. An outcome word comes later than the word after the change verb, and is followed by whitespace, punctuation or the end of the body. Such a report names its object before its outcome: `fix #12 merged`, `build #1740 failed on main`, `fix for #12 landed`, `build #1740 passed CI`.

The outcome words are `passed`, `failed`, `landed`, `merged`, `succeeded`, `shipped`, `finished`, `completed`, `started`, `worked`, `done`, `reverted`, `deployed`, `released`, `pushed`, `broke`, `broken`, `green` and `red`. The list is closed: a report that starts with a change verb and states its outcome in other words, such as `fix #12 rolled back`, is taken for a work request. The change verbs in patterns 2 to 4 are the same list as in the work-request pattern, so `work on` counts as one verb, and in `work on failed tests` the word after the verb is `failed`. Patterns 2 and 4 read across line breaks.

An outcome word straight after the change verb and followed by a noun is an adjective in a request, so `fix failed tests` and `update merged branches` remain work requests. A copula or outcome word followed directly by another character, such as the `_` in `fix is_valid` or the `-` in `fix passed-in args`, is part of an identifier and does not count.

The hook makes this decision because the routing prompt would decide these reports the other way. The routing prompt applies the first rule that matches. Rule 4, `FILE_EDIT or IMPLEMENT present (no UNDERSTAND) → **DEV**`, comes before rule 8 and its exception for result reports, and the IMPLEMENT signal lists the verbs these reports start with. A verb-led report that reached the routing prompt would usually be classified as DEV and start dev-orchestrator. A unit test checks that rule 4 still comes before rule 8.

### Errors fall towards the note

The patterns cannot separate every report from every request, so they are written to err in one direction.

A request that the patterns take for a report receives the note. The note ends "If it asks for a code change, use dev-orchestrator as usual.", so the agent still reaches dev-orchestrator, through the note's last sentence instead of the routing prompt.

A report that the patterns take for a request is decided as the same report without the marker would be: it receives the note while the throttle holds, and the routing prompt on a new topic (see [Work requests and the throttle](#work-requests-and-the-throttle)). On a new topic, rule 4 classifies it as DEV, and the routing prompt tells a DEV classification "Do not answer inline", so the agent would usually start a recipe for a status line, which is what the marker is meant to prevent.

The first mistake is the milder one, so patterns 2 and 4 look at the whole body and not only at the second word. These marked requests receive the note, and a unit test pins them:

- `fix the tests that failed`, `fix the failed tests` and `update the list of merged branches`, by pattern 4.
- `add a getting started guide`, by pattern 4, because `started` is an outcome word.
- `fix the broken build` and `fix the red test on main`, by pattern 4, because `broken` and `red` are outcome words.
- `fix the case where the list is empty` and `start #12 once #11 has merged`, by pattern 2.

To keep the note away from such a request, send it as an explicit `/dev` command, as in `[coord] /dev fix the tests that failed`, which runs with no hook text, or word it as the change verb and its object without a clause, as in `fix failed tests on PR 12`, which receives the routing prompt on a new topic.

`update on` counts as a report at the start of any body, so a request worded `[coord] update on main and rerun` receives the note as well.

## The coordination note

The note is the constant `COORDINATION_NOTE`. Its full text is:

```text
[coordination] This message starts with the [coord] marker and is coordination traffic between sessions. Answer it or act on it directly, without dev-orchestrator, smart-orchestrator or a recipe. If it asks for a code change, use dev-orchestrator as usual.
```

The note keeps recipes away from replies, reports, acknowledgements and single commands such as retrying an install. It does not keep them away from code changes. The routing prompt's downstream invariant is that development work always runs through `default-workflow`, and a marked request for a code change that the work-request rule does not recognise must still meet it.

The note is emitted on every marked prompt that is not a work request or `/dev` command, at every turn, and on a marked work request that the throttle holds back. It never contains `auto-intent-router`, `launching dev-orchestrator`, `Invoke Skill(skill="dev-orchestrator")` or `[auto-routed]`. The body of the message is never copied into the hook output.

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

The exception is part of rule 8, and the prompt applies the first rule that matches. An unmarked report that starts with a change verb, such as `build failed on PR 12`, is decided by rule 4 as DEV before rule 8 is reached. Only the marker keeps such a report away from the routing prompt; see [Reports that start with a verb](#reports-that-start-with-a-verb).

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
5. Marker: a marked body that is neither an explicit `/dev` command nor a work request receives the note.
6. Topic decision, for unmarked prompts and for the remaining marked bodies, applied to the body: explicit `/dev` command, new topic or throttled. A throttled marked body receives the note.
7. Emit the result.

Provenance takes precedence over the marker. A marked prompt inside a recipe run, including `[coord] build #1740`, produces `{}`. The marker can only replace the routing prompt, or the empty output of the throttle, with the note. It never adds a routing prompt and never starts a recipe itself.

Turn resolution runs before the provenance gate. When the runtime reports no turn number, step 2 increments `observed_turns` and saves the state file, so a prompt stopped at step 3 or 4 can still raise `observed_turns` by one. It never changes `last_rule` or `last_classified_turn`. When the runtime does report a turn number, these gates leave the state file untouched.

An empty `session_id` is treated as `unknown-session`, as a missing one already is.

## Outcomes and `last_rule`

Each outcome is recorded in the state file's `last_rule` field. These values are a stable contract for log readers; renaming one is a breaking change.

| Path | Output | `last_rule` | `last_classified_turn` |
| --- | --- | --- | --- |
| Marked; body is an explicit `/dev` command | `{}` | `explicit_dev_command` | unchanged |
| Marked; body is a work request (starts with a change verb, not a report), new topic | routing prompt | `coordination_explicit_dev` | set to the current turn |
| Marked; body is a work request, not a new topic | coordination note | `coordination_throttled` | unchanged |
| Marked; anything else | coordination note, every turn | `coordination_marker` | unchanged |
| Unmarked explicit `/dev` command | `{}` | `explicit_dev_command` | unchanged |
| Unmarked new topic | routing prompt | `classification` | set to the current turn |
| Unmarked, not a new topic | `{}` | `throttled` | unchanged |

The note path leaves `last_classified_turn` unchanged because no classification took place, so a note never starts or extends the three-turn throttle window. A note still counts as a turn. The next unmarked prompt is judged at its own turn number, so after two notes in a fresh session it is at turn 2: past the rule that routes turns 0 and 1 unconditionally, and subject to the follow-up wording check like any later prompt. In a session where nothing has been classified, `last_classified_turn` is absent and the window does not apply, so a new topic after the notes receives the routing prompt.

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
| `last_classified_turn` | integer or absent, default absent | Turn at which the routing prompt was last emitted; absent until the session's first classification |
| `session_id` | string | Session ID as received |
| `observed_turns` | integer, default `0` | The hook's own prompt counter, used when the runtime sends no turn number |
| `last_rule` | string or absent, default absent | The `last_rule` value of the most recent recorded outcome |

In a session where nothing has been classified, `last_classified_turn` is absent. After two coordination notes in a fresh session, under a runtime that sends no turn number:

```json
{
  "session_id": "lane-3",
  "observed_turns": 2,
  "last_rule": "coordination_marker"
}
```

Files written before this feature have no `last_rule` field and always hold `last_classified_turn`; they load unchanged. An older binary ignores `last_rule`. It cannot read a file without `last_classified_turn`, so it treats that file as missing, which routes the prompt and restarts its own `observed_turns` count at 0.

Before this feature, a state file created by turn resolution held `last_classified_turn: 0` although nothing had been classified, and the throttle read that as a classification at turn 0. In a session whose first two prompts classified nothing (coordination notes, explicit `/dev` commands or prompts stopped by a gate), a new topic at turn 2 or 3 was throttled. It now receives the routing prompt. This applies to unmarked sessions as well.

State writes are best-effort. A write failure is logged at debug level with the error and path, never the prompt, and the hook still returns its output. An unreadable or malformed file is treated as empty, which leans towards emitting the routing prompt, not suppressing it.

## Security properties

- The marker is advisory. No marked prompt leads directly to a recipe; the strongest effect, for a work request, is the routing prompt, and only when the same text without the marker would receive it.
- The provenance, nested-session and workflow-active gates run before the marker is read.
- The prefix comparison cannot panic on short, whitespace-only or multibyte input.
- The work-request and report patterns are literal regexes, each compiled once. The `regex` crate matches in linear time.
- Neither the prompt nor the body is written to the state file or the debug log, and neither is copied into the hook output, so a peer session cannot forge hook context through a coordination message.

## Related

- [UserPromptSubmit Hook API](./user-prompt-submit-hook-api.md#workflow-classification-reminder): the other hooks run on each prompt
- [Dev-Orchestrator Routing Contract](./dev-orchestrator-routing.md): what happens once a prompt is routed to dev-orchestrator
- [Workflow Classifier](./workflow-classifier.md): how the orchestrator classifies a routed task
