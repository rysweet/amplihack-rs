# Configure Workflow Runtime Isolation

Use this guide to inspect, override, and troubleshoot where workflow runtime
output is written.

Updated: 2026-11-01

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

The runner creates a per-run runtime root outside the commit worktree, at
`<work root>/runs/<run id>/runtime`, where the work root defaults to
`~/.amplihack/work`. It sets `AMPLIHACK_RUNTIME_ROOT` and
`AMPLIHACK_WORKFLOW_RUNTIME_DIR` to that folder and passes the same value to
child recipes, agents, provenance logging, publish helpers, and finalization
helpers. `TMPDIR`, `TMP` and `TEMP` point at its `tmp/` subfolder.

The runner removes the runtime root when the run ends, whether it succeeded or
failed. The run report `run.json` stays in the run folder. A runtime root left
by a killed run is removed by `amplihack cleanup` (category `run-tmp`). See
[Clean Up After Recipe Runs](clean-up-after-recipe-runs.md) for the full
layout and the work root settings.

## Override the runtime root

Set `AMPLIHACK_WORK_ROOT` when CI or local policy requires runtime files on a
specific disk or in a specific directory. The runner puts each run's runtime
root under `<work root>/runs/<run id>/runtime`.

```bash
install -d -m 700 /var/tmp/amplihack-work
export AMPLIHACK_WORK_ROOT=/var/tmp/amplihack-work

amplihack recipe run default-workflow \
  -c task_description="Fix flaky login tests" \
  -c repo_path=.
```

Use an absolute, private path outside any repository checkout. The runner
refuses a work root that is `/`, your home folder or one of its ancestors, a
system folder, the system temporary folder, or a git checkout root. It also
refuses an existing folder that you do not own or that group or others can
write to.

The runner always sets `AMPLIHACK_RUNTIME_ROOT` for its steps, so setting that
variable before a run has no effect on where the runtime root goes. Setting
`AMPLIHACK_WORK_ROOT` also moves worktrees into `<work root>/worktrees/`; see
[Worktree location](clean-up-after-recipe-runs.md#worktree-location).

## Inspect the active runtime root

The JSON result reports the attempt id as `run_id`. The run folder is named
after the run id, which differs from the attempt id, so find the folder by
searching the run reports for the attempt id. While the run is going, its
runtime root is under the run folder:

```bash
amplihack recipe run default-workflow \
  -c task_description="Update OAuth docs" \
  -c repo_path=. \
  --format json > result.json 2> progress.log

ATTEMPT_ID="$(jq -r '.run_id' result.json)"
WORK_ROOT="${AMPLIHACK_WORK_ROOT:-$HOME/.amplihack/work}"
RUN_DIR="$(grep -l "\"$ATTEMPT_ID\"" "$WORK_ROOT"/runs/*/run.json | xargs -r dirname)"
ls "$RUN_DIR"
# run.json  owned-paths
```

The `runtime/` subfolder exists only while the run is going. If the default
path to `runtime/tmp` would be longer than 80 bytes, the runtime root is
`<work root>/r/<first 12 hex digits of the run id>` instead.

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
- [Clean Up After Recipe Runs](clean-up-after-recipe-runs.md)
