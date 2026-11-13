---
title: Multitask command reference
description: Native workstream commands and Operations routing.
last_updated: 2026-11-13
review_schedule: quarterly
owner: amplihack
doc_type: reference
---

# amplihack multitask — Command Reference

## Synopsis

```
amplihack multitask <SUBCOMMAND>
```

## Description

Runs a validated plan with per-leaf routing and optional issue tracking.
Operations runs in the supplied execution directory; Development retains clone
preparation. The engine preserves dependencies, immutable keys and results in
`<TMPDIR>/amplihack-workstreams/state/canonical-plan.json`. It schedules work
from child-exit and cancellation events, without interval or settle sleeps.

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
| `--max-runtime <SECS>` | integer | — | Legacy runtime-budget metadata; the event scheduler does not impose timed interruption. |
| `--timeout-policy <POLICY>` | string | — | Legacy policy metadata saved for compatibility; no timer is installed. |
| `--dry-run` | bool | `false` | Show what would be executed without launching any workstreams. |

### cleanup

Remove workstream directories when the configured branch has a merged PR.
Completed Operations leaves can remove their owned runtime artefacts without a PR.
Cleanup preserves the caller's execution repository and unowned directories.

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
| `--base-dir <PATH>` | string | — | Base directory for workstream artefacts. When omitted, uses the default location. |

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

## Workstream identity and tracking

Keys are derived from configuration on every invocation. A tracked workstream
uses its issue number; an issue-less workstream uses a slug of its description
(or branch), suffixed with its one-based index. Keys contain 1–64 ASCII
letters, digits, hyphens or underscores, and duplicate derived keys are
rejected during registration. Preparation can already have occurred for earlier
entries before a later registration fails.

Changing issues, descriptions or array order can change keys and paths.
The canonical plan stores the allocated key. Resume that saved plan to keep keys
stable when attaching an issue. Conflicting saved ownership or task identity
stops execution. A per-leaf Boolean `create_issue=true` acquires tracking before
launch only when no valid issue is already present. Unknown placeholders are
errors; supported absence values are normalised before any numeric use.

Tracking calls use the authenticated GitHub CLI with prompts disabled and a
60-second timeout. Failed or uncertain creation is never retried automatically:
the saved acquisition reference is reconciled on resume. Exactly one issue must
carry that reference; missing, duplicate or incomplete search results stop
execution and require reconciliation on GitHub before resuming. Service errors
report the exit status and recovery reference without exposing raw service output.

Dry-run validates the whole plan and shows resolved routes without preparing
workspaces or contacting tracking services. Runtime state and logs use the same
private temporary root across launch, inspection, resumption and cleanup.

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

- [Dev-Orchestrator Routing Contract](./dev-orchestrator-routing.md) — Route selection, optional issues and workstream identity
- [Run Operations and Mixed Requests](../howto/run-operations-and-mixed-work.md) — Running work that has no issue number
- [recipe Command](./recipe-command.md) — Recipes executed by multitask in recipe mode
- [Recipe context from files](./large-payload-delivery.md#recipe-context-from-files) - How `--context-file` converts JSON values
- [Environment Variables](./environment-variables.md) — Variables that influence workstream execution
