# Run a Recipe End-to-End

How to use `amplihack recipe run` to execute a YAML recipe through the Rust CLI
— from finding the right recipe to inspecting results.

## Before you start

- `amplihack` is installed (run `amplihack --version` to confirm)
- `recipe-runner-rs` is on your `$PATH` or `RECIPE_RUNNER_RS_PATH` is set
- You have a recipe YAML file to run (see [recipe search path](#find-available-recipes))

---

## Find available recipes

List all recipes `amplihack` can find:

```sh
amplihack recipe list
```

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

Recipes are discovered from `~/.amplihack/.claude/recipes/` and the current
repository's `amplifier-bundle/recipes/` directory. See
[Recipe search path](../reference/recipe-command.md#recipe-search-path) for
the full resolution order.

---

## Inspect a recipe before running

Before executing, check what a recipe does:

```sh
amplihack recipe show ~/.amplihack/.claude/recipes/default-workflow.yaml
```

Validate the file is well-formed:

```sh
amplihack recipe validate ~/.amplihack/.claude/recipes/default-workflow.yaml
```

```
✓ Recipe is valid
  Name: default-workflow
```

---

## Dry run first

Always dry-run a recipe before executing it for the first time. This shows every
step that will run without actually running anything:

```sh
amplihack recipe run ~/.amplihack/.claude/recipes/default-workflow.yaml \
  -c task_description="Add rate limiting to the API" \
  --dry-run
```

```
Recipe: default-workflow
Status: ✓ Success

Steps:
  ✓ requirements-clarification: completed
    Output: [DRY RUN] Would execute agent: requirements-clarifier
  ✓ implementation-planning: completed
    Output: [DRY RUN] Would execute agent: architect
  ✓ code-implementation: completed
    Output: [DRY RUN] Would execute agent: builder
  ...
```

A dry run exits 0 if the recipe is valid and all steps would be attempted.

---

## Execute the recipe

Run the recipe with context variables describing your task:

```sh
amplihack recipe run ~/.amplihack/.claude/recipes/default-workflow.yaml \
  -c task_description="Add rate limiting to the API"
```

For tasks in a specific repository:

```sh
amplihack recipe run ~/.amplihack/.claude/recipes/default-workflow.yaml \
  -c task_description="Refactor the authentication module" \
  -c repo_path=/home/user/src/myapp
```

The command emits live progress to stderr while the recipe runs. The output
shows each step as it starts, heartbeats for long-running steps, and completion
or failure. It also prints an early correlation pointer before the child runner
starts:

```
amplihack.recipe.log_pointer {"schema_version":1,"event":"early","run_id":"5b60657b-76ef-4f49-8a22-8b89ed75f43e","recipe_name":"default-workflow","worktree":"/home/user/src/myapp","branch":"main","runner_path":"/home/user/.local/bin/recipe-runner-rs"}
[recipe default-workflow] started (23 steps)
[step 01/23 requirements-clarification] started agent=prompt-writer
[step 01/23 requirements-clarification] completed elapsed=24s
[step 02/23 implementation-planning] started agent=architect
[step 02/23 implementation-planning] heartbeat elapsed=60s status=running phase=agent agent=architect
[step 02/23 implementation-planning] completed elapsed=91s
```

The final result is printed to stdout in the requested format. This keeps
human-readable progress separate from machine-readable output.

At the end of the run, stderr includes a final pointer with the same `run_id` and
the terminal status:

```text
amplihack.recipe.log_pointer {"schema_version":1,"event":"final","run_id":"5b60657b-76ef-4f49-8a22-8b89ed75f43e","recipe_name":"default-workflow","status":"success","child_pid":41822,"exit_code":0,"runner_path":"/home/user/.local/bin/recipe-runner-rs"}
```

---

## Run from a non-git directory

Routing, Q&A, and investigation recipes can run from any existing directory.
This is useful for scratch analysis, CI smoke checks, containers, and temporary
workspaces where no checkout exists yet.

```sh
tmpdir=$(mktemp -d)
cd "$tmpdir"

amplihack recipe run ~/.amplihack/.claude/recipes/investigation-workflow.yaml \
  -c task_description="Compare the available deployment approaches" \
  -c repo_path=.
```

`smart-orchestrator` can also be dry-run outside Git to inspect routing:

```sh
tmpdir=$(mktemp -d)
cd "$tmpdir"

amplihack recipe run ~/.amplihack/.claude/recipes/smart-orchestrator.yaml \
  -c task_description="hello" \
  -c repo_path=. \
  --dry-run
```

Dry-run inspects routing only; it does not execute downstream Git-required
steps.

If routing selects a development, publish, PR, worktree, or TDD workflow, the
Git-dependent step checks for repository state before running `git`. Outside a
checkout it fails with a clear precondition error:

```text
ERROR: step <workflow>/<step> requires a git repo at /tmp/demo; either `git init` or rerun from a checkout
```

Initialize the directory with `git init` or rerun the recipe from an existing
checkout when you need branch, worktree, commit, diff, or pull-request behavior.
Optional Git telemetry prints an explicit `[skip] not a git repo ...` note and
continues.

---

## Supply context variables

Context variables fill template slots in the recipe. Supply them with `-c`:

```sh
amplihack recipe run recipe.yaml \
  -c task_description="Your task here" \
  -c repo_path=/path/to/repo \
  -c custom_var=my_value
```

Or set them as environment variables to avoid typing them every run:

```sh
export AMPLIHACK_TASK_DESCRIPTION="Add OpenAPI docs to all public endpoints"
export AMPLIHACK_REPO_PATH=/home/user/src/myapp

amplihack recipe run ~/.amplihack/.claude/recipes/default-workflow.yaml
```

| Context variable   | Environment variable override      |
|--------------------|------------------------------------|
| `task_description` | `AMPLIHACK_TASK_DESCRIPTION`       |
| `repo_path`        | `AMPLIHACK_REPO_PATH`              |
| Any variable       | `AMPLIHACK_CONTEXT_<UPPERCASE_KEY>` |

---

## Control step timeouts

Bundled recipes under `amplifier-bundle/recipes/` **do not** set per-step
timeouts on agent steps. Agent reasoning is highly variable and aborting
mid-thought corrupts orchestrator state, so agent steps run to completion
by default ([issue #439](https://github.com/rysweet/amplihack-rs/issues/439)).
A small number of bash steps that call external network services (e.g.,
`gh api`, `git fetch`) carry a generous `timeout_seconds: 1800` to guard
against stuck sockets — those are deliberate availability fences, not work
bounds.

If you need a hard ceiling across an entire run (for example, a CI job),
apply one with `--step-timeout`:

```sh
# Apply a 30-minute ceiling to every step in the recipe
amplihack recipe run ~/.amplihack/.claude/recipes/default-workflow.yaml \
  -c task_description="Large codebase migration" \
  --step-timeout 1800
```

To explicitly disable step timeouts for every step (this is already the
default for agent steps; the flag also clears the 1800s availability floor
on network-bash steps):

```sh
amplihack recipe run ~/.amplihack/.claude/recipes/default-workflow.yaml \
  -c task_description="Complex architecture redesign" \
  --step-timeout 0
```

> **Note:** `--step-timeout 0` removes the 1800s availability floor on
> network-bash steps as well. A stuck `gh api` or `git fetch` can then hang
> the run indefinitely. Only use it interactively where you can Ctrl-C a
> stuck run; never in CI.

Omit `--step-timeout` to use the recipe as authored — agent steps run
without a per-step timeout, and only the network-bash steps that carry
`timeout_seconds: 1800` will time out (and only on a stuck connection).

The flag sets `AMPLIHACK_STEP_TIMEOUT` in the child process environment.
See [AMPLIHACK_STEP_TIMEOUT](../reference/environment-variables.md#amplihack_step_timeout)
for details.

### Publish and PR commands are not shell-wrapped

`workflow-publish` does not wrap its GitHub CLI publish or pull-request paths in
shell `timeout` or `gtimeout`. If you need a hard ceiling for publish or PR
creation, use `--step-timeout` at recipe-runner level instead of editing inline
shell commands.

The publish recipe still fails explicitly when `gh` fails. It captures command
output, checks the exit status, and reports the captured error rather than
continuing silently.

---

## Get machine-readable output

Use `--format json` to process results with `jq` or in CI:

```sh
amplihack recipe run ~/.amplihack/.claude/recipes/verification.yaml \
  -c task_description="Bump version to 2.1.0" \
  --format json
```

Progress continues to appear on stderr. Stdout contains only the final JSON
result, so this is safe:

```sh
amplihack recipe run ~/.amplihack/.claude/recipes/verification.yaml \
  -c task_description="Bump version to 2.1.0" \
  --format json > result.json
```

```json
{
  "recipe_name": "verification",
  "success": true,
  "run_id": "5b60657b-76ef-4f49-8a22-8b89ed75f43e",
  "log_pointer": {
    "run_id": "5b60657b-76ef-4f49-8a22-8b89ed75f43e",
    "recipe_name": "verification",
    "status": "success",
    "child_pid": 41822,
    "exit_code": 0,
    "runner_path": "/home/user/.local/bin/recipe-runner-rs"
  },
  "duration_seconds": 42.6,
  "step_results": [
    {
      "step_id": "pre-check",
      "status": "completed",
      "output": "All pre-conditions met",
      "error": "",
      "elapsed_seconds": 3.1,
      "phase": "bash"
    },
    {
      "step_id": "implementation",
      "status": "completed",
      "output": "Version bumped in Cargo.toml and pyproject.toml",
      "error": "",
      "elapsed_seconds": 34.8,
      "phase": "agent",
      "child": {
        "kind": "agent",
        "name": "builder"
      }
    }
  ]
}
```

Check the overall result in a script:

```sh
result=$(amplihack recipe run recipe.yaml -c task_description="..." --format json)
success=$(echo "$result" | jq -r '.success')
if [ "$success" != "true" ]; then
  echo "Recipe failed"
  echo "$result" | jq '.step_results[] | select(.status == "failed")'
  exit 1
fi
```

Correlate the final result with captured progress logs:

```sh
run_id=$(jq -r '.run_id' result.json)
grep "$run_id" recipe-progress.log
```

---

## Run in CI

```yaml
# .github/workflows/ai-workflow.yml
jobs:
  default-workflow:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Run default workflow
        env:
          AMPLIHACK_NONINTERACTIVE: "1"
          AMPLIHACK_TASK_DESCRIPTION: ${{ github.event.inputs.task }}
        run: |
          amplihack recipe run \
            ~/.amplihack/.claude/recipes/default-workflow.yaml \
            --format json > recipe-result.json
```

Set `AMPLIHACK_NONINTERACTIVE=1` to suppress interactive prompts. The recipe
runner exits 0 on success and 1 on failure, integrating naturally with CI
pass/fail. Live progress and heartbeat lines are written to the job log on
stderr; `recipe-result.json` contains only the final structured result.

To keep the job log terse, redirect progress stderr to a file while preserving
the final JSON result on stdout:

```sh
amplihack recipe run ~/.amplihack/.claude/recipes/default-workflow.yaml \
  -c task_description="Regenerate API docs" \
  --format json > recipe-result.json 2> recipe-progress.log
```

---

## Troubleshoot failures

### recipe-runner-rs not found

```
Error: recipe-runner-rs binary not found.
Install it: cargo install --git https://github.com/rysweet/amplihack-recipe-runner
or set RECIPE_RUNNER_RS_PATH.
```

Install the binary or point `RECIPE_RUNNER_RS_PATH` at an existing build:

```sh
export RECIPE_RUNNER_RS_PATH=/path/to/recipe-runner-rs
amplihack recipe run ...
```

### Invalid context format

```
Error: Invalid context format 'my task'. Use key=value format
(e.g., -c 'question=What is X?' -c 'var=value')
```

Context values must use `key=value` syntax. Quote values that contain spaces:

```sh
amplihack recipe run recipe.yaml -c 'task_description=Fix the login bug'
```

### Recipe validation error

```
✗ Recipe is invalid
  Error: Every step must have a non-empty 'id' field
```

Run `amplihack recipe validate <file> --verbose` for full error details, then
fix the YAML and re-run.

### A step fails mid-recipe

The output shows which step failed, how long it ran, which child process or
agent was active, and recent bounded output from that child:

```
[step 08/23 code-implementation] failed elapsed=12m14s agent=builder
error: agent exited with code 1

recent stderr from agent:builder (last 20 lines, 8192 bytes max):
  error[E0425]: cannot find value `cache_policy` in this scope
  --> src/http/cache.rs:42:18
```

`--verbose` adds child launch details and extra diagnostic context, but it is not
required for basic step progress or failure snippets:

```sh
amplihack recipe run recipe.yaml -c task_description="..." --verbose
```

---

## Related

- [amplihack recipe — Reference](../reference/recipe-command.md) — Full flag reference, output formats, and schema
- [Recipe Runner Logging Reference](../reference/recipe-runner-logging.md) — stderr progress, heartbeats, bounded snippets, and JSON fields
- [Observe Recipe Progress Tutorial](../tutorials/recipe-progress-transparency.md) — Learn the progress and failure-debugging workflow
- [Environment Variables](../reference/environment-variables.md) — `AMPLIHACK_CONTEXT_*` and `RECIPE_RUNNER_RS_PATH`
- [Run amplihack in Non-interactive Mode](./run-in-noninteractive-mode.md) — CI configuration
- [Agent Binary Routing](../concepts/agent-binary-routing.md) — How agents are located during recipe execution
