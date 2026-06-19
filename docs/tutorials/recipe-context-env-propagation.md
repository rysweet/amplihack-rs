# Propagate Recipe Context to Bash Steps

Learn how recipe context variables reach bash steps as environment variables —
so a step can read `$TASK_DESCRIPTION` and `$REPO_PATH` directly, even under
`set -u` and inside nested sub-recipes.

## What you will do

1. Run a top-level recipe whose bash step reads context from the environment.
2. Prove the values arrive under `set -u`.
3. Confirm a nested sub-recipe inherits the same values.
4. Observe a skipped (invalid) key being warned about, not crashing the run.
5. Understand why dangerous names are never exported.

## Before you start

You need:

- `amplihack` installed (the Rust CLI; verify with `command -v amplihack`)
- a writable scratch directory
- `bash`

All context values feed both `{{placeholder}}` substitution and the process
environment. This tutorial uses the environment form on purpose.

## Step 1: Write a recipe that reads the environment

```bash
mkdir -p /tmp/ctx-env && cd /tmp/ctx-env

cat > top.yaml << 'EOF'
name: top
description: Reads context from the environment under set -u
version: "1.0"

context:
  task_description: ""
  repo_path: "."

steps:
  - id: read-env
    type: bash
    command: |
      set -euo pipefail
      echo "TASK_DESCRIPTION=$TASK_DESCRIPTION"
      echo "REPO_PATH=$REPO_PATH"
EOF
```

The `set -u` line is the important part: before context environment export
existed, this step aborted with `TASK_DESCRIPTION: unbound variable`.

## Step 2: Run it and see the values

```bash
amplihack recipe run /tmp/ctx-env/top.yaml \
  -c task_description="Add validation for empty display names" \
  -c repo_path=.
```

Expected output:

```
TASK_DESCRIPTION=Add validation for empty display names
REPO_PATH=.
```

The keys were lowercase in the context (`task_description`, `repo_path`) and
arrived uppercased in the environment (`TASK_DESCRIPTION`, `REPO_PATH`). The
values are passed through unchanged.

## Step 3: Confirm a nested sub-recipe inherits the values

Add a sub-recipe and a grandchild shell to prove inheritance flows all the way
down.

```bash
cat > parent.yaml << 'EOF'
name: parent
context:
  task_description: ""
  repo_path: "."
steps:
  - id: call-child
    type: recipe
    recipe: child
EOF

cat > child.yaml << 'EOF'
name: child
steps:
  - id: read-inherited
    type: bash
    command: |
      set -euo pipefail
      sh -c 'set -u; echo "child sees: $TASK_DESCRIPTION at $REPO_PATH"'
EOF

amplihack recipe run /tmp/ctx-env/parent.yaml \
  -c task_description="Ship the fix" \
  -c repo_path=/work/repo
```

Expected output:

```
child sees: Ship the fix at /work/repo
```

The CLI exports the context once, onto the `recipe-runner-rs` subprocess. The
sub-recipe step — and even the `sh -c` grandchild it spawns — then inherit those
values through normal OS process-environment inheritance. Because
`recipe-runner-rs` is a separate binary, this `sh -c` line is the **canary** for
the whole chain: it proves the export survives CLI → `recipe-runner-rs` → bash
step → grandchild shell. If it prints the value (instead of aborting with
`unbound variable` under `set -u`), end-to-end propagation works — the behavior
multi-workstream campaigns depend on at `step-03-create-issue`.

## Step 4: Watch an invalid key get skipped, not crash

Context keys that cannot become valid shell identifiers are skipped with a
name-only warning. The run still succeeds.

The skip notice is a `WARN`-level log from the `amplihack` process. The CLI
shows only `ERROR` by default, so set `RUST_LOG=warn` to see it:

```bash
RUST_LOG=warn amplihack recipe run /tmp/ctx-env/top.yaml \
  -c task_description="ok" \
  -c "issue title=has spaces" \
  -c repo_path=. 2>&1 | grep -E 'skipped|TASK_DESCRIPTION|REPO_PATH'
```

Expected output includes:

```
WARN recipe context key skipped for env export name=ISSUE TITLE reason=invalid_identifier
TASK_DESCRIPTION=ok
REPO_PATH=.
```

`issue title` uppercases to `ISSUE TITLE`, which contains a space and is not a
valid identifier, so it is dropped. Note the warning logs the **name only**,
never the value. `TASK_DESCRIPTION` and `REPO_PATH` still export normally.

## Step 5: Understand the security guardrails

Some names are never exported from context, because setting them would change
how the shell or dynamic loader behaves before your step runs. Try to inject one
and confirm it is ignored:

```bash
cat > guard.yaml << 'EOF'
name: guard
steps:
  - id: probe
    type: bash
    command: |
      set -euo pipefail
      echo "LD_PRELOAD=[${LD_PRELOAD:-unset}]"
      echo "PATH unchanged? $(command -v echo >/dev/null && echo yes)"
EOF

RUST_LOG=warn amplihack recipe run /tmp/ctx-env/guard.yaml \
  -c ld_preload=/tmp/evil.so \
  -c path=/evil 2>&1 | grep -E 'LD_PRELOAD|PATH unchanged|skipped'
```

Expected output:

```
WARN recipe context key skipped for env export name=LD_PRELOAD reason=reserved_name
WARN recipe context key skipped for env export name=PATH reason=reserved_name
LD_PRELOAD=[unset]
PATH unchanged? yes
```

`LD_PRELOAD` and `PATH` are on the reserved-name denylist, so the context values
never reach the environment. The same protection covers `BASH_ENV`, `PS4`,
`IFS`, the `DYLD_*`/`LD_*` loader variables, interpreter options like
`PYTHONPATH` and `NODE_OPTIONS`, and any `AMPLIHACK_`-prefixed name.

## Clean up

```bash
rm -rf /tmp/ctx-env
```

## What you learned

- Recipe context is exported to bash steps as uppercased environment variables,
  so `$TASK_DESCRIPTION` and `$REPO_PATH` work under `set -u`.
- The export is inherited by nested sub-recipes and their grandchild shells.
- Invalid keys are skipped with a name-only warning instead of failing the run.
- Dangerous names are never exported, and builder-managed/correlation variables
  always take precedence over context.

## Related

- [Recipe Context Environment Export — Reference](../reference/recipe-context-environment.md) — Full contract, transform rules, denylist, and API
- [Recipe Executor Environment — Reference](../reference/recipe-executor-environment.md) — Subprocess and step-level environment injection
- [Troubleshoot Recipe Execution](../howto/troubleshoot-recipe-execution.md) — Fixing `TASK_DESCRIPTION: unbound variable`
- [Trace a Recipe Run Across Terminal and JSON Logs](./recipe-run-correlation.md) — Correlating runs by `AMPLIHACK_RECIPE_RUN_ID`
