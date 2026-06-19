# Workflow Terminal-State Reference

> [Home](../index.md) > Reference > Workflow Terminal State

This reference defines the target contract for development workflow terminal
closure. Development workflows fail closed unless they reach a proven terminal
state. `default-workflow`, routed `smart-orchestrator` development workstreams,
and their publish/finalize sub-recipes must use the same terminal-state gate so
planning, analysis, design, or worktree setup cannot be reported as successful
code-change completion.

## Contents

- [What the Gate Protects](#what-the-gate-protects)
- [Valid Success Evidence](#valid-success-evidence)
- [Default Workflow Agentic Finalization](#default-workflow-agentic-finalization)
- [Evidence Precedence](#evidence-precedence)
- [Evidence Marker API](#evidence-marker-api)
- [Recipe Output Contract](#recipe-output-contract)
- [Shell Helper API](#shell-helper-api)
- [Configuration](#configuration)
- [Failure Semantics](#failure-semantics)
- [Examples](#examples)
- [Security Invariants](#security-invariants)

---

## What the Gate Protects

The terminal-state gate applies to workflow classifications that can modify code:

- `Development`
- `Default`
- `Feature`
- `Bugfix`
- `Refactor`
- routed `smart-orchestrator` workstreams that invoke `default-workflow`

The gate does not turn investigation, Q&A, or operations workflows into
development workflows. It only enforces the completion contract once a task has
been routed to code-change semantics.

For protected workflows, these states are never enough for success by
themselves:

- requirements clarification
- analysis
- architecture or design output
- issue creation
- worktree or branch preparation
- skipped implementation
- skipped verification
- empty agent output
- a shell process exiting `0` without terminal evidence

---

## Valid Success Evidence

A protected workflow exits `0` only when one of these evidence groups is present.

| Evidence group | Required proof | Typical producer |
| --- | --- | --- |
| Implementation and verification complete | `implementation_completed=true` and `verification_completed=true` with non-empty reason text or structured detail | `workflow-tdd`, `workflow-precommit-test`, review/feedback phases |
| Publish or change-request state reached | `publish_state_reached=true` with a known publish state such as `FOLLOWUP_CREATED`, `SUPERSEDED`, `MERGED`, `NO_DIFF_SUCCESS`, or `CLOSED_OBSOLETE` | `workflow-publish`, `workflow-finalize` |
| Explicit terminal no-op | `terminal_no_op=true`, a known no-op state, and a non-empty reason | no-diff, merged, obsolete, superseded, or `allow_no_op` paths |

`terminal_failure=true` is valid terminal evidence, but it is never success
evidence. It proves the workflow reached a known failing terminal state and must
exit nonzero.

Unknown, empty, malformed, or contradictory evidence is failure evidence.

---

## Default Workflow Agentic Finalization

`default-workflow` finalization uses the terminal-state gate as its deterministic
validator, not as a brittle text parser. Deterministic shell/JSON steps collect
Git, PR, CI, implementation, verification, publish, and observed-phase evidence.
A structured agentic finalizer then classifies one terminal state from that
evidence and explains the required next action. The deterministic gate validates
the finalizer JSON, persists normalized fields, and chooses the recipe exit
status.

The finalizer may exercise judgment when evidence is nuanced, such as
distinguishing `CLOSED_OBSOLETE` from a closed-unmerged PR with remaining diff,
identifying stale PR metadata, or detecting hollow success after empty agent
output. It cannot create success from unsupported prose. Missing, malformed,
contradictory, non-JSON finalizer output fails closed as
`FAILED_FINALIZER_OUTPUT`. Medium- or low-confidence output cannot prove
terminal success and must resolve to a non-success state.

See [Default Workflow Agentic Finalization](default-workflow-agentic-finalization.md)
for the evidence document, finalizer output schema, configuration, and examples.

---

## Evidence Precedence

The gate evaluates evidence in this order. Implementations and tests must use
the same precedence so ambiguous marker combinations do not behave differently
across helper, recipe, and CLI layers.

| Precedence | Evidence | Result |
| --- | --- | --- |
| 1 | `terminal_failure=true`, or a known failing `terminal_state` | Fail nonzero. Failure evidence overrides all success-looking markers. |
| 2 | Malformed, unknown, empty, or contradictory evidence | Fail nonzero with `FAILED_INVALID_EVIDENCE`. |
| 3 | Valid publish/PR/merge/no-diff state with required details | Succeed, because publish/finalize proves the workflow reached terminal semantics. |
| 4 | `terminal_no_op=true` with known no-op state and non-empty reason | Succeed only for eligible no-op paths. |
| 5 | `implementation_completed=true` and `verification_completed=true` | Succeed only when the selected workflow path does not require publish/finalize evidence. |
| 6 | Anything else | Fail nonzero with `FAILED_MISSING_TERMINAL_EVIDENCE`. |

Contradictory examples include `terminal_failure=true` with
`terminal_success=true`, `terminal_no_op=true` without a no-op state and reason,
or `implementation_completed=true` with `verification_completed=false` and no
valid publish or no-op state.

---

## Evidence Marker API

Recipes communicate terminal progress through structured markers. Marker values
may be emitted as recipe context keys, JSON step output fields, or exported shell
environment variables. The terminal-state gate normalizes all three forms into
the same contract.

Input may use native JSON booleans or boolean strings. The canonical emitted
form is the lowercase string `"true"` or `"false"` because shell helpers and
recipe key/value output are string-based. Consumers should accept either native
booleans or canonical strings, but producers for this feature should emit the
canonical strings.

### Boolean Markers

| Marker | Type | Meaning |
| --- | --- | --- |
| `implementation_completed` | boolean | Code, docs, config, or other requested repository changes were applied. |
| `verification_completed` | boolean | Required checks for the selected workflow path completed. |
| `publish_state_reached` | boolean | The workflow reached PR, merge, no-diff, obsolete, or follow-up publication semantics. |
| `terminal_no_op` | boolean | The workflow intentionally ends without file changes or new publication work. Requires a valid state and reason. |
| `terminal_failure` | boolean | The workflow has a known failing terminal state and must exit nonzero. |

### Detail Markers

| Marker | Type | Required when | Meaning |
| --- | --- | --- | --- |
| `terminal_state` | enum | Always | Stable machine-readable state. |
| `terminal_reason` | string | Always | Actionable human-readable explanation. |
| `observed_phases` | list/string | Failure diagnostics | Phases that produced evidence before the gate ran. |
| `missing_evidence` | list/string | Failure diagnostics | Evidence required for success but not observed. |
| `change_request_url` | URL string | Publish/change-request states | Provider change request that represents the completed work. |
| `change_request_id` | string | Publish/change-request states | Provider change-request identifier when available. |
| `pr_url` | URL string | GitHub compatibility output | GitHub pull request that represents the completed work. |
| `pr_number` | positive integer string | GitHub compatibility output | GitHub pull request number when available. |
| `verification_summary` | string/object | Verification completed | Commands or checks that satisfied verification. |
| `required_next_action` | string | Finalization output | Operator or agent action needed after the terminal decision. |
| `hollow_success_detected` | boolean | Finalization output | Whether finalization detected a success-looking run without meaningful completion evidence. |
| `evidence_used` | list/string | Finalization output | Stable evidence keys used by the agentic finalizer. |
| `finalizer_output_valid` | boolean | Finalization output | Whether the structured finalizer JSON passed deterministic schema validation. |
| `finalizer_confidence` | enum string | Finalization output | `high`, `medium`, or `low`; only `high` can prove terminal success. |

### Terminal State Vocabulary

| State | Success? | Meaning |
| --- | --- | --- |
| `IMPLEMENTED_VERIFIED` | Yes | Implementation and verification both completed. |
| `FOLLOWUP_CREATED` | Yes | Meaningful work was published or represented by a follow-up PR. |
| `MERGED` | Yes | The workflow-owned PR is merged or has merge evidence. |
| `NO_DIFF_SUCCESS` | Yes | The checkout is clean and has no meaningful diff against the intended base. |
| `CLOSED_OBSOLETE` | Yes | The PR or branch is obsolete and equivalent work is already upstream or no meaningful work remains. |
| `SUPERSEDED` | Yes | A newer workflow-owned PR or issue explicitly supersedes this run. |
| `ALLOW_NO_OP` | Yes | `allow_no_op=true` was set for an eligible non-code-change path and includes a reason. |
| `MANUAL_REQUIRED` | No | A provider action is intentionally manual; `required_next_action` names the action. |
| `BLOCKED_MANUAL_PROVIDER` | No | Required provider tooling, credentials, permissions, or APIs are unavailable. |
| `FAILED_MISSING_TERMINAL_EVIDENCE` | No | The workflow stopped before implementation, verification, publish, or valid no-op evidence. |
| `FAILED_INVALID_EVIDENCE` | No | Evidence is malformed, unknown, empty, or contradictory. |
| `FAILED_FINALIZER_OUTPUT` | No | The agentic finalizer returned missing, malformed, non-JSON, or schema-invalid output. |
| `FAILED_DIRTY_WORKTREE` | No | Uncommitted work exists and terminal success cannot be proven. |
| `FAILED_MEANINGFUL_DIFF` | No | Meaningful branch changes remain but no validated publish, merge, follow-up, no-op, or implementation-plus-verification path proves closure. |
| `FAILED_WRONG_BRANCH` | No | The target checkout is not on the expected branch. |
| `FAILED_INVALID_INPUT` | No | Required context such as repository path, branch, or PR identity is invalid. |
| `FAILED_MISSING_TOOLING` | No | Required deterministic tooling is unavailable for the selected finalization path. |
| `FAILED_PR_METADATA_UNAVAILABLE` | No | Provider change-request proof is required but metadata, auth, or provider output is unavailable or ambiguous. |
| `FAILED_CLOSED_UNMERGED` | No | A PR is closed without merge evidence and meaningful local branch diff remains. |
| `BLOCKED_CI` | No | Required checks are failing or unavailable. |
| `HOLLOW_SUCCESS` | No | The recipe appeared successful but lacked implementation, verification, publish, or valid no-op evidence. |
| `INCOMPLETE` | No | Work remains and no more specific terminal failure state applies. |

---

## Recipe Output Contract

`workflow-terminal-state.yaml` is the target recipe-level gate. It wraps the
shell evaluator and preserves terminal success or failure as recipe success or
failure.

The canonical finalization result is a `workflow_result` object in full recipe
JSON. Shell helpers and individual recipe steps may expose the same fields as
flattened key/value output; those flattened fields must have the same names and
semantics.

Successful `workflow_result` content includes:

```json
{
  "terminal_success": "true",
  "terminal_state": "IMPLEMENTED_VERIFIED",
  "terminal_reason": "implementation and verification evidence present",
  "required_next_action": "No further action is required.",
  "hollow_success_detected": "false",
  "evidence_used": "implementation.completed=true,verification.completed=true",
  "finalizer_output_valid": "true",
  "finalizer_confidence": "high",
  "implementation_completed": "true",
  "verification_completed": "true",
  "publish_state_reached": "false",
  "terminal_no_op": "false",
  "terminal_failure": "false",
  "observed_phases": "workflow-prep,workflow-worktree,workflow-design,workflow-tdd,workflow-precommit-test",
  "missing_evidence": ""
}
```

Failure `workflow_result` content includes:

```json
{
  "terminal_success": "false",
  "terminal_state": "FAILED_MISSING_TERMINAL_EVIDENCE",
  "terminal_reason": "development workflow stopped after workflow-worktree; implementation, verification, publish, or explicit no-op evidence is required",
  "required_next_action": "Continue into implementation and verification, publish a meaningful diff, or emit a valid no-op state.",
  "hollow_success_detected": "false",
  "evidence_used": "observed_phases=workflow-prep,workflow-worktree",
  "finalizer_output_valid": "true",
  "finalizer_confidence": "high",
  "implementation_completed": "false",
  "verification_completed": "false",
  "publish_state_reached": "false",
  "terminal_no_op": "false",
  "terminal_failure": "true",
  "observed_phases": "workflow-prep,workflow-worktree",
  "missing_evidence": "implementation_completed,verification_completed,publish_state_reached,terminal_no_op"
}
```

The recipe runner treats a nonzero terminal-state step as recipe failure.
`smart-execute-routing.yaml` and `smart-orchestrator.yaml` must propagate that
failure instead of converting it into orchestration success.

`workflow-finalize` emits the same normalized fields after validating the
agentic finalizer output. If the finalizer output is missing or malformed, the
normalized result uses `terminal_state=FAILED_FINALIZER_OUTPUT`,
`terminal_success=false`, and `finalizer_output_valid=false`.

---

## Shell Helper API

`amplifier-bundle/tools/workflow_final_status.sh` is the target canonical
evaluator for this feature. Recipes call it from the repository checkout or
workflow worktree.

```bash
amplifier-bundle/tools/workflow_final_status.sh
```

### Inputs

The helper reads normalized environment variables. Recipe context fields map to
the same names through `RECIPE_VAR_*` variables when invoked by the runner.

| Environment variable | Type | Description |
| --- | --- | --- |
| `WORKFLOW_CLASSIFICATION` | enum string | Workflow class such as `Development`, `Default`, `Investigation`, or `Ops`. |
| `RECIPE_NAME` | string | Current recipe name. Used for diagnostics and protected-workflow detection. |
| `REPO_PATH` | path | Repository root used for Git evidence. |
| `WORKTREE_SETUP_WORKTREE_PATH` | path | Preferred workflow checkout when step 04 created or reused a worktree. |
| `BRANCH_NAME` | git ref name | Expected branch for development completion. |
| `BASE_REF` | git ref name | Intended comparison base. |
| `PR_URL` | URL string | Pull request URL, when publication happened or recovery is in progress. |
| `PR_NUMBER` | positive integer string | Pull request number, when available. |
| `ALLOW_NO_OP` | boolean string | Explicit opt-out for eligible no-op paths. Defaults to `false`. |
| `IMPLEMENTATION_COMPLETED` | boolean string | Direct implementation evidence. |
| `VERIFICATION_COMPLETED` | boolean string | Direct verification evidence. |
| `PUBLISH_STATE_REACHED` | boolean string | Direct publish/finalize evidence. |
| `TERMINAL_NO_OP` | boolean string | Direct no-op evidence. |
| `TERMINAL_FAILURE` | boolean string | Direct failure evidence. |
| `TERMINAL_STATE` | enum string | Existing state from a publish/finalize step. |
| `TERMINAL_REASON` | string | Existing reason from a publish/finalize step. |
| `OBSERVED_PHASES` | comma-separated string | Optional explicit phase list. |

### Outputs

The helper prints human-readable diagnostics to `stderr` and key/value evidence
to `stdout`.

```text
terminal_success=false
terminal_state=FAILED_MISSING_TERMINAL_EVIDENCE
terminal_reason=development workflow stopped after workflow-worktree; implementation, verification, publish, or explicit no-op evidence is required
observed_phases=workflow-prep,workflow-worktree
missing_evidence=implementation_completed,verification_completed,publish_state_reached,terminal_no_op
```

Exit codes:

| Code | Meaning |
| --- | --- |
| `0` | Terminal success is proven, or the workflow is not protected by development terminal-state validation. |
| `1` | Protected workflow lacks required evidence or has an explicit terminal failure. |
| `2` | Helper invocation is invalid, required tooling is missing, or input is malformed before evaluation can run. |

These are direct helper exit codes. Through `amplihack recipe run`, any nonzero
helper result is surfaced as recipe failure and the CLI exits nonzero; current
CLI behavior reports recipe failures as exit `1` rather than preserving the
helper's numeric code.

---

## Configuration

### `allow_no_op`

`allow_no_op` is the explicit no-op escape hatch. It is valid only when the
workflow classification allows a no-op path and the terminal evidence includes a
reason.

```bash
amplihack recipe run default-workflow \
  -c task_description="Review workflow docs and report findings without editing files" \
  -c repo_path=. \
  -c allow_no_op=true
```

`allow_no_op=true` does not hide errors. It succeeds only with
`terminal_no_op=true`, `terminal_state=ALLOW_NO_OP` or another valid no-op
state, and a non-empty `terminal_reason`.

### No Silent Relaxation

There is no configuration flag that makes development terminal validation
advisory. CI, local recipe runs, and nested `smart-orchestrator` routing all use
the same fail-closed behavior.

### Agentic Finalizer

No separate feature flag enables the agentic finalizer. It is part of the
`default-workflow` finalization path. The active agent runtime is inherited from
the workflow environment, including `AMPLIHACK_AGENT_BINARY` when nested
workflows are launched by Copilot, Claude Code, Amplifier, or Codex wrappers.

If the finalizer cannot run or cannot return valid schema-compliant JSON, the
deterministic gate reports `FAILED_FINALIZER_OUTPUT` rather than falling back to
a success-shaped shell guess.

### Node Memory Preference

When workflow checks run Node-based tooling, use the configured memory ceiling:

```bash
export NODE_OPTIONS=--max-old-space-size=32768
```

This affects child tool memory limits only. It does not change terminal-state
success rules.

---

## Failure Semantics

The terminal-state gate fails visibly when required evidence is missing.

Example diagnostic:

```text
ERROR: development terminal state not proven
recipe: default-workflow
state: FAILED_MISSING_TERMINAL_EVIDENCE
observed phases: workflow-prep, workflow-worktree, workflow-design
missing evidence: implementation_completed, verification_completed, publish_state_reached, terminal_no_op
next action: continue into workflow-tdd/workflow-precommit-test/workflow-publish or emit an explicit terminal no-op/failure state with a reason
```

This failure exits nonzero from:

- `workflow-terminal-state.yaml`
- `default-workflow.yaml`
- `smart-execute-routing.yaml`
- `smart-orchestrator.yaml`
- `amplihack recipe run smart-orchestrator ...`

Callers must treat that as an incomplete workflow, not a successful planning
run.

---

## Examples

### Early Stop After Worktree Prep Fails

```bash
amplihack recipe run smart-orchestrator \
  -c task_description="Fix cache invalidation bug" \
  -c repo_path=. \
  --format json
```

If the routed development workstream stops after analysis or worktree setup, the
command exits `1` and the JSON result contains a failed terminal-state step.

```json
{
  "status": "FAILURE",
  "failure_context": {
    "step_id": "workflow-terminal-state",
    "status": "failed"
  }
}
```

### Implementation and Verification Succeed

```text
terminal_success=true
terminal_state=IMPLEMENTED_VERIFIED
terminal_reason=implementation, pre-commit, and targeted tests completed
implementation_completed=true
verification_completed=true
```

The workflow may then continue to publish/finalize, or finish successfully if
the current recipe path ends at verification.

### No Diff Succeeds Explicitly

```text
terminal_success=true
terminal_state=NO_DIFF_SUCCESS
terminal_reason=clean branch has no meaningful diff or commits against origin/main
terminal_no_op=true
```

No-diff success is evidence-based. A clean status alone is not enough if the
workflow also observed dirty work, branch mismatch, invalid PR identity, or
malformed terminal markers.

### Contradictory Evidence Fails

```text
implementation_completed=true
verification_completed=false
terminal_no_op=false
publish_state_reached=false
```

Result:

```text
terminal_success=false
terminal_state=FAILED_MISSING_TERMINAL_EVIDENCE
missing_evidence=verification_completed,publish_state_reached,terminal_no_op
```

---

## Security Invariants

- Structured markers, not free-form agent prose, determine terminal success.
- Protected workflow classifications are validated against a small known set.
- Missing, empty, unknown, malformed, and contradictory evidence fails closed.
- `terminal_failure=true` overrides all success-looking markers.
- No-op states require explicit state and reason text.
- Git and PR evidence is read only from the workflow checkout or configured
  repository path.
- Branch names, PR numbers, and PR URLs are validated before use.
- Diagnostics do not print full environments, tokens, credentials, or auth
  headers.
- Nonzero exit codes are preserved through nested recipe routing.

---

## See Also

- [Default Workflow Agentic Finalization](./default-workflow-agentic-finalization.md)
- [RecipeResult Reference](./recipe-result.md)
- [Recipe CLI Reference](./recipe-cli-reference.md)
- [Workflow Execution Guardrails Reference](./workflow-execution-guardrails.md)
- [Default Coding Workflow](../concepts/default-workflow.md)
- [How to Diagnose Workflow Terminal-State Failures](../howto/diagnose-workflow-terminal-state.md)
