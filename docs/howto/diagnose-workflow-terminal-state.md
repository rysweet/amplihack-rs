# How to Diagnose Workflow Terminal-State Failures

Use this guide when `default-workflow` or `smart-orchestrator` exits nonzero
with `FAILED_MISSING_TERMINAL_EVIDENCE`, `FAILED_INVALID_EVIDENCE`, or another
terminal-state error.

## Before you start

- Run from a Git checkout for development workflows.
- Use the same `repo_path`, `branch_name`, `pr_number`, and `pr_url` values that
  the failing workflow used.
- Preserve Node memory settings when checks include Node tooling:

```bash
export NODE_OPTIONS=--max-old-space-size=32768
```

---

## Read the Failure

Run with JSON output so the failing step and terminal evidence are visible:

```bash
amplihack recipe run smart-orchestrator \
  -c task_description="Fix cache invalidation bug" \
  -c repo_path=. \
  --format json > result.json
```

If the shell exits nonzero, inspect the terminal-state failure:

```bash
jq '(.failure_context // empty),
    (.step_results[]? | select((.step_id // "") | test("terminal-state|workflow-finalize|default-workflow")))' result.json
```

Look for these fields:

| Field | What to check |
| --- | --- |
| `terminal_state` | Stable failure state such as `FAILED_MISSING_TERMINAL_EVIDENCE`. |
| `terminal_reason` | Human-readable explanation and next action. |
| `observed_phases` | Last workflow phases that produced evidence. |
| `missing_evidence` | Required proof that was absent. |

---

## Fix `FAILED_MISSING_TERMINAL_EVIDENCE`

This means the workflow was classified as a code-change workflow but stopped
before it proved completion.

Common causes:

| Observed phases | Meaning | Fix |
| --- | --- | --- |
| `workflow-prep` only | Requirements or issue setup ran, then execution stopped. | Re-run or resume so worktree, design, implementation, verification, and publish phases execute. |
| `workflow-prep,workflow-worktree` | Branch/worktree setup completed, but no implementation evidence exists. | Continue into `workflow-design` and `workflow-tdd`; do not treat worktree prep as completion. |
| `workflow-prep,workflow-worktree,workflow-design` | Design exists, but code and verification did not run. | Continue into implementation and verification, or emit an explicit no-op state if no changes are required. |
| `workflow-tdd` without verification | Implementation ran but checks did not prove success. | Run pre-commit, targeted tests, or the workflow verification phase. |

Example recovery:

```bash
amplihack recipe run default-workflow \
  -c task_description="Fix cache invalidation bug" \
  -c repo_path=. \
  -c branch_name=feat/cache-invalidation \
  -c existing_branch=feat/cache-invalidation
```

The rerun must either continue into implementation/verification/publish or end
with an explicit terminal no-op/failure state.

---

## Fix `FAILED_INVALID_EVIDENCE`

This means the workflow produced terminal markers, but they were not trustworthy.

Check for:

- unknown `terminal_state`
- empty `terminal_reason`
- boolean markers with non-boolean values
- `terminal_no_op=true` without a no-op state
- `implementation_completed=true` and `terminal_failure=true` in the same result
- PR URL or PR number that does not match the repository

Correct the producing recipe step so it emits one coherent state:

```text
terminal_success=true
terminal_state=IMPLEMENTED_VERIFIED
terminal_reason=implementation and verification completed
implementation_completed=true
verification_completed=true
terminal_failure=false
```

or:

```text
terminal_success=false
terminal_state=BLOCKED_CI
terminal_reason=required status check unit-tests failed
terminal_failure=true
```

---

## Use an Explicit No-Op Correctly

Use a terminal no-op only when the task is legitimately complete without further
code-change work.

Valid examples:

- requested change is already merged
- branch is clean with no meaningful diff against base
- issue is explicitly superseded by another workflow-owned PR
- documentation review, audit, or orchestration task with no requested file edits
  was classified with `allow_no_op=true`

Example:

```bash
amplihack recipe run default-workflow \
  -c task_description="Verify the already-merged workflow fix" \
  -c repo_path=. \
  -c pr_number=579
```

Expected terminal evidence:

```text
terminal_success=true
terminal_state=MERGED
terminal_reason=PR #579 is merged
terminal_no_op=true
```

Do not use `allow_no_op=true` to bypass failed implementation or verification.
The gate still requires a valid no-op state and reason.

---

## Verify Routing Propagation

When the failure happens under `smart-orchestrator`, confirm the routed
`default-workflow` failure is not masked:

```bash
set +e
amplihack recipe run smart-orchestrator \
  -c task_description="Add validation for user input" \
  -c repo_path=. \
  --format json > result.json
status=$?
set -e

echo "$status"
jq '.status, .failure_context.step_id, .failure_context.status' result.json
```

Expected behavior for missing terminal evidence:

```text
1
"FAILURE"
"workflow-terminal-state"
"failed"
```

If the command exits `0` while the routed development workflow reports missing
terminal evidence, the routing layer is incorrectly masking failure.

---

## Run the Terminal-State Evaluator Directly

The shell helper can be run from a checkout to inspect normalized evidence.

```bash
WORKFLOW_CLASSIFICATION=Development \
RECIPE_NAME=default-workflow \
REPO_PATH=. \
BRANCH_NAME="$(git branch --show-current)" \
OBSERVED_PHASES="workflow-prep,workflow-worktree,workflow-design" \
amplifier-bundle/tools/workflow_final_status.sh
```

Expected result:

```text
terminal_success=false
terminal_state=FAILED_MISSING_TERMINAL_EVIDENCE
missing_evidence=implementation_completed,verification_completed,publish_state_reached,terminal_no_op
```

Add implementation and verification evidence to confirm a valid success path:

```bash
WORKFLOW_CLASSIFICATION=Development \
RECIPE_NAME=default-workflow \
REPO_PATH=. \
BRANCH_NAME="$(git branch --show-current)" \
IMPLEMENTATION_COMPLETED=true \
VERIFICATION_COMPLETED=true \
TERMINAL_STATE=IMPLEMENTED_VERIFIED \
TERMINAL_REASON="implementation and targeted tests completed" \
amplifier-bundle/tools/workflow_final_status.sh
```

Expected result:

```text
terminal_success=true
terminal_state=IMPLEMENTED_VERIFIED
```

---

## CI Usage

In CI, run the same workflow command and let the process exit code gate the job:

```bash
export NODE_OPTIONS=--max-old-space-size=32768

amplihack recipe run smart-orchestrator \
  -c task_description="$TASK_DESCRIPTION" \
  -c repo_path="$GITHUB_WORKSPACE" \
  --format json > recipe-result.json
```

Do not post-process `FAILED_MISSING_TERMINAL_EVIDENCE` into success. That state
means the code-change task did not reach implementation/verification/publish or
a valid terminal no-op.

---

## See Also

- [Workflow Terminal-State Reference](../reference/workflow-terminal-state.md)
- [RecipeResult Reference](../reference/recipe-result.md)
- [Run a Recipe End-to-End](run-a-recipe.md)
- [How to Troubleshoot Recipe Execution Failures](troubleshoot-recipe-execution.md)
