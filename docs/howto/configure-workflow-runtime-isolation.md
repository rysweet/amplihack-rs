# Configure Workflow Runtime Isolation

Use this guide to inspect, override, and troubleshoot where workflow runtime
output is written.

Updated: 2026-11-12

## Contents

- [Use the default runtime root](#use-the-default-runtime-root)
- [Override the runtime root](#override-the-runtime-root)
- [Inspect the active runtime root](#inspect-the-active-runtime-root)
- [How a step gets its TMPDIR](#how-a-step-gets-its-tmpdir)
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
failed, unless a running process still uses it. Before removing it, the runner
gives the owner back read, write and search access on every folder in it that
the run owns, so a test fixture left at mode `000` or `0500` does not block the
removal. Entries the run does not own, or that sit on another filesystem, are
left unchanged and named in a warning; the warning never fails the run. See
[Locked folders inside the runtime folder](clean-up-after-recipe-runs.md#locked-folders-inside-the-runtime-folder).
The run report `run.json`
stays in the run folder. A runtime root left by a killed run, or kept because
a process still used it, is removed by a later sweep or by `amplihack cleanup`
(category `run-tmp`) once no process uses it. See
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
will not use a work root that is `/`, your home folder or one of its
ancestors, a system folder, the system temporary folder, or a git checkout
root, nor an existing folder that you do not own or that group or others can
write to. In those cases the run stops with an error naming the work root
before any step runs.

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

## How a step gets its TMPDIR

The runner starts the step runner with `TMPDIR`, `TMP` and `TEMP` set to the
`tmp/` folder of its own run, whatever value the shell that started the run
had. It sets these three variables last, after the inherited environment and
the recipe context (`-c` values), so neither can replace them. This matters
when `amplihack recipe run` is started from inside another run, for example by
an agent: the inherited `TMPDIR` belongs to the outer run, and may already have
been removed.

This guarantee covers only the environment the runner hands to the step runner.
A step whose own `env:` block in the recipe YAML sets `TMPDIR`, and the reduced
environment the step runner builds for an agent step, are applied later by the
step runner and are not covered here.

Before each attempt of a step, including retries, the runner creates
`<runtime>/tmp` and `<runtime>/artifacts` again if they are missing, with mode
`0700` and at the same path as before. A step therefore never starts with a
runner-provided `TMPDIR` that does not exist, even if something deleted the
folder during the run.

You can see this by starting a run with a `TMPDIR` that does not exist:

```bash
export TMPDIR=/tmp/amplihack-gone TMP=/tmp/amplihack-gone TEMP=/tmp/amplihack-gone
amplihack recipe run default-workflow \
  -c task_description="Fix flaky login tests" \
  -c repo_path=.
```

The step runner gets `TMPDIR=<runtime>/tmp`, which exists, and
`/tmp/amplihack-gone` is never created.

The runner refuses to launch the step, and the run fails, in two cases. If it
cannot create the folder, it prints:

```text
amplihack: cannot create this run's TMPDIR "~/.amplihack/work/r/<id>/tmp": <reason>; refusing to launch the step
```

If `tmp` or `artifacts` already exists as a symbolic link or as a file, the
runner does not follow or replace it, and prints:

```text
amplihack: this run's TMPDIR "~/.amplihack/work/r/<id>/tmp" is a symbolic link or not a directory; refusing to launch the step
```

The same message names `artifacts` when that is the folder at fault. The real
messages show the full absolute path of the runtime root, in double quotes,
where these examples show `~/.amplihack/work/r/<id>`. The runtime root is
`<work root>/runs/<run id>/runtime`, or `<work root>/r/<short id>` when that
path would be too long.

A dry run (`recipe run --dry-run`) has no run folder. It creates a private
`amplihack-dry-*` folder inside the inherited `TMPDIR`, so that `TMPDIR` must
be an absolute path to an existing folder. If it is not, the dry run stops
before any step runs and quotes the path:

```text
amplihack: inherited TMPDIR "/tmp/amplihack-gone" does not exist (or is not a directory); refusing to start the run — set TMPDIR to an existing directory
amplihack: inherited TMPDIR "build/tmp" is not an absolute directory; refusing to start the run — set TMPDIR to an existing directory
```

The runner never creates a missing inherited `TMPDIR`, because that path may
be the removed folder of another run.

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
