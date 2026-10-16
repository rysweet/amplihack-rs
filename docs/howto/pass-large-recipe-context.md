---
title: "Pass Large Recipe Context and Agent Text"
description: "Pass large recipe context with --context-file and --context-from-file, and publish agent-generated text with --body-file and git commit -F, without hitting Argument list too long."
last_updated: 2026-10-16
review_schedule: quarterly
owner: rysweet
doc_type: howto
related:
  - ../reference/large-payload-delivery.md
  - ../concepts/large-payload-delivery.md
  - ../reference/recipe-cli-reference.md
---

# Pass Large Recipe Context and Agent Text

Use these patterns when a recipe value, prompt or GitHub body can grow past a
few kilobytes. Each one keeps the text out of argv, so the spawn cannot fail
with `Argument list too long (os error 7)`.

## Contents

- [Pass one large value from a file](#pass-one-large-value-from-a-file)
- [Pass a whole context as JSON](#pass-a-whole-context-as-json)
- [Read a context value of any size in a bash step](#read-a-context-value-of-any-size-in-a-bash-step)
- [Re-invoke a recipe from a bash step](#re-invoke-a-recipe-from-a-bash-step)
- [Give a nested recipe the context it needs](#give-a-nested-recipe-the-context-it-needs)
- [Create an issue or pull request with agent text](#create-an-issue-or-pull-request-with-agent-text)
- [Commit with an agent-written message](#commit-with-an-agent-written-message)
- [Pass agent text to jq](#pass-agent-text-to-jq)
- [Start auto mode from a prompt file](#start-auto-mode-from-a-prompt-file)
- [Troubleshooting](#troubleshooting)

## Pass one large value from a file

Use `--context-from-file KEY=PATH`. The file's raw UTF-8 contents become the
value; nothing is parsed.

```bash
gh issue view 1570 --json body --jq .body > /tmp/issue-1570.md

amplihack recipe run default-workflow \
  --context-from-file task_description=/tmp/issue-1570.md \
  -c repo_path=.
```

The option can be repeated, once per key.

The value stays out of `amplihack`'s own argv at any size. The total context
handed to the recipe runner must currently stay under 128 KiB, because the
runner cannot yet read a larger context from a file; see
[Troubleshooting](#troubleshooting).

## Pass a whole context as JSON

Use `--context-file PATH` with a file holding one JSON object. Strings are used
as they are; other values become text, and arrays and objects become compact
JSON.

```bash
cat > /tmp/qa-context.json <<'EOF'
{
  "target_path": "crates/amplihack-cli",
  "min_cycles": 3,
  "validation_threshold": 0.85,
  "fix_all_per_cycle": true,
  "categories": ["security", "reliability"]
}
EOF

amplihack recipe run quality-audit-cycle \
  --context-file /tmp/qa-context.json \
  -c repo_path=.
```

The recipe receives `min_cycles=3`, `fix_all_per_cycle=true` and
`categories=["security","reliability"]`.

### Combining sources

When a key is set more than once, the later source in this list wins:

1. `--context-file`
2. `--context-from-file`
3. `-c`

```bash
# task_description comes from the issue file; repo_path from -c overrides the JSON.
amplihack recipe run default-workflow \
  --context-file /tmp/base-context.json \
  --context-from-file task_description=/tmp/issue-1570.md \
  -c repo_path=/home/ryan/src/amplihack-rs
```

## Read a context value of any size in a bash step

The pinned recipe runner sets `$RECIPE_VAR_<key>` in every bash step at any
size. A value too large for the environment is a shell-local variable: the
step's own shell sees it, but `bash other.sh`, `python3 tool.py` and every
other child process the step starts do not. Never put the value back into
argv either; `-c key="$RECIPE_VAR_key"` and `--arg body "$RECIPE_VAR_body"`
fail with `Argument list too long` once the value passes 128 KiB.

Source `recipe_context_value.sh` and write the value to a file. The helper
reads `RECIPE_VAR_<key>` from the step's shell, falls back to
`AMPLIHACK_CONTEXT_FILE` when an older runner set that instead, and exits 3 if
neither holds the key:

```bash
set -euo pipefail
. "${AMPLIHACK_HOME:-${REPO_PATH:-$(pwd)}}/amplifier-bundle/tools/recipe_context_value.sh"

umask 077
HISTORY_FILE=$(mktemp)
trap 'rm -f -- "$HISTORY_FILE"' EXIT
write_context_value cycle_history "$HISTORY_FILE"
```

Nested values use a dotted key, for example
`write_context_value final_requirements.summary "$REQ_FILE"`.

Source the helper with `.`; do not run it as `bash recipe_context_value.sh`.
A separate process cannot see a value the runner spilled, and the bundled
recipes are checked by a test for this.

Avoid copying a `{{key}}` placeholder into a quoted heredoc to get the same
effect. The runner pastes the value into the script, and a value that contains
the delimiter line ends the heredoc and runs the remainder as commands.

## Re-invoke a recipe from a bash step

Write each large value to a private temporary file with `write_context_value`
and pass the path with `--context-from-file`:

```yaml
- id: run-recursive-cycle
  type: bash
  command: |
    set -euo pipefail
    . "${AMPLIHACK_HOME:-${REPO_PATH:-$(pwd)}}/amplifier-bundle/tools/recipe_context_value.sh"
    umask 077
    HISTORY_FILE=$(mktemp)
    TASK_FILE=$(mktemp)
    trap 'rm -f -- "$HISTORY_FILE" "$TASK_FILE"' EXIT
    write_context_value cycle_history "$HISTORY_FILE"
    write_context_value task_description "$TASK_FILE"
    timeout 900 amplihack recipe run quality-audit-cycle \
      --context-from-file cycle_history="$HISTORY_FILE" \
      --context-from-file task_description="$TASK_FILE" \
      -c repo_path="$(write_context_value repo_path -)" \
      -c cycle_number="$(write_context_value next_cycle -)"
  output: final_report
```

An `<OUT>` of `-` writes the value to stdout, which suits short values such as
`repo_path`. Read short values through the helper too, so the step behaves
the same on a runner older than the pin, which moved the whole context to
`AMPLIHACK_CONTEXT_FILE` and unset `RECIPE_VAR_next_cycle` as well.

Do not write `-c cycle_history="$(cat "$HISTORY_FILE")"`. The command
substitution puts the whole file back into one argv element.

Values too large for one environment entry reach the step intact: the runner spills them to
private files and reads them back before the script runs. See
[Values the runner spills](../reference/large-payload-delivery.md#values-the-runner-spills).

## Give a nested recipe the context it needs

A nested `amplihack recipe run` does not inherit the parent's `RECIPE_VAR_*`
variables, their uppercase aliases or `AMPLIHACK_CONTEXT_FILE`. Pass every key
the nested recipe reads:

```yaml
- id: run-nested-workflow
  type: bash
  command: |
    set -euo pipefail
    . "${AMPLIHACK_HOME:-${REPO_PATH:-$(pwd)}}/amplifier-bundle/tools/recipe_context_value.sh"
    umask 077
    TASK_FILE=$(mktemp)
    trap 'rm -f -- "$TASK_FILE"' EXIT
    write_context_value task_description "$TASK_FILE"
    amplihack recipe run default-workflow \
      --context-from-file task_description="$TASK_FILE" \
      -c repo_path="$(write_context_value repo_path -)"
```

`PATH`, `HOME` and the other `AMPLIHACK_*` variables are still inherited. See
[Nested recipe environment](../reference/large-payload-delivery.md#nested-recipe-environment).

## Create an issue or pull request with agent text

Write the text with `write_context_value`, pass it through `write_body_file`
and publish with `--body-file`. Bodies over 60,000 bytes are truncated on a
UTF-8 boundary with a note, which keeps them under GitHub's
65,536-character limit.

```bash
set -euo pipefail
TOOLS="${AMPLIHACK_HOME:-${REPO_PATH:-$(pwd)}}/amplifier-bundle/tools"
. "$TOOLS/recipe_context_value.sh"
. "$TOOLS/workflow_body_file.sh"

umask 077
RAW_FILE=$(mktemp)
BODY_FILE=$(mktemp)
trap 'rm -f -- "$RAW_FILE" "$BODY_FILE"' EXIT
write_context_value pr_body "$RAW_FILE"
write_body_file "$BODY_FILE" "the publish step log" < "$RAW_FILE"

gh pr create --title "$PR_TITLE" --body-file "$BODY_FILE" --base main
```

The same pattern applies to `gh issue create --body-file` and
`gh pr edit --body-file`.

## Commit with an agent-written message

Write the message to a temporary file and use `git commit -F`:

```bash
set -euo pipefail
. "${AMPLIHACK_HOME:-${REPO_PATH:-$(pwd)}}/amplifier-bundle/tools/recipe_context_value.sh"
umask 077
MSG_FILE=$(mktemp)
trap 'rm -f -- "$MSG_FILE"' EXIT
write_context_value commit_message "$MSG_FILE"
git commit -F "$MSG_FILE"
```

Commit messages are not truncated; git has no length limit.

## Pass agent text to jq

`jq --arg name "$VALUE"` places the value in argv. For text of unknown size,
write it to a file and use `--rawfile`, which reads the file as one string:

```bash
set -euo pipefail
. "${AMPLIHACK_HOME:-${REPO_PATH:-$(pwd)}}/amplifier-bundle/tools/recipe_context_value.sh"
umask 077
TASK_FILE=$(mktemp)
trap 'rm -f -- "$TASK_FILE"' EXIT
write_context_value task_description "$TASK_FILE"
jq -n --rawfile task "$TASK_FILE" '{task: $task, status: "merged"}' > summary.json
```

`jq -n '{task: $ENV.TASK_DESCRIPTION}'` is not a substitute. It removes the
second copy that `--arg` would add to argv, but the value is still one
environment string, which has the same 131,072-byte limit, and `$ENV` can only
read a variable that is already set.

## Start auto mode from a prompt file

```bash
gh issue view 1570 --json body --jq .body > /tmp/issue-1570.md
amplihack claude --auto --max-turns 20 --prompt-file /tmp/issue-1570.md
```

`--prompt-file` is a subcommand option, so it goes before any `--`, and it
requires `--auto`. Each turn's prompt is then delivered on stdin for Claude
and by file reference for Copilot and Amplifier. There is no prompt-file form
for `--ultrathink`, `--analyze` or `--fix`.

## Troubleshooting

| Message or symptom | Cause | Fix |
| --- | --- | --- |
| `Argument list too long (os error 7)` | A large value is still in argv or the environment | Find the `-c`, `--body`, `-m`, `--arg` or `-p` that carries it and use the matching pattern above |
| A helper script or tool started by the step sees an empty `$RECIPE_VAR_cycle_history` | The runner spilled the value, and spilled values are not exported to child processes | Source the helper instead of running it, or pass the value on stdin or in a file written with `write_context_value` |
| A step aborts with `RECIPE_VAR_cycle_history: unbound variable` | A runner older than the pin moved the context to `AMPLIHACK_CONTEXT_FILE` | Read the value with `write_context_value`, or install the pinned runner |
| `Dropping env var <NAME> (<N> bytes) from bash step` | A large value's uppercase alias is a name bash treats specially, such as `BASH_ENV` from the key `bash_env` | Read `$RECIPE_VAR_<key>`, which still holds the value, or rename the key. See [Names the runner drops](../reference/large-payload-delivery.md#names-the-runner-drops) |
| `field 'max_env_value_bytes' is retired and ignored` | The recipe still declares the old step field | Delete the field |
| `write_context_value: context key 'cycle_history' is not available (...)` | The key is not in this run's context | Pass it to the run, or, in a nested run, pass it explicitly from the parent step |
| `--context-file /tmp/ctx.json: top-level JSON value must be an object` | The file holds an array or a scalar | Wrap the values in `{ ... }` |
| `--context-from-file: key 'AMPLIHACK_HOME' is reserved` | File sources cannot set `AMPLIHACK_*` keys | Use a recipe context key instead |
| `--context-from-file: expected KEY=PATH` | The `=` is missing | Write `task_description=/tmp/issue.md` |
| A nested recipe fails with `unbound variable` | It relied on an inherited parent alias | Pass the key with `-c` or `--context-from-file` |
| `error: unexpected argument '--context-file' found` from `recipe-runner-rs` | The merged context is over 128 KiB, and the runner cannot yet read it from a file | Keep the total context passed to `amplihack recipe run` under 128 KiB; produce larger values inside the run, for example as a step output |
| `error: unexpected argument '--prompt-file' found` | The amplihack release on this machine or VM predates `--prompt-file` | Update amplihack there |
| `remote command 'ultrathink' is not supported: only 'auto' accepts --prompt-file` | Only auto mode can take a prompt file | Run the remote command as `auto` |

## Related

- [Large Payload Delivery Reference](../reference/large-payload-delivery.md) - Options, validation and constants
- [Large Payload Delivery to Child Processes](../concepts/large-payload-delivery.md) - Background and design
- [Recipe CLI Reference](../reference/recipe-cli-reference.md) - All `amplihack recipe run` options
- [Quality Audit Cycle Recipe](../reference/quality-audit-cycle-recipe.md) - The `run-recursive-cycle` step
