---
title: Recipe CLI Reference
description: Recipe commands, text encoding and validation rules.
type: reference
updated: 2026-11-05
---

# Recipe CLI Reference

Command-line reference for `amplihack recipe`.

## Contents

- [Subcommands](#subcommands)
- [amplihack recipe list](#amplihack-recipe-list)
- [amplihack recipe run](#amplihack-recipe-run)
- [amplihack recipe validate](#amplihack-recipe-validate)
- [Recipe text encoding](#recipe-text-encoding)
- [amplihack recipe show](#amplihack-recipe-show)
- [Environment variables](#environment-variables)
- [Exit codes](#exit-codes)

## Subcommands

```bash
amplihack recipe <SUBCOMMAND> [OPTIONS]
```

`amplihack recipe` has no recipe-specific global flags beyond standard help.
Options such as `--verbose` and `--format` belong to individual subcommands.

## amplihack recipe list

List discovered recipes.

```bash
amplihack recipe list [RECIPE_DIR] [OPTIONS]
```

| Option | Description | Default |
| --- | --- | --- |
| `RECIPE_DIR` | Optional directory to search instead of the default recipe search path | default search path |
| `--format <format>`, `-f <format>` | Output format: `table`, `json`, or `yaml` | `table` |
| `--tags <tag>`, `-t <tag>` | Filter by tag; repeat for multiple tags | none |
| `--verbose`, `-v` | Include extra recipe metadata | `false` |

```bash
amplihack recipe list --format json
amplihack recipe list ~/.amplihack/.claude/recipes --tags dev --verbose
```

## amplihack recipe run

Execute a workflow recipe through `recipe-runner-rs`.

This section defines the stream, progress, and run-correlation contract.

```bash
amplihack recipe run <RECIPE> [--context-file PATH] [--context-from-file KEY=PATH]... [-c KEY=VALUE]... [OPTIONS]
```

| Option | Description | Default |
| --- | --- | --- |
| `<RECIPE>` | Recipe name or recipe YAML path | required |
| `--context KEY=VALUE`, `-c KEY=VALUE` | Set a context variable; repeat for multiple values | none |
| `--context-file <PATH>` | Read context from a file holding one JSON object; non-string values become text | none |
| `--context-from-file KEY=PATH` | Set `KEY` to the raw UTF-8 contents of `PATH`; repeat for multiple keys | none |
| `--dry-run` | Show the execution plan without running steps | `false` |
| `--verbose`, `-v` | Add diagnostic detail to progress output | `false` |
| `--format <format>`, `-f <format>` | Final stdout format: `table`, `json`, or `yaml` | `table` |
| `--working-dir <dir>`, `-w <dir>` | Working directory for recipe execution | current directory |
| `--step-timeout <seconds>` | Set `AMPLIHACK_STEP_TIMEOUT` for every step; `0` disables step timeouts | omitted |

Short context uses repeated `KEY=VALUE` flags:

```bash
amplihack recipe run default-workflow \
  -c task_description="Add user authentication" \
  -c repo_path=/home/user/src/myapp
```

Values that can exceed a few kilobytes, such as an issue body or a cycle
history, come from files so that they never occupy an argv element:

```bash
amplihack recipe run default-workflow \
  --context-from-file task_description=/tmp/issue-1570.md \
  -c repo_path=.
```

When a key is set by more than one source, `-c` wins over
`--context-from-file`, which wins over `--context-file`. File sources reject
keys outside `[A-Za-z0-9_]+`, `AMPLIHACK_*` keys, NUL bytes and files over
16 MiB. See [Recipe context from files](./large-payload-delivery.md#recipe-context-from-files)
for the conversion and validation rules, and
[Pass Large Recipe Context](../howto/pass-large-recipe-context.md) for worked
examples.

Capture the final JSON result while still watching live progress:

```bash
amplihack recipe run default-workflow \
  -c task_description="Add user authentication" \
  -c repo_path=. \
  --format json > result.json
```

Progress, heartbeats, correlation pointer lines, and failure diagnostics are
written to `stderr`. The final result is written to `stdout` in the selected
format. `--verbose` adds detail; basic progress does not require it.

Every run receives a UUID `run_id`. The same value appears in early and final
`amplihack.recipe.log_pointer` stderr lines, in the child environment as
`AMPLIHACK_RECIPE_RUN_ID`, and in JSON/YAML final results as `run_id` when a
result can be produced.

`--progress` is intentionally unsupported. Passing it fails with an
actionable message explaining that progress is already emitted to `stderr` by
default and that `--verbose` only increases diagnostic detail.

`amplihack recipe run` does not support `--adapter`, `--resume-from`,
`--stop-at`, `--output`, `--interactive`, or `--quiet`.

## amplihack recipe validate

Validate a recipe file containing YAML or JSON without executing it.

```bash
amplihack recipe validate <FILE> [OPTIONS]
```

| Option | Description | Default |
| --- | --- | --- |
| `<FILE>` | Recipe file containing YAML or JSON to validate | required |
| `--verbose`, `-v` | Include validation details | `false` |
| `--format <format>`, `-f <format>` | Output format: `table`, `json`, or `yaml` | `table` |

```bash
amplihack recipe validate ~/.amplihack/.claude/recipes/default-workflow.yaml \
  --verbose
```

## Recipe text encoding

The CLI recipe parser accepts UTF-8 YAML and JSON. JSON strings may contain
supplementary Unicode characters directly or encode them as valid UTF-16
surrogate pairs. Both forms produce identical decoded recipe text. No flag,
environment variable or configuration change is required.

These complete recipes decode to the same prompt:

```json
{"name":"unicode-review","steps":[{"id":"review","type":"agent","prompt":"Review: \uD83D\uDE80, \uD834\uDD1E.\nKeep punctuation!"}]}
```

```json
{"name":"unicode-review","steps":[{"id":"review","type":"agent","prompt":"Review: 🚀, 𝄞.\nKeep punctuation!"}]}
```

The decoded prompt contains exactly these two lines:

```text
Review: 🚀, 𝄞.
Keep punctuation!
```

JSON escaping still applies. The JSON string `"\\uD83D\\uDE80"` decodes to
the literal text `\uD83D\uDE80`. The parser preserves those backslashes and
does not interpret the decoded text again. Actual lone, reversed, interrupted
or malformed surrogate escapes are rejected, including `"\uD83D"`,
`"\uDE80"`, `"\uDE80\uD83D"`, `"\uD83Dx\uDE80"` and `"\uD83G"`.

The parser tries YAML decoding first. If decoding fails, it tries JSON decoding
on the original input, requiring a complete JSON document without trailing
content. If both decoders fail, it returns the original YAML error. A schema
error after successful YAML decoding does not trigger a JSON retry. Existing
YAML syntax and scalar behaviour remain unchanged.

Both decoding paths apply the same recipe validation. The original file and
input limits remain 1,000,000 bytes, measured before escape decoding. Recipes
require a top-level mapping, a string `name`, non-empty `steps` and non-blank,
unique step identifiers. Field types and the supported explicit step types
(`bash`, `agent` and `recipe`) remain unchanged, as does step-type inference.
Duplicate mapping keys are rejected at every depth, including context and
additional fields. Keys that become equal after escape decoding also count
as duplicates. Additional fields remain available in the parsed recipe;
unknown-field and retired-field warning behaviour remains unchanged.

This encoding contract applies to CLI recipe parsing and pre-validation.
Recipe execution passes the original file to `recipe-runner-rs`, whose parsing
support is independent. Successful CLI validation does not establish runner
compatibility or change formatter behaviour.

## amplihack recipe show

Display recipe metadata, context defaults, and steps.

```bash
amplihack recipe show <RECIPE> [OPTIONS]
```

| Option | Description | Default |
| --- | --- | --- |
| `<RECIPE>` | Recipe name or recipe YAML path | required |
| `--format <format>`, `-f <format>` | Output format: `table`, `json`, or `yaml` | `table` |
| `--no-steps` | Omit step details | `false` |
| `--no-context` | Omit context details | `false` |

```bash
amplihack recipe show default-workflow --format yaml
```

## Environment variables

| Variable | Applies to | Description |
| --- | --- | --- |
| `RECIPE_RUNNER_RS_PATH` | `run` | Absolute path to a `recipe-runner-rs` binary. Used before `$PATH` lookup. |
| `AMPLIHACK_RECIPE_RUN_ID` | runner child process | Stable UUID generated by `amplihack recipe run` and injected into the child. Treat as read-only. |
| `AMPLIHACK_STEP_TIMEOUT` | runner child process | Set by `--step-timeout`; read by the runner as a global per-step timeout hint. |
| `AMPLIHACK_RECIPE_HEARTBEAT_INTERVAL_SECONDS` | runner | Heartbeat interval in seconds. `0` disables heartbeat lines. |
| `AMPLIHACK_RECIPE_SNIPPET_LINES` | runner | Maximum recent output lines retained per child source and stream. |
| `AMPLIHACK_RECIPE_SNIPPET_BYTES` | runner | Maximum recent output bytes retained per child source and stream. |
| `AMPLIHACK_RECIPE_LOG_JSONL` | runner | Optional path for structured JSONL lifecycle, heartbeat, output snippet, and failure events. |

See [Environment Variables](./environment-variables.md) for full details.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Command succeeded |
| `1` | Command failed, validation failed, recipe execution failed, runner missing, or arguments were malformed |

For development workflows, exit `0` also requires terminal-state evidence. A
code-change task that stops after planning, analysis, design, or worktree setup
exits `1` with `FAILED_MISSING_TERMINAL_EVIDENCE` unless it reaches
implementation/verification, publish/PR/merge state, or an explicit no-op state.
The terminal-state shell helper has its own direct invocation exit code `2` for
invalid helper usage, but `amplihack recipe run` surfaces helper failures as
recipe failures and exits nonzero, currently `1`. See
[Workflow Terminal-State Reference](./workflow-terminal-state.md).

## See also

- [amplihack recipe Reference](./recipe-command.md) - Detailed command behavior
- [Run a Recipe End-to-End](../howto/run-a-recipe.md) - Task-oriented usage
- [Recipe Run Correlation Reference](./recipe-run-correlation.md) - Stable run IDs and log pointer events
- [Recipe Runner Logging Reference](./recipe-runner-logging.md) - stderr progress, heartbeats, bounded snippets, and JSONL events
- [Workflow Terminal-State Reference](./workflow-terminal-state.md) - Development workflow completion evidence and visible failure contract
