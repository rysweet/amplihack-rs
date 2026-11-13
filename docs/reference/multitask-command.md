---
title: Multitask command reference
description: Execute and inspect leaf workstreams with stable identities and optional issues.
last_updated: 2026-11-12
review_schedule: quarterly
owner: amplihack
doc_type: reference
---

# amplihack multitask — Command Reference

> **[IMPLEMENTED - Validation Pending]** Per-leaf routing, optional tracking,
> result bindings and the `reconcile` subcommand describe the issue #2553
> contract. These additions require implementation and validation before use.

## Synopsis

```
amplihack multitask <SUBCOMMAND>
```

## Description

Dependency-aware workstream orchestrator. Runs ready leaves concurrently,
using stable execution UUIDs independently of optional issue tracking.
Development uses isolated workspaces; Operations executes one scoped task in
its trusted repository with a separate private artefact directory.

## Subcommands

### run

Launch workstreams from a JSON configuration file. Operations always uses the
direct adapter in both `recipe` and `classic` modes; neither `--recipe` nor a
per-entry recipe can send it through Development.

The [workstreams JSON schema](orch-run-command.md#workstreams-json-schema) and
[routing contract](dev-orchestrator-routing.md) apply to both entry points.

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

### reconcile (planned)

Record an externally verified outcome for an uncertain tracking attempt or
Operations action. This command does not launch work or change GitHub:

```text
amplihack multitask reconcile <CONFIG> --resolution-file <PATH>
```

`CONFIG` is the original saved normalised plan; `PATH` contains one resolution
object. The [reconciliation contract](workstream-state.md#reconciliation-and-safe-resume)
defines its required identities, revision, evidence, outcome and receipt.
An exclusive lock, ownership validation and revision check protect the update.
Unresolved outcomes remain blocked. Resume separately with `run` using the same
plan and options. See the [resume procedure](../howto/run-operations-and-mixed-work.md#resume-without-repeating-administrative-actions).

### cleanup

Remove eligible owned workstream artefacts. Development retains its existing
delivery and merged-PR checks. Operations cleanup never removes its repository
working directory. Unknown external outcomes are retained for reconciliation.

```
amplihack multitask cleanup <CONFIG> [OPTIONS]
```

| Argument / Flag | Type | Default | Description |
|-----------------|------|---------|-------------|
| `<CONFIG>` | string | — | Path to workstreams JSON config file. Required. |
| `--dry-run` | bool | `false` | Show what would be deleted without deleting. |

### status

Display the current state of existing workstreams by execution identity,
including optional tracking, completion and blocked or uncertain outcomes.

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
0600). The launcher passes it through `amplihack recipe run` with
`--context-file` and the trusted leaf route. Operations uses `operations-direct`.
Classic mode supplies an equivalent scoped context to its direct adapter.

The context is never expanded into command-line arguments, so a workstream
with a large task description cannot fail with `Argument list too long`, and
values containing spaces or quotes are passed unchanged. The recipe name is
validated before it is quoted into the script.

Absent issues are omitted from context and inherited issue-specific variables
are cleared. Only relevant prerequisite results are passed, with provenance.
See [child context and environment](workstream-state.md#child-context-and-environment).

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
