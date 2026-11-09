# amplihack multitask — Command Reference

## Synopsis

```
amplihack multitask <SUBCOMMAND>
```

## Description

Parallel workstream orchestrator. Runs multiple independent development tasks
concurrently, each in its own isolated workspace. Tracks status, enforces
timeouts, and cleans up after merged PRs.

## Subcommands

### run

Launch parallel workstreams from a JSON configuration file.

```
amplihack multitask run <CONFIG> [OPTIONS]
```

| Argument / Flag | Type | Default | Description |
|-----------------|------|---------|-------------|
| `<CONFIG>` | string | — | Path to workstreams JSON config file. Required. |
| `--mode <MODE>` | string | `recipe` | Execution mode: `recipe` (uses recipe runner) or `classic` (direct execution). |
| `--recipe <NAME>` | string | `default-workflow` | Recipe name for recipe mode. Ignored in classic mode. |
| `--max-runtime <SECS>` | integer | — | Override workstream runtime budget in seconds. When reached, the timeout policy applies. |
| `--timeout-policy <POLICY>` | string | — | What to do when `--max-runtime` is exceeded: `interrupt-preserve` (stop and keep artifacts) or `continue-preserve` (let active work finish, keep artifacts). |
| `--dry-run` | bool | `false` | Show what would be executed without launching any workstreams. |

### cleanup

Remove workstream artifacts for workstreams whose PRs have been merged.

```
amplihack multitask cleanup <CONFIG> [OPTIONS]
```

| Argument / Flag | Type | Default | Description |
|-----------------|------|---------|-------------|
| `<CONFIG>` | string | — | Path to workstreams JSON config file. Required. |
| `--dry-run` | bool | `false` | Show what would be deleted without deleting. |

### status

Display the current state of existing workstreams.

```
amplihack multitask status [OPTIONS]
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--base-dir <PATH>` | string | — | Base directory for workstream artifacts. When omitted, uses the default location. |

## Examples

```sh
# Launch workstreams from a config file
amplihack multitask run workstreams.json

# Dry-run to preview what would execute
amplihack multitask run workstreams.json --dry-run

# Use classic mode with a 30-minute timeout
amplihack multitask run workstreams.json --mode classic --max-runtime 1800

# Check workstream status
amplihack multitask status

# Clean up merged workstreams
amplihack multitask cleanup workstreams.json

# Preview cleanup without deleting
amplihack multitask cleanup workstreams.json --dry-run
```

## Exit Codes

| Code | Meaning |
|------|---------|
| `0` | Success (all workstreams completed, or status/cleanup succeeded) |
| `1` | Error (config not found, workstream failures, invalid arguments) |

## Workstream context in recipe mode

In recipe mode each workstream writes its context to `context.json` (mode
0600) and its launcher script runs:

```bash
RECIPE='default-workflow'
exec amplihack recipe run "$RECIPE" --context-file "$CONTEXT_JSON" --verbose
```

The context is never expanded into command-line arguments, so a workstream
with a large task description cannot fail with `Argument list too long`, and
values containing spaces or quotes are passed unchanged. The recipe name is
validated before it is quoted into the script.

When a workstream entry has an `is_critical_code` field, its value is written
to `context.json` byte for byte under the same key. The launcher does not
trim, case-fold, validate or default it; `consensus-workflow` does the
validation. The field must be a JSON string: `"is_critical_code": true` makes the whole
workstreams file fail to parse, and no workstream starts. An entry without
the field writes no `is_critical_code` key. See
the [Consensus Criticality Contract](./consensus-criticality-contract.md#workstreams-field).

## Related

- [recipe Command](./recipe-command.md) — Recipes executed by multitask in recipe mode
- [Recipe context from files](./large-payload-delivery.md#recipe-context-from-files) - How `--context-file` converts JSON values
- [Environment Variables](./environment-variables.md) — Variables that influence workstream execution
