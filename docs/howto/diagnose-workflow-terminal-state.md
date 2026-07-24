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
| `required_next_action` | The action the finalizer expects before the workflow can close. |
| `hollow_success_detected` | Whether the run appeared successful but lacked real completion evidence. |
| `evidence_used` | Typed evidence keys used by the deterministic classifier. |
| `reporting_failure` | Whether a reporting step failed after successful implementation (produces `FAILED_REPORTING`, durable evidence preserved). |
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
- `terminal_success=true` with `finalizer_confidence=medium` or `low`

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

## Distinguish Implementation Failure from Reporting Failure

Finalization classifies two failure families separately so free-form
explanatory output — or a transient failure in a reporting step — can never
erase proof that the implementation succeeded.

`FAILED_IMPLEMENTATION` means durable implementation or verification evidence is
absent or failed while meaningful work remains:

```text
terminal_success=false
terminal_state=FAILED_IMPLEMENTATION
implementation_completed=false
verification_completed=false
required_next_action=Resume default-workflow from implementation and verification.
```

`FAILED_REPORTING` means the implementation succeeded but a reporting/finalization
step failed. Durable evidence is preserved:

```text
terminal_success=false
terminal_state=FAILED_REPORTING
reporting_failure=true
implementation_completed=true
verification_completed=true
pr_url=https://github.com/rysweet/amplihack-rs/pull/123
pr_number=123
required_next_action=Re-run the reporting step; the implementation is intact.
```

Retry the reporting step for `FAILED_REPORTING`; do not re-run the
implementation. Resume from implementation only for `FAILED_IMPLEMENTATION`.

The finalizer narrative is never parsed, so its text cannot cause either state.
Read it as diagnostics only:

```bash
jq -r '.. | objects | .agentic_finalizer_narrative // empty' recipe-result.json
```

Do not patch CI scripts to ignore these states. They mean finalization could not
prove closure.

---

## Fix `HOLLOW_SUCCESS`

`HOLLOW_SUCCESS` means a workflow path looked complete at the process level but
did not produce meaningful terminal evidence. This often follows setup-only,
design-only, inaccessible-codebase, or empty/generic agent output.

Inspect the evidence:

```bash
jq '.. | objects | select(.terminal_state? == "HOLLOW_SUCCESS") | {
  terminal_reason,
  required_next_action,
  observed_phases,
  implementation_completed,
  verification_completed,
  publish_state_reached,
  evidence_used
}' result.json
```

Recovery is one of:

| Missing evidence | Recovery |
| --- | --- |
| Implementation | Resume `default-workflow` from implementation on the existing branch/worktree. |
| Verification | Run the selected validation, tests, or pre-commit phase and rerun finalization. |
| Publish or follow-up | Publish the meaningful diff or create a durable follow-up PR/issue. |
| Valid no-op | Emit `NO_DIFF_SUCCESS`, `MERGED`, `CLOSED_OBSOLETE`, `SUPERSEDED`, or `ALLOW_NO_OP` with deterministic proof. |

---

## Fix `BLOCKED_CI`

`BLOCKED_CI` is a terminal failure state, not an incomplete finalizer run. It
means the finalizer found a matching PR or publish target, but required checks
are failing, pending beyond policy, or unavailable when required.

For GitHub PRs:

```bash
gh pr view "$PR_NUMBER" \
  --json number,state,headRefName,baseRefName,headRefOid,statusCheckRollup
```

Then fix or rerun the failing checks. Do not convert `BLOCKED_CI` into success
unless the CI evidence changes and finalization is rerun.

---

## Fix `FAILED_MEANINGFUL_DIFF`

`FAILED_MEANINGFUL_DIFF` means local branch changes still exist, but
finalization could not prove that the diff is represented by a valid PR,
follow-up, merge, no-op, or completed implementation-plus-verification path.

Inspect the branch against the intended base:

```bash
git status --short
git diff --stat origin/main...HEAD
git rev-list --count origin/main..HEAD
```

Recovery is one of:

| Situation | Recovery |
| --- | --- |
| Diff is intended work | Publish or update the workflow-owned PR, then rerun finalization. |
| Diff should be handled later | Create a durable follow-up PR or issue and return `FOLLOWUP_CREATED` or `SUPERSEDED` with its identifier. |
| Diff is already upstream or obsolete | Prove `NO_DIFF_SUCCESS` or `CLOSED_OBSOLETE` with local no-diff/obsolete evidence. |
| Diff is accidental | Remove or commit the intended changes before rerunning finalization. |

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
- [Default Workflow Agentic Finalization](../reference/default-workflow-agentic-finalization.md)
- [RecipeResult Reference](../reference/recipe-result.md)
- [Run a Recipe End-to-End](run-a-recipe.md)
- [How to Troubleshoot Recipe Execution Failures](troubleshoot-recipe-execution.md)
