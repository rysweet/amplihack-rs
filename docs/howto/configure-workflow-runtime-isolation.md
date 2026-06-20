# Configure Workflow Runtime Isolation

Use this guide to inspect, override, and troubleshoot where workflow runtime
output is written.

Updated: 2026-06-19

## Contents

- [Use the default runtime root](#use-the-default-runtime-root)
- [Override the runtime root](#override-the-runtime-root)
- [Inspect the active runtime root](#inspect-the-active-runtime-root)
- [Clean known workflow runtime leftovers](#clean-known-workflow-runtime-leftovers)
- [Handle Artifact Guard failures](#handle-artifact-guard-failures)
- [Related documentation](#related-documentation)

## Use the default runtime root

No configuration is required for normal workflow runs.

```bash
amplihack recipe run default-workflow \
  -c task_description="Fix flaky login tests" \
  -c repo_path=.
```

The workflow creates a per-run runtime root outside the commit worktree, sets
`AMPLIHACK_RUNTIME_ROOT`, and passes that same value to child recipes, agents,
provenance logging, publish helpers, and finalization helpers. Child workflows
inherit the value unchanged.

## Override the runtime root

Set `AMPLIHACK_RUNTIME_ROOT` when CI or local policy requires runtime files in a
specific external directory.

```bash
export AMPLIHACK_RUNTIME_ROOT="/var/tmp/amplihack-runtime/$USER/login-tests"

amplihack recipe run default-workflow \
  -c task_description="Fix flaky login tests" \
  -c repo_path=.
```

Use an absolute, private path outside the repository worktree. Do not point
`AMPLIHACK_RUNTIME_ROOT` at `.claude/runtime`, `worktrees/`, `target/`, or any
other path inside the task worktree.

For shared CI machines, create the directory with owner-only permissions where
the platform supports them:

```bash
install -d -m 700 "$AMPLIHACK_RUNTIME_ROOT"
```

## Inspect the active runtime root

Recipe progress logs include the run ID. Use that value to inspect the default
runtime location.

```bash
amplihack recipe run default-workflow \
  -c task_description="Update OAuth docs" \
  -c repo_path=. \
  --format json > result.json 2> progress.log

RUN_ID="$(jq -r '.run_id' result.json)"
if [ -n "${AMPLIHACK_RUNTIME_ROOT:-}" ]; then
  echo "$AMPLIHACK_RUNTIME_ROOT"
elif [ -n "${XDG_RUNTIME_DIR:-}" ]; then
  echo "$XDG_RUNTIME_DIR/amplihack/runtime/$RUN_ID"
else
  echo "/tmp/amplihack-runtime/$USER/$RUN_ID"
fi
```

## Clean known workflow runtime leftovers

Normal workflows run preflight cleanup automatically. To diagnose a worktree
manually, source the bundled helper and run preflight from the active worktree.

```bash
cd /path/to/task-worktree

. "$AMPLIHACK_HOME/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
preflight_known_workflow_runtime_artifacts "$PWD"
```

The helper removes only:

```text
.claude/runtime
worktrees
```

Root-level `worktrees/` is reserved for workflow-owned nested scratch worktrees
inside amplihack-managed task worktrees. Do not store source files there. A
tracked `worktrees/` path is a repository design conflict and should be renamed;
cleanup implementations must fail closed rather than delete tracked source.

It preserves user-authored `.claude` files:

```bash
mkdir -p .claude
printf '{"permissions":{}}\n' > .claude/settings.json

. "$AMPLIHACK_HOME/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
preflight_known_workflow_runtime_artifacts "$PWD"

test -f .claude/settings.json
```

## Handle Artifact Guard failures

If Artifact Guard reports `.claude/runtime/` or `worktrees/` during a workflow
run, rerun the workflow or manually run the preflight helper from the active
task worktree. Those two paths are known workflow runtime leftovers.

If Artifact Guard reports any other path, fix the reported artifact directly.
Do not add broad allowlist entries for generated output.

```bash
amplihack hygiene artifact-guard --repo . --mode all
```

Examples that still require manual remediation:

```text
node_modules/
dist/plugin.js
coverage/
logs/generated.log
```

Move generated output outside the worktree or delete it if it is local scratch
state. Commit intentional fixtures only with a narrow reviewed allowlist entry.

## Related documentation

- [Workflow Runtime Isolation](../features/workflow-runtime-isolation.md)
- [Workflow Runtime Artifacts Reference](../reference/workflow-runtime-artifacts.md)
- [Artifact Guard](../artifact-guard.md)
- [Correlate Recipe Runs with Logs](correlate-recipe-runs.md)
