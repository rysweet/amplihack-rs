---
title: Configure workflow runtime isolation
description: Select and inspect per-run runtime storage outside commit worktrees.
doc_type: howto
last_updated: 2026-11-12
---

# Configure Workflow Runtime Isolation

Runtime cleanup requires exclusive creation evidence and preserves directories
when it cannot establish deletion authority. Linux cleanup requires filesystem
identity and descriptor-relative operations; unsupported platforms or
filesystems preserve directories or refuse ownership initialization.
See the [cleanup contract](../reference/owned-run-directory-cleanup.md).

Use this guide to inspect, override, and troubleshoot where workflow runtime
output is written.

## Contents

- [Use the default runtime root](#use-the-default-runtime-root)
- [Override the runtime root](#override-the-runtime-root)
- [Inspect the active runtime root](#inspect-the-active-runtime-root)
- [Handle a preserved runtime directory](#handle-a-preserved-runtime-directory)
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

Without a suitable external temporary root, the runner creates its per-run
runtime directory outside the commit worktree, at
`<work root>/runs/<run id>/runtime`, where the work root defaults to
`~/.amplihack/work`. It sets `AMPLIHACK_RUNTIME_ROOT` and
`AMPLIHACK_WORKFLOW_RUNTIME_DIR` to that folder and passes the same value to
steps, agents, provenance logging, publish helpers, and finalization
helpers. `TMPDIR`, `TMP` and `TEMP` point at its `tmp/` subfolder.

To select an external parent, use `AMPLIHACK_RUN_TMP_ROOT` or the caller's
suitable `TMPDIR`; see [temporary-root selection](clean-up-after-recipe-runs.md#where-a-runs-temp-folder-goes).
Nested recipe runs create their own directories rather than reuse their
parent's disposable directory.

The runner requests guarded removal of the runtime directory when the run
ends, whether it succeeded or failed. Creation ownership, protected-root,
direct-parent, and directory identity checks must pass; otherwise the
directory survives with a diagnostic. The run report `run.json` stays in
the run folder. `amplihack cleanup` (category `run-tmp`) uses the same
authority for directories left by killed runs. Legacy unmarked directories
survive. See the [cleanup contract](../reference/owned-run-directory-cleanup.md)
for refusal conditions and
[Clean Up After Recipe Runs](clean-up-after-recipe-runs.md) for the full
layout and the work root settings.

## Override the runtime root

Set `AMPLIHACK_WORK_ROOT` when CI or local policy requires runtime files on a
specific disk or in a specific directory. This moves durable run reports and
recovery storage. Disposable runtime storage is selected separately:

| Selection | Runtime directory |
| --- | --- |
| Usable, nonempty `AMPLIHACK_RUN_TMP_ROOT` | `<selected root>/ahr-<first 12 hex digits of run id>` |
| Explicit root unset or empty, caller's `TMPDIR` suitable | `<selected root>/ahr-<first 12 hex digits of run id>` |
| No usable external root | `<work root>/runs/<run id>/runtime` |
| No usable external root; default `runtime/tmp` path exceeds 80 bytes | `<work root>/r/<first 12 hex digits of run id>` |

An unusable explicit root falls back to the default layout, without trying
the caller's `TMPDIR`. Setting `AMPLIHACK_WORK_ROOT` does not override an
external temporary root. To keep runtime files under the work root, unset
`AMPLIHACK_RUN_TMP_ROOT` and `TMPDIR` for the invocation. See
[root selection and refusal rules](clean-up-after-recipe-runs.md#where-a-runs-temp-folder-goes).

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
write to. A durable run stops with an error when its work root cannot be used;
it cannot replace required recovery storage with temporary storage.

The runner always sets `AMPLIHACK_RUNTIME_ROOT` for its steps, so setting that
variable before a run has no effect on where the runtime root goes. Setting
`AMPLIHACK_WORK_ROOT` also moves worktrees into `<work root>/worktrees/`; see
[Worktree location](clean-up-after-recipe-runs.md#worktree-location).

## Inspect the active runtime root

The JSON result reports the attempt id as `run_id`. The run folder is named
after the run id, which differs from the attempt id, so find the folder by
searching the run reports for the attempt id. While the run is going, its
runtime root can use any layout in the table above. After obtaining
`result.json` from a run with `--format json`, inspect its report:

```bash
ATTEMPT_ID="$(jq -r '.run_id' result.json)"
RUN_WORK_ROOT="${AMPLIHACK_WORK_ROOT:-$HOME/.amplihack/work}"
for RUN_REPORT in "$RUN_WORK_ROOT"/runs/*/run.json; do
  [ -f "$RUN_REPORT" ] || continue
  jq -e --arg attempt "$ATTEMPT_ID" \
    'any(.attempts[]?; .correlation_id == $attempt)' "$RUN_REPORT" >/dev/null || continue
  jq '{run_id, runtime_dir, runtime_root, outcome, ended_at}' "$RUN_REPORT"
done
# Outputs the matching report's run id, external runtime path and parent
# (or null), outcome, and end time.
```

Use `run.json.runtime_dir` when present; it records the external runtime
directory. When absent, use `<run dir>/runtime` if its `tmp/` path is at most
80 bytes, otherwise `<work root>/r/<first 12 hex digits of run id>`. Within
a running step, inspect `AMPLIHACK_WORKFLOW_RUNTIME_DIR` directly.

The report records a location, not deletion permission or proof that it
still exists. Successful guarded cleanup removes runtime storage. A killed
run, failed ownership check, or unsupported filesystem operation can leave
it present after completion, in any layout. `run.json` and durable
`recovery/` data remain in the run folder. Inspect a retained directory and
its diagnostic before considering manual removal; do not add ownership
markers to make it eligible.

## Handle a preserved runtime directory

Preview stale runtime cleanup before applying it:

```bash
amplihack cleanup --only run-tmp --format json
# JSON entries describe eligible or skipped directories and their reasons.
```

After inspecting the reported paths and reasons, apply the same category:

```bash
amplihack cleanup --only run-tmp --apply --format json
# Eligible, verified owned directories are removed; refused entries are skipped.
```

An unmarked directory, a copied marker, a moved run directory, or a path with
the wrong immediate parent remains intact. Cleanup also rejects `TMPDIR`,
`std::env::temp_dir()`, `/tmp`, and `$HOME` themselves, even with markers.
Owned children beneath a selected temporary root can still be removed.
Creation-time and current protections both apply, including resolved aliases.

Read the preservation diagnostic before changing configuration. On unsupported
platforms or filesystems, retain the directory or inspect it for manual
retirement after confirming that no process uses it. Never fabricate ownership
records or use a recursive deletion of the configured parent as a workaround.
If a failed cleanup leaves a claimed location, use that reported location for
inspection; recreating the original name does not transfer ownership.

`AMPLIHACK_CLEANUP_ON_EXIT=0` disables the sweep of earlier runs. The
completing run still requests guarded disposal of its own runtime directory.
See the [authorization reference](../reference/owned-run-directory-cleanup.md)
for all refusal conditions.

## Clean known workflow runtime leftovers

Normal workflows run preflight cleanup automatically. To diagnose a worktree
manually, source the bundled helper and run preflight from the active worktree.

```bash
cd "$(git rev-parse --show-toplevel)"

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
