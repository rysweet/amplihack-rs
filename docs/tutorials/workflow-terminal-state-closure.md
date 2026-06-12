# Tutorial: Workflow Terminal-State Closure

This tutorial shows how a development workflow proves completion and how a
planning-only run fails visibly instead of reporting success.

## What you will learn

- Run `smart-orchestrator` for a code-change task.
- Recognize missing terminal evidence.
- Inspect the evidence fields in the recipe result.
- Rerun the workflow so it reaches implementation, verification, publish, or an
  explicit no-op state.

## Prerequisites

- `amplihack` is installed.
- You are in a Git checkout.
- `jq` is installed for JSON inspection.

```bash
export NODE_OPTIONS=--max-old-space-size=32768
```

---

## 1. Run a Development Task

Start with a normal code-change request:

```bash
amplihack recipe run smart-orchestrator \
  -c task_description="Add validation for malformed workflow terminal evidence" \
  -c repo_path=. \
  --format json > recipe-result.json
```

During a healthy run, `smart-orchestrator` routes to `default-workflow`, which
continues through implementation, verification, and publish/finalize phases.

---

## 2. Inspect Terminal Evidence

Read the terminal-state fields:

```bash
jq '.. | objects | select(has("terminal_state")) | {
  terminal_success,
  terminal_state,
  terminal_reason,
  observed_phases,
  missing_evidence
}' recipe-result.json
```

A completed implementation and verification path looks like this:

```json
{
  "terminal_success": "true",
  "terminal_state": "IMPLEMENTED_VERIFIED",
  "terminal_reason": "implementation and targeted tests completed",
  "observed_phases": "workflow-prep,workflow-worktree,workflow-design,workflow-tdd,workflow-precommit-test",
  "missing_evidence": ""
}
```

The important part is not the exact phase count. The important part is that the
workflow proves one of the valid terminal states.

---

## 3. Understand a Planning-Only Failure

If a development workflow stops after worktree setup or design, the command
exits nonzero. The result includes missing evidence:

```json
{
  "terminal_success": "false",
  "terminal_state": "FAILED_MISSING_TERMINAL_EVIDENCE",
  "terminal_reason": "development workflow stopped after workflow-design; implementation, verification, publish, or explicit no-op evidence is required",
  "observed_phases": "workflow-prep,workflow-worktree,workflow-design",
  "missing_evidence": "implementation_completed,verification_completed,publish_state_reached,terminal_no_op"
}
```

This is expected behavior. A design or prepared worktree is useful progress, but
it is not a completed code-change task.

---

## 4. Continue the Workflow

Resume or rerun the development workflow with the existing branch/worktree
context:

```bash
amplihack recipe run default-workflow \
  -c task_description="Add validation for malformed workflow terminal evidence" \
  -c repo_path=. \
  -c existing_branch="$(git branch --show-current)" \
  --format json > resumed-result.json
```

The rerun must reach one of these outcomes:

- `IMPLEMENTED_VERIFIED`
- `FOLLOWUP_CREATED`
- `MERGED`
- `NO_DIFF_SUCCESS`
- `CLOSED_OBSOLETE`
- `SUPERSEDED`
- `ALLOW_NO_OP`
- a visible terminal failure such as `BLOCKED_CI`

---

## 5. Confirm the Exit Code

Use the shell exit code as the source of truth:

```bash
set +e
amplihack recipe run smart-orchestrator \
  -c task_description="Add validation for malformed workflow terminal evidence" \
  -c repo_path=. \
  --format json > recipe-result.json
status=$?
set -e

echo "$status"
```

Expected values:

| Exit code | Meaning |
| --- | --- |
| `0` | Terminal success evidence is proven. |
| `1` | The recipe failed, including missing terminal evidence. |

Do not treat a JSON result with `FAILED_MISSING_TERMINAL_EVIDENCE` as success
even if earlier planning or worktree steps completed.

When `amplifier-bundle/tools/workflow_final_status.sh` is invoked directly, it
may exit `2` for invalid helper invocation, missing tooling, or malformed input
before evaluation can run. Through `amplihack recipe run`, that helper failure is
reported as recipe failure and the CLI exits nonzero, currently `1`.

---

## Next Steps

- Use [How to Diagnose Workflow Terminal-State Failures](../howto/diagnose-workflow-terminal-state.md) for recovery patterns.
- Use [Workflow Terminal-State Reference](../reference/workflow-terminal-state.md) for the field and helper API.
