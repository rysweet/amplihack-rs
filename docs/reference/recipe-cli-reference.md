# Recipe CLI Reference

Command-line reference for `amplihack recipe`.

## Contents

- [Subcommands](#subcommands)
- [amplihack recipe list](#amplihack-recipe-list)
- [amplihack recipe run](#amplihack-recipe-run)
- [amplihack recipe validate](#amplihack-recipe-validate)
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

This section includes the planned finished-state transparency contract.

```bash
amplihack recipe run <RECIPE> [-c KEY=VALUE]... [OPTIONS]
```

| Option | Description | Default |
| --- | --- | --- |
| `<RECIPE>` | Recipe name or recipe YAML path | required |
| `--context KEY=VALUE`, `-c KEY=VALUE` | Set a context variable; repeat for multiple values | none |
| `--dry-run` | Show the execution plan without running steps | `false` |
| `--verbose`, `-v` | Add diagnostic detail to progress output | `false` |
| `--format <format>`, `-f <format>` | Final stdout format: `table`, `json`, or `yaml` | `table` |
| `--working-dir <dir>`, `-w <dir>` | Working directory for recipe execution | current directory |
| `--step-timeout <seconds>` | Set `AMPLIHACK_STEP_TIMEOUT` for every step; `0` disables step timeouts | omitted |

Context uses repeated `KEY=VALUE` flags, not a JSON blob:

```bash
amplihack recipe run default-workflow \
  -c task_description="Add user authentication" \
  -c repo_path=/home/user/src/myapp
```

Capture the final JSON result while still watching live progress:

```bash
amplihack recipe run default-workflow \
  -c task_description="Add user authentication" \
  -c repo_path=. \
  --format json > result.json
```

Progress, heartbeats, and failure diagnostics are written to `stderr`. The final
result is written to `stdout` in the selected format. `--verbose` adds detail;
basic progress does not require it.

`--progress` is intentionally unsupported. Passing it should fail fast with an
actionable message explaining that progress is already emitted to `stderr` by
default and that `--verbose` only increases diagnostic detail.

`amplihack recipe run` does not support `--adapter`, `--resume-from`,
`--stop-at`, `--output`, `--interactive`, or `--quiet`.

## amplihack recipe validate

Validate a recipe YAML file without executing it.

```bash
amplihack recipe validate <FILE> [OPTIONS]
```

| Option | Description | Default |
| --- | --- | --- |
| `<FILE>` | Recipe YAML file to validate | required |
| `--verbose`, `-v` | Include validation details | `false` |
| `--format <format>`, `-f <format>` | Output format: `table`, `json`, or `yaml` | `table` |

```bash
amplihack recipe validate ~/.amplihack/.claude/recipes/default-workflow.yaml \
  --verbose
```

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

## See also

- [amplihack recipe Reference](./recipe-command.md) - Detailed command behavior
- [Run a Recipe End-to-End](../howto/run-a-recipe.md) - Task-oriented usage
- [Recipe Runner Logging Reference](./recipe-runner-logging.md) - stderr progress, heartbeats, bounded snippets, and JSONL events
