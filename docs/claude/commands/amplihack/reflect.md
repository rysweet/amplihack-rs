---
name: amplihack:reflect
disable-model-invocation: true
version: 1.0.0
description: Reflection and meta-cognitive analysis of sessions
triggers:
  - "Analyze session"
  - "Run reflection"
  - "Identify improvements"
  - "Session retrospective"
---

# /amplihack:reflect - Session Reflection Analysis

## Input Validation

@~/.amplihack/.claude/context/AGENT_INPUT_VALIDATION.md

## Usage

```bash
/amplihack:reflect analyze --session session-id --messages messages.json
/amplihack:reflect state --session session-id
/amplihack:reflect clear --session session-id
```

Run the corresponding `amplihack reflect` subcommand with the supplied arguments.

## Purpose

Identify patterns in recent assistant responses and inspect the saved analysis
state for a session. Automatic Stop-hook reflection provides separate,
AI-generated session feedback.

## How It Works

1. **Session Analysis**: Manual analysis reads a supplied messages JSON file
2. **Pattern Detection**: Keyword heuristics inspect the two most recent assistant responses
3. **State Storage**: Analysis saves the session state and returns a report

## Environment Control

The Stop hook reads these variables for automatic reflection:

- **AMPLIHACK_SKIP_REFLECTION**: Any non-empty value disables automatic reflection,
  including `0` or `false`.
- **AMPLIHACK_ENABLE_REFLECTION**: `1`, `true` or `yes` (case-insensitive) enables
  reflection unless the skip variable is non-empty or the reflection lock exists.
  Other values fall back to the configuration file, so `false` does not override
  an enabled configuration.

These variables do not control manual `amplihack reflect` commands.

## What Gets Analysed

### Manual Analysis

- Error indicators: `error` or `failed`
- Timeout indicators: `timeout` or `timed out`
- Repeated retries: two or more occurrences of `tried`
- Optional error content supplied with `--error` for contextual error analysis

### Stop-Hook Analysis

- Session conversation from the transcript
- Power-steering redirect history, when available
- Task feedback, workflow adherence and learning opportunities

## Output Format

Manual commands accept `--format text` (the default) or `--format json`.
`text` prints compact JSON; `json` prints indented JSON.

- `analyze` returns `session_id`, `state`, `lightweight`, `error_analysis` and `locked`.
- `state` returns `session_id`, `state_file` and `data`.
- `clear` returns `session_id`, `cleared` and `state_file`.

An analysis with detected patterns or contextual error analysis saves the state
as `awaiting_approval`; otherwise it saves `completed`. If another manual
reflection is in progress, the report has `locked: true` and analysis is skipped.

## Manual Invocation

```bash
amplihack reflect analyze --session session-id --messages messages.json
amplihack reflect state --session session-id --format json
amplihack reflect clear --session session-id
```

- `analyze` requires `--session` and `--messages`. It also accepts `--error`,
  `--runtime-dir` and `--format`.
- `state` and `clear` require `--session`. They also accept `--runtime-dir` and
  `--format`.
- `--runtime-dir` defaults to `~/.amplihack/runtime` for all three commands.
  If the home directory is unavailable, the default is `.amplihack/runtime`
  relative to the working directory.
- `clear` resets the selected session's analysis state to `idle`.

The messages file must contain a JSON array. Each object requires a `role`
(`user`, `assistant`, `system` or `tool`) and a string `content`:

```json
[
  {"role": "user", "content": "Check the build."},
  {"role": "assistant", "content": "The build failed with an error."}
]
```

## Automatic Invocation

The Stop hook considers reflection after lock mode and power steering. Its
reflection settings are checked in this order:

1. A non-empty `AMPLIHACK_SKIP_REFLECTION` disables reflection.
2. An existing `.claude/runtime/reflection/.reflection_lock` disables reflection.
3. `AMPLIHACK_ENABLE_REFLECTION` set to `1`, `true` or `yes` enables reflection.
4. Otherwise, `.claude/tools/amplihack/.reflection_config` must contain
   `"enabled": true`. Missing, invalid or disabled configuration leaves reflection
   disabled by default.

When enabled, the hook invokes the agent CLI for session feedback. It saves
`FEEDBACK_SUMMARY.md` in the session's logs directory, and timestamped feedback
and `current_findings.md` in the session's reflection directory. The hook presents
feedback before allowing the session to end. A presentation marker prevents it
from presenting the same feedback again on the next Stop call.

## Customisation

### Disable Automatic Reflection

```bash
export AMPLIHACK_SKIP_REFLECTION=1
```

### Enable Automatic Reflection

Ensure the skip variable is empty or unset and the reflection lock is absent, then:

```bash
export AMPLIHACK_ENABLE_REFLECTION=true
```

## Examples

### Session Analysis

```bash
amplihack reflect analyze --session session-id --messages messages.json \
  --error "File not found: config.json" --format json
```

### Check Status

```bash
amplihack reflect state --session session-id --runtime-dir .reflection-runtime
```

### Reset Analysis State

```bash
amplihack reflect clear --session session-id --runtime-dir .reflection-runtime
```

## Integration Points

### With Stop Hook

- Runs when automatic reflection is enabled and earlier Stop checks allow it
- Saves feedback in the session's state directories
- Respects the environment settings and configuration above

## Implementation Instructions

When the user invokes `/amplihack:reflect`, follow these steps:

### 1. Parse Command Arguments

Select `analyze`, `state` or `clear`. Obtain the required session ID and, for
analysis, the path to a messages JSON file. Do not infer a last session or messages
file when none was supplied.

### 2. For Reflection Analysis

Run `amplihack reflect analyze` with `--session`, `--messages` and any supplied
optional inputs. Present the returned report and saved analysis state.

### 3. For State Inspection or Reset

Run `amplihack reflect state` to inspect the selected session's saved state, or
`amplihack reflect clear` to reset it. Pass the same `--runtime-dir` used for
analysis when one was supplied.

## Configuration File

Automatic Stop-hook reflection reads the project file
`.claude/tools/amplihack/.reflection_config`:

```json
{
  "enabled": false
}
```

## Remember

- Manual analysis and automatic Stop-hook feedback are separate operations
- Use the returned report to decide what improvements to make
- Automatic reflection is opt-in and disabled by default
