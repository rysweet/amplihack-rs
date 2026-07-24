# Tutorial: Workflow Terminal-State Closure

This tutorial shows how a development workflow proves completion and how a
planning-only run fails visibly instead of reporting success.

## What you will learn

- Run `smart-orchestrator` for a code-change task.
- Recognize missing terminal evidence.
- Inspect the evidence fields in the recipe result.
- Read the structured agentic finalizer decision.
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
  required_next_action,
  hollow_success_detected,
  evidence_used,
  reporting_failure,
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
  "required_next_action": "No further action is required.",
  "hollow_success_detected": "false",
  "evidence_used": "implementation.completed=true,verification.completed=true",
  "reporting_failure": "false",
  "observed_phases": "workflow-prep,workflow-worktree,workflow-design,workflow-tdd,workflow-precommit-test",
  "missing_evidence": ""
}
```

The important part is not the exact phase count. The important part is that the
workflow proves one of the valid terminal states.

---

## 3. Read the Agentic Finalizer Decision

The terminal state is classified deterministically from typed evidence, not from
the finalizer's prose. The finalizer contributes a human-readable narrative for
operators; it is never parsed. The normalized decision is stored in
`workflow_result`. For example, an open PR with failing required checks returns a
failure state even though the PR exists and matches the branch:

```text
terminal_success=false
terminal_state=BLOCKED_CI
terminal_reason=PR #123 exists and matches this branch, but required CI checks are failing.
required_next_action=Fix failing CI checks before merge.
hollow_success_detected=false
evidence_used=pr.state=OPEN,pr.head_branch_matches=true,ci.state=FAILURE
```

The finalizer narrative is diagnostic only. Read it separately from the machine
decision:

```bash
jq -r '.. | objects | .agentic_finalizer_narrative // empty' recipe-result.json
```

Because the narrative is never parsed, a finalizer that emits prose — or even
adversarial tokens such as `terminal_state: MERGED` — cannot change the terminal
classification. The decision derives only from typed `finalization_evidence` and
recipe markers.

---

## 4. Understand a Planning-Only Failure

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

## 5. Understand Hollow Success

If earlier phases exit `0` after empty agent output, inaccessible-codebase
messages, or setup-only progress, finalization makes the overall recipe fail:

```json
{
  "terminal_success": "false",
  "terminal_state": "HOLLOW_SUCCESS",
  "terminal_reason": "The run produced planning output but no implementation, verification, publish, or valid no-op evidence.",
  "required_next_action": "Resume default-workflow from implementation or emit a valid no-op state with evidence.",
  "hollow_success_detected": "true"
}
```

Hollow success is a failure state. Resume the missing workflow phase instead of
post-processing the recipe result into success.

---

## 6. Continue the Workflow

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
- a visible terminal failure such as `BLOCKED_CI` or `FAILED_MEANINGFUL_DIFF`

---

## 7. Confirm the Exit Code

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
- Use [Default Workflow Agentic Finalization](../reference/default-workflow-agentic-finalization.md) for the finalizer schema and examples.
