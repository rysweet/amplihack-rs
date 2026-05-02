# amplihack recipe — Reference

Full CLI reference for the `amplihack recipe` subcommand, which lists,
inspects, validates, and runs YAML recipe files.

## Contents

- [Synopsis](#synopsis)
- [Subcommands](#subcommands)
  - [recipe list](#recipe-list)
  - [recipe show](#recipe-show)
  - [recipe validate](#recipe-validate)
  - [recipe run](#recipe-run)
- [Output formats](#output-formats)
- [Recipe file format](#recipe-file-format)
  - [Required fields](#required-fields)
  - [Optional fields](#optional-fields)
  - [Step fields](#step-fields)
  - [Step types](#step-types)
- [Context variables](#context-variables)
  - [Supplying context](#supplying-context)
  - [Context inference](#context-inference)
  - [Context environment variables](#context-environment-variables)
- [Git context behavior](#git-context-behavior)
- [Recipe search path](#recipe-search-path)
- [Recipe runner binary](#recipe-runner-binary)
- [Exit codes](#exit-codes)
- [Related](#related)

---

## Synopsis

```
amplihack recipe <SUBCOMMAND> [OPTIONS]
```

---

## Subcommands

### recipe list

List all recipes discovered in the [recipe search path](#recipe-search-path).

```
amplihack recipe list [--dir <DIR>] [--format <FORMAT>] [--tag <TAG>]... [--verbose]
```

| Flag | Default | Description |
|------|---------|-------------|
| `--dir <DIR>` | (search path) | Search only this directory instead of the default path |
| `--format <FORMAT>` | `table` | Output format: `table`, `json`, or `yaml` |
| `--tag <TAG>` | (none) | Filter by tag; repeat for AND logic (e.g. `--tag ci --tag lint`) |
| `--verbose` | false | Include version, author, and step count in table output |

```sh
# List all recipes (table)
amplihack recipe list

# List with full metadata
amplihack recipe list --verbose

# Filter to recipes tagged both "ci" and "lint"
amplihack recipe list --tag ci --tag lint

# Output as JSON for scripting
amplihack recipe list --format json
```

**Example output (table):**

```
Available Recipes (3):

• default-workflow
  Run the full 23-step development workflow
  Tags: dev, workflow

• smart-orchestrator
  Intelligent task routing and delegation
  Tags: dev, orchestration

• verification
  5-step verification workflow for trivial changes
  Tags: dev, verification
```

**Example output (JSON, --verbose):**

```json
[
  {
    "name": "default-workflow",
    "description": "Run the full 23-step development workflow",
    "version": "1.0.0",
    "author": "amplihack",
    "tags": ["dev", "workflow"],
    "step_count": 23
  }
]
```

---

### recipe show

Display the full contents of a recipe file: metadata, context variables, and
step list.

```
amplihack recipe show <FILE> [--format <FORMAT>] [--no-steps] [--no-context]
```

| Flag | Default | Description |
|------|---------|-------------|
| `<FILE>` | (required) | Path to the recipe YAML file |
| `--format <FORMAT>` | `table` | Output format: `table`, `json`, or `yaml` |
| `--no-steps` | false | Omit the step list from output |
| `--no-context` | false | Omit context variable defaults from output |

```sh
amplihack recipe show ~/.amplihack/.claude/recipes/default-workflow.yaml

amplihack recipe show ~/.amplihack/.claude/recipes/default-workflow.yaml \
  --format json
```

---

### recipe validate

Validate a recipe YAML file and report any structural errors.

```
amplihack recipe validate <FILE> [--format <FORMAT>] [--verbose]
```

| Flag | Default | Description |
|------|---------|-------------|
| `<FILE>` | (required) | Path to the recipe YAML file |
| `--format <FORMAT>` | `table` | Output format: `table`, `json`, or `yaml` |
| `--verbose` | false | Include description and step count in success output |

```sh
# Validate a recipe
amplihack recipe validate ~/.amplihack/.claude/recipes/default-workflow.yaml

# Verbose: show description and step count on success
amplihack recipe validate ~/.amplihack/.claude/recipes/default-workflow.yaml \
  --verbose
```

**Success output (table):**

```
✓ Recipe is valid
  Name: default-workflow
  Description: Run the full 23-step development workflow
  Steps: 23
```

**Failure output (table):**

```
✗ Recipe is invalid
  Error: Every step must have a non-empty 'id' field
```

**Validation rules:**

- `name` field must be present and non-null
- `steps` field must be present with at least one step
- Every step must have a non-empty `id`
- Step `id` values must be unique within the recipe
- `type` field (if present) must be `bash`, `agent`, or `recipe`
- File size must be under 1 MB

**JSON output on failure:**

```json
{
  "valid": false,
  "errors": ["Every step must have a non-empty 'id' field"]
}
```

Exit code is 1 on validation failure; 0 on success.

---

### recipe run

Execute a recipe by delegating to the `recipe-runner-rs` binary.

```
amplihack recipe run <FILE> [-c KEY=VALUE]... [--dry-run] [--verbose]
  [--format <FORMAT>] [--working-dir <DIR>] [--step-timeout <SECONDS>]
```

| Flag | Default | Description |
|------|---------|-------------|
| `<FILE>` | (required) | Path to the recipe YAML file |
| `-c KEY=VALUE` | (none) | Set a context variable; repeat for multiple values |
| `--dry-run` | false | Print the execution plan without running any steps |
| `--verbose` | false | Print recipe name and dry-run status to stderr |
| `--format <FORMAT>` | `table` | Output format for results: `table`, `json`, or `yaml` |
| `--working-dir <DIR>` | `.` | Working directory for step execution |
| `--step-timeout <SECONDS>` | (none) | Apply a global per-step timeout ceiling to **every** step in the recipe. Useful for CI guard rails. `0` disables all step timeouts. When omitted, agent steps run without a per-step timeout and bash steps use the YAML-defined `timeout_seconds` only when present |

Before spawning `recipe-runner-rs`, the Rust CLI always injects `AMPLIHACK_HOME` and, when available, `AMPLIHACK_ASSET_RESOLVER` into the child environment. That gives recipes a stable native way to resolve `amplifier-bundle/...` assets without assuming the Python package layout. Additionally, when `--step-timeout` is provided, the CLI sets [`AMPLIHACK_STEP_TIMEOUT`](./environment-variables.md#amplihack_step_timeout) in the child environment so `recipe-runner-rs` can read and apply the override.

> **No per-step timeouts on agent steps by default.** As of the resolution
> of [issue #439](https://github.com/rysweet/amplihack-rs/issues/439), the
> bundled recipes under `amplifier-bundle/recipes/*.yaml` no longer set
> `timeout:` / `timeout_seconds:` on agent steps. Models routinely think
> longer than any reasonable per-step timeout, and aborting them
> mid-thought corrupts orchestrator state. Agent steps now run to
> completion. To impose a hard ceiling (e.g., for CI), pass
> `--step-timeout <SECONDS>`. Bash steps may still carry a
> `timeout_seconds` value, but only when they call external network
> services (`gh api`, `curl`, `git fetch`, etc.) where a stuck socket
> could hang indefinitely; in that case the floor is **1800s**.

```sh
# Dry run — inspect the plan before executing
amplihack recipe run ~/.amplihack/.claude/recipes/default-workflow.yaml \
  -c task_description="Add rate limiting to the API" \
  --dry-run

# Execute with context variables
amplihack recipe run ~/.amplihack/.claude/recipes/default-workflow.yaml \
  -c task_description="Fix the failing pagination tests" \
  -c repo_path=/home/user/src/myproject

# Override every step with a 30-minute ceiling (CI guard rail)
amplihack recipe run ~/.amplihack/.claude/recipes/default-workflow.yaml \
  -c task_description="Large refactoring task" \
  --step-timeout 1800

# Disable all step timeouts (default behavior for agent steps; explicit here)
amplihack recipe run ~/.amplihack/.claude/recipes/default-workflow.yaml \
  -c task_description="Complex migration requiring extended agent time" \
  --step-timeout 0

# Output results as JSON
amplihack recipe run ~/.amplihack/.claude/recipes/verification.yaml \
  -c task_description="Bump the version number" \
  --format json

# Run an investigation from any directory, including a non-git temp directory
tmpdir=$(mktemp -d)
cd "$tmpdir"
amplihack recipe run ~/.amplihack/.claude/recipes/investigation-workflow.yaml \
  -c task_description="Explain the deployment options" \
  -c repo_path=.
```

**Example output (table, dry run):**

```
Recipe: default-workflow
Status: ✓ Success

Steps:
  ✓ requirements-clarification: completed
    Output: [DRY RUN] Would execute agent: requirements-clarifier
  ✓ implementation-planning: completed
    Output: [DRY RUN] Would execute agent: architect
  ...
```

**Example output (JSON):**

```json
{
  "recipe_name": "default-workflow",
  "success": true,
  "step_results": [
    {
      "step_id": "requirements-clarification",
      "status": "completed",
      "output": "[DRY RUN] Would execute agent: requirements-clarifier",
      "error": ""
    }
  ]
}
```

Step status values: `completed`, `failed`, `skipped`.

---

## Output formats

All `recipe` subcommands support `--format table` (default), `--format json`,
and `--format yaml`.

| Format | Use case |
|--------|----------|
| `table` | Human-readable terminal output |
| `json` | Scripting, CI pipelines, `jq` processing |
| `yaml` | Config file integration |

---

## Recipe file format

A recipe is a YAML file. The top-level document must be a mapping.

### Required fields

| Field   | Type   | Description |
|---------|--------|-------------|
| `name`  | string | Unique identifier for the recipe |
| `steps` | list   | Ordered list of step objects (at least one) |

### Optional fields

| Field         | Type   | Default   | Description |
|---------------|--------|-----------|-------------|
| `description` | string | `""`      | Human-readable description shown in `list` output |
| `version`     | string | `"1.0.0"` | Semantic version |
| `author`      | string | `""`      | Author or team name |
| `tags`        | list   | `[]`      | Labels used to filter with `--tag` |
| `context`     | map    | `{}`      | Default values for context variables |

### Step fields

| Field         | Type    | Required | Description |
|---------------|---------|----------|-------------|
| `id`          | string  | yes      | Unique step identifier within the recipe |
| `type`        | string  | no       | `bash`, `agent`, or `recipe` (inferred if omitted) |
| `command`     | string  | no       | Shell command to run (for `bash` steps) |
| `agent`       | string  | no       | Agent name to invoke (for `agent` steps) |
| `prompt`      | string  | no       | Prompt to send to the agent |
| `condition`   | string  | no       | Expression; step is skipped when false |
| `output`      | string  | no       | Variable name to store step output |
| `parse_json`  | bool    | no       | Parse step output as JSON |
| `timeout`     | integer | no       | Step timeout in seconds. **Discouraged on `agent` steps** — agent reasoning is variable and per-step timeouts cause spurious mid-thought aborts (see [issue #439](https://github.com/rysweet/amplihack-rs/issues/439)). Permitted on `bash` steps that perform external network I/O (`gh`, `curl`, `git fetch`); use a generous floor of **1800s** to guard against stuck sockets, not to bound work. Omit otherwise. |
| `working_dir` | string  | no       | Working directory override for this step |
| `recipe`      | string  | no       | Sub-recipe path (for `recipe` steps) |
| `context`     | map     | no       | Context overrides for a sub-recipe step |
| `mode`        | string  | no       | Step execution mode |
| `auto_stage`  | bool    | no       | Auto-stage git changes after step completes |

### Step types

| Type     | Description |
|----------|-------------|
| `bash`   | Run a shell command. Uses the `command` field. |
| `agent`  | Invoke an amplihack agent. Uses `agent` and `prompt` fields. |
| `recipe` | Execute a sub-recipe. Uses the `recipe` field. |

---

## Context variables

Context variables are key-value string pairs that fill template slots in a
recipe's step commands and prompts.

### Supplying context

Pass context on the command line with `-c`:

```sh
amplihack recipe run recipe.yaml \
  -c task_description="Refactor the auth module" \
  -c repo_path=/home/user/src/myapp
```

Multiple `-c` flags are supported. Format is `key=value`.

### Context inference

If a required context variable is missing from `-c`, `amplihack recipe run`
checks environment variables in this order:

1. `AMPLIHACK_CONTEXT_<KEY>` (uppercased key) for any variable
2. `AMPLIHACK_TASK_DESCRIPTION` for the `task_description` key
3. `AMPLIHACK_REPO_PATH` for the `repo_path` key (defaults to `.`)

```sh
export AMPLIHACK_TASK_DESCRIPTION="Add OpenAPI docs to all endpoints"
amplihack recipe run ~/.amplihack/.claude/recipes/default-workflow.yaml
# task_description is inferred from $AMPLIHACK_TASK_DESCRIPTION
```

Recipe defaults in the `context:` block are the lowest-priority source.

### Context environment variables

| Variable | Applies to |
|----------|-----------|
| `AMPLIHACK_CONTEXT_<KEY>` | Any context variable named `<key>` |
| `AMPLIHACK_TASK_DESCRIPTION` | `task_description` |
| `AMPLIHACK_REPO_PATH` | `repo_path` |

---

## Git context behavior

`amplihack recipe run` does not require the current working directory to be a
Git repository unless the selected recipe or step needs Git semantics. Recipes
validate universal inputs first, such as `task_description` and whether
`repo_path` exists, then each host-tool step checks its own prerequisites.
This keeps routing, Q&A, and investigation recipes usable from existing scratch
directories, containers, and temporary folders.

The bundled recipes follow these Git-context rules:

| Recipe or step type | Git requirement | Behavior outside a Git repository |
|---------------------|-----------------|-----------------------------------|
| `smart-orchestrator` routing and dry-runs | Optional | Runs classification and routing. Dry-runs stop there and do not execute downstream Git-required steps; normal runs let selected Git-only steps report their own precondition. |
| `investigation-workflow` and Q&A workflows | Not required | Do not require Git. History-specific analysis is skipped or marked unavailable when no repository is present. |
| Development, publish, PR, worktree, and TDD workflow steps | Required when they create branches, commits, worktrees, PRs, or inspect tracked changes | Fail before the Git operation with a structured, actionable precondition error. |
| Telemetry and status-only Git checks | Optional | Print a visible `[skip] not a git repo ...` note and continue. |

Strict Git-required steps fail with an error shaped like:

```text
ERROR: step <workflow>/<step> requires a git repo at /work/app; either `git init` or rerun from a checkout
```

That error means the recipe is valid, but the selected workflow needs repository
state. To resolve it, either initialize the directory:

```sh
git init
amplihack recipe run ~/.amplihack/.claude/recipes/default-workflow.yaml \
  -c task_description="Implement the feature" \
  -c repo_path=.
```

or rerun from an existing checkout:

```sh
cd /home/user/src/myproject
amplihack recipe run ~/.amplihack/.claude/recipes/default-workflow.yaml \
  -c task_description="Implement the feature" \
  -c repo_path=.
```

Optional Git telemetry never hides failures with `|| true`. When no repository
is present, it emits a visible skip note instead:

```text
[skip] not a git repo at /tmp/amplihack-demo; skipping operations file-change git telemetry
```

This distinction is intentional: optional observations may be skipped, but
required host-tool work must fail before doing partial or misleading work.

---

## Recipe search path

`amplihack recipe list` (without `--dir`) searches these directories in order,
later entries overriding earlier ones with the same recipe name:

1. `<git-repo-root>/amplifier-bundle/recipes/` (detected by walking up from cwd)
2. `~/.amplihack/.claude/recipes/`
3. `amplifier-bundle/recipes/` (relative to cwd)
4. `src/amplihack/amplifier-bundle/recipes/` (relative to cwd)
5. `.claude/recipes/` (relative to cwd)

When a recipe name appears in multiple directories, the last matching file wins.
This allows local overrides of bundled recipes.

---

## Recipe runner binary

`amplihack recipe run` delegates actual execution to a separate
`recipe-runner-rs` binary. The binary is located using these sources in order:

1. `RECIPE_RUNNER_RS_PATH` environment variable
2. `recipe-runner-rs` on `$PATH`
3. `~/.cargo/bin/recipe-runner-rs`
4. `~/.local/bin/recipe-runner-rs`

If no binary is found, `recipe run` fails with:

```
recipe-runner-rs binary not found. Install it:
  cargo install --git https://github.com/rysweet/amplihack-recipe-runner
or set RECIPE_RUNNER_RS_PATH.
```

Set `RECIPE_RUNNER_RS_PATH` to point at a custom build or wrapper:

```sh
export RECIPE_RUNNER_RS_PATH=/opt/amplihack/bin/recipe-runner-rs
amplihack recipe run recipe.yaml -c task_description="..."
```

---

## Exit codes

| Exit code | Meaning |
|-----------|---------|
| 0 | Success (list, show, validate passed; run completed with all steps succeeded) |
| 1 | Error (validate failed; run failed; recipe-runner-rs not found; malformed context args) |

---

## Failure modes

Recipe execution can fail at multiple levels. For diagnosis:

| Failure | Symptom | See |
|---|---|---|
| Runner binary missing | `recipe-runner-rs: command not found` | [Recipe search path](#recipe-search-path) and [Binary Resolution](./binary-resolution.md) |
| Step timeout | Bash step killed after `timeout_seconds` (network-I/O steps only; agent steps no longer carry per-step timeouts) | [Recipe Executor Environment](./recipe-executor-environment.md) |
| Routing gap (no step runs) | Empty result after execution phase | [Smart-Orchestrator Recovery](../concepts/smart-orchestrator-recovery.md) |
| Duplicate issues filed | Repeated `gh issue create` on retries | [Issue Deduplication](./issue-dedup.md) |
| Condition parse error | `condition` field uses unsupported expression | [Agentic Step Patterns](../concepts/agentic-step-patterns.md) |
| Context variable missing | `{{var}}` renders as empty string | [Context variables](#context-variables) |

For general troubleshooting, see [Troubleshoot Recipe Execution Failures](../howto/troubleshoot-recipe-execution.md).

---

## Related

- [Run a Recipe End-to-End](../howto/run-a-recipe.md) — Step-by-step guide to executing recipes
- [Environment Variables](./environment-variables.md) — `AMPLIHACK_CONTEXT_*`, `AMPLIHACK_TASK_DESCRIPTION`, `RECIPE_RUNNER_RS_PATH`
- [Parity Test Scenarios](./parity-test-scenarios.md) — `tier4-recipe-run.yaml` test cases
- [Agent Binary Routing](../concepts/agent-binary-routing.md) — How `AMPLIHACK_AGENT_BINARY` affects recipe step execution
- [Recipe Runner Architecture](../concepts/recipe-runner-architecture.md) — Why the runner is an external binary
- [Agentic Step Patterns](../concepts/agentic-step-patterns.md) — When to use bash vs agent vs recipe steps
