# Default Workflow Agentic Finalization

> [Home](../index.md) > Reference > Default Workflow Agentic Finalization

`default-workflow` finalization is a fail-closed terminal assessment for
development workstreams. It keeps deterministic scripts responsible for
collecting evidence, validating schema, persisting outputs, and choosing the
process exit code. It uses an agentic finalizer only for the judgment-heavy part:
classifying the terminal state from structured evidence and explaining the next
action.

This page is the target implementation contract for the feature. If recipe,
helper, or test behavior differs, update the implementation to match this
contract rather than weakening finalization into best-effort prose parsing.

The finalizer does not mutate Git state, create or edit pull requests, merge,
or decide success from free-form prose. Terminal success is reported only after
the deterministic finalization gate validates a known terminal state and the
evidence needed for that state.

## When Finalization Runs

Agentic finalization runs at the end of `default-workflow` and any
`smart-orchestrator` development workstream routed through `default-workflow`.
It also runs when publish or PR handling short-circuits into an already-terminal
state such as merged, obsolete, no-diff, or blocked CI.

Use it through the normal workflow entry point:

```bash
export NODE_OPTIONS=--max-old-space-size=32768

amplihack recipe run smart-orchestrator \
  -c task_description="Fix cache invalidation bug" \
  -c repo_path=. \
  --format json > recipe-result.json
```

Standalone `default-workflow` runs use the same finalization contract:

```bash
amplihack recipe run default-workflow \
  -c task_description="Fix cache invalidation bug" \
  -c repo_path=. \
  -c branch_name="$(git branch --show-current)" \
  --format json > workflow-result.json
```

Maintainers may invoke the finalization path directly for diagnosis when the
implementation, verification, publish, and PR context are already known:

```bash
amplihack recipe run workflow-finalize \
  -c repo_path=. \
  -c branch_name="$(git branch --show-current)" \
  -c base_ref=origin/main \
  -c pr_number=123 \
  --format json > finalize-result.json
```

## Finalization Pipeline

Finalization has three phases.

| Phase | Owner | Responsibility |
| --- | --- | --- |
| Evidence collection | Deterministic shell/JSON steps | Read Git status, branch/base diff, PR metadata when provider-safe, CI state, implementation and verification markers, publish result, prior recipe output, and observed phases. |
| Terminal assessment | Structured agentic finalizer | Classify one terminal state from the evidence document, explain the reason, identify hollow success, and name the required next action. |
| Validation and persistence | Deterministic shell/JSON steps | Validate the finalizer schema, reject missing or malformed output, enforce terminal-state evidence rules, persist normalized fields, and return the recipe exit status. |

The agentic finalizer is deliberately boxed in: it receives structured evidence
and must return a small JSON object. It cannot make an unsupported state
successful, ignore dirty work, override failed CI, or repair invalid PR metadata.

## Observed Failure Modes This Model Addresses

Recent workflow logs, tests, and history showed repeated brittle finalization
failures. The agentic finalizer is designed around these concrete cases:

| Failure mode | Observed cause | Required finalization behavior |
| --- | --- | --- |
| Brittle parsing | Shell snippets inferred completion from text fragments such as publish status strings, PR URLs, or partial command output. | Parse only structured JSON/key-value evidence. Free-form text may appear in `reason`, but cannot prove success. |
| Missing or stale PR metadata | A stale `pr_number`, missing `pr_url`, mismatched head branch, stale head SHA, or unavailable `gh` metadata caused the wrong PR state to be trusted. | Validate PR identity against repo, branch, base, and head SHA. If the needed metadata is unavailable or mismatched, fail closed. |
| Dirty worktree misclassification | Generated files, unstaged edits, or leftover workflow artifacts were treated as harmless no-diff states. | Dirty worktree evidence blocks terminal success unless the workflow explicitly commits, removes, or accounts for the changes before finalization. |
| Closed-unmerged PR handling | Closed PRs without merge evidence were sometimes treated like completed work even when branch diffs remained. | Return `CLOSED_OBSOLETE` only when local no-diff/obsolete proof exists; otherwise return a failing closed-unmerged state. |
| Remaining meaningful diff | Branch changes remain but no valid publish, merge, follow-up, or implementation-plus-verification path proves closure. | Return `FAILED_MEANINGFUL_DIFF` unless the diff is intentionally represented by a validated PR/follow-up or another success state with deterministic proof. |
| Missing tooling | `gh`, `jq`, Git metadata, or provider auth was absent on paths that required it. | Tooling absence is reported as a deterministic failure such as `FAILED_MISSING_TOOLING` or `FAILED_PR_METADATA_UNAVAILABLE`, not as no-op success. |
| Failed CI | Open PRs with failed or unavailable required checks reached final output that looked complete. | Return `BLOCKED_CI` with failing check evidence and `terminal_success=false`. |
| Hollow success | A recipe exited `0` after setup, planning, empty agent output, or inaccessible codebase messages without implementation, verification, publish, or valid no-op evidence. | Return `HOLLOW_SUCCESS` or `FAILED_MISSING_TERMINAL_EVIDENCE`; malformed finalizer output also fails closed. |

## Input Evidence Document

The finalizer receives one normalized evidence document. Producers may collect
the data from recipe context, step outputs, shell helpers, Git, and provider
metadata, but the finalizer sees a single JSON object.

| Field | Type | Description |
| --- | --- | --- |
| `schema_version` | integer | Evidence schema version. Current value is `1`. |
| `recipe_name` | string | Usually `default-workflow` or `workflow-finalize`. |
| `workflow_classification` | string | Classification such as `Development`, `Default`, `Feature`, `Bugfix`, or `Refactor`. |
| `repo_path` | string | Repository root or workflow worktree used for local evidence. |
| `branch_name` | string | Expected branch for the workflow-owned work. |
| `base_ref` | string | Intended comparison base. |
| `git` | object | Clean/dirty status, branch match, diff status, commits ahead, and base resolution details. |
| `pr` | object | Provider, URL, number, state, merge evidence, head branch, base branch, head SHA, and identity-match booleans. Empty when no PR exists. |
| `ci` | object | Required check state, failing checks, pending checks, and CI metadata availability. |
| `implementation` | object | Whether code/docs/config work was applied and where that evidence came from. |
| `verification` | object | Whether required tests, pre-commit, or validation completed and summaries of those checks. |
| `publish` | object | Publish result, follow-up PR details, no-diff state, or provider-specific skip reason. |
| `observed_phases` | array of strings | Recipe phases that produced evidence before finalization. |
| `agent_outputs` | object | Summaries needed to detect empty, inaccessible, or generic agent responses. |
| `prior_terminal_state` | object | Existing terminal markers from publish or terminal-state probes, when present. |

Evidence collectors must record absence explicitly. For example, a missing PR is
`"pr": {"present": false}`, not an omitted `pr` object.

## Agentic Finalizer Output API

The finalizer returns exactly one JSON object:

```json
{
  "schema_version": 1,
  "terminal_state": "BLOCKED_CI",
  "terminal_success": false,
  "confidence": "high",
  "reason": "PR #123 exists and matches this branch, but required CI checks are failing.",
  "required_next_action": "Fix failing CI checks before merge.",
  "hollow_success_detected": false,
  "evidence_used": [
    "pr.state=OPEN",
    "pr.head_branch_matches=true",
    "ci.state=FAILURE"
  ]
}
```

| Field | Required | Type | Rules |
| --- | --- | --- | --- |
| `schema_version` | Yes | integer | Must be `1`. Unknown versions fail closed. |
| `terminal_state` | Yes | enum string | Must be one known terminal state from this reference. |
| `terminal_success` | Yes | boolean | Must match the success semantics for `terminal_state`. |
| `confidence` | Yes | enum string | `high`, `medium`, or `low`. Only `high` can prove terminal success; `medium` and `low` are diagnostic and must return a non-success state. |
| `reason` | Yes | string | Human-readable explanation. Must be non-empty and evidence-backed. |
| `required_next_action` | Yes | string | Operator or agent action needed next. For success states, explain why no further action is required. |
| `hollow_success_detected` | Yes | boolean | `true` when the run looked successful but lacked meaningful completion evidence. |
| `evidence_used` | Yes | array of strings | Stable evidence keys used for the decision. Must not contain secrets or full environment dumps. |

Malformed JSON, missing fields, extra top-level prose, unknown states,
contradictory values, or unsupported success claims produce
`FAILED_FINALIZER_OUTPUT` and a nonzero recipe result.

## Terminal States

| State | Success? | Meaning |
| --- | --- | --- |
| `MERGED` | Yes | Workflow-owned PR is merged or closed with merge evidence. |
| `CLOSED_OBSOLETE` | Yes | PR or branch is obsolete and local evidence proves no meaningful work remains. |
| `NO_DIFF_SUCCESS` | Yes | Worktree is clean with no meaningful diff or commits against base. |
| `FOLLOWUP_CREATED` | Yes | Meaningful remaining work is represented by a new workflow-owned follow-up PR or issue. |
| `SUPERSEDED` | Yes | A newer workflow-owned PR or issue explicitly supersedes this run and durable metadata links the old run to the replacement. |
| `IMPLEMENTED_VERIFIED` | Yes | Implementation and required verification completed on a path that does not require publish/merge evidence. |
| `ALLOW_NO_OP` | Yes | Explicit no-op path was allowed and includes evidence-backed reason text. |
| `BLOCKED_CI` | No | Required checks are failing, pending beyond policy, or unavailable when required. |
| `FAILED_DIRTY_WORKTREE` | No | Uncommitted or untracked work prevents terminal success. |
| `FAILED_MEANINGFUL_DIFF` | No | Meaningful branch changes remain but no validated publish, merge, follow-up, no-op, or implementation-plus-verification path proves closure. |
| `FAILED_CLOSED_UNMERGED` | No | PR is closed without merge evidence and meaningful branch diff remains. |
| `FAILED_PR_METADATA_UNAVAILABLE` | No | GitHub PR proof is required but metadata or auth is missing, stale, or ambiguous. |
| `FAILED_MISSING_TOOLING` | No | Required deterministic tooling such as `git`, `jq`, or provider CLI support is missing for the selected path. |
| `FAILED_INVALID_EVIDENCE` | No | Evidence is malformed, contradictory, unknown, or incomplete. |
| `FAILED_FINALIZER_OUTPUT` | No | The agentic finalizer returned missing, malformed, non-JSON, or schema-invalid output. |
| `FAILED_MISSING_TERMINAL_EVIDENCE` | No | Development workflow stopped before implementation, verification, publish, or valid no-op evidence. |
| `HOLLOW_SUCCESS` | No | The recipe appeared successful but agents produced empty/generic output or could not access the codebase. |
| `INCOMPLETE` | No | Work remains and no more specific terminal failure state applies. |

`terminal_failure=true` is derived for all non-success states. Failure states
override success-looking implementation, verification, publish, and no-op
markers.

## Deterministic Validation Rules

After the finalizer returns JSON, the deterministic gate validates it before
persisting or reporting the result.

1. The output must be a single JSON object using schema version `1`.
2. `terminal_state` must be known.
3. `terminal_success` must match the state table.
4. `reason`, `required_next_action`, and `evidence_used` must be non-empty.
5. Only `confidence=high` can produce terminal success.
6. `hollow_success_detected=true` cannot produce terminal success.
7. Success states must have their required deterministic proof:
   `MERGED` needs merge evidence, `NO_DIFF_SUCCESS` needs clean no-diff proof,
   `CLOSED_OBSOLETE` needs local no-diff or obsolete proof,
   `FOLLOWUP_CREATED` needs a durable follow-up identifier, `SUPERSEDED` needs
   a durable replacement PR or issue identifier plus a supersession reason,
   `IMPLEMENTED_VERIFIED` needs implementation and verification evidence, and
   `ALLOW_NO_OP` needs explicit no-op authorization plus reason text.
8. Dirty worktree, invalid PR identity, failed CI, malformed evidence, and
   missing required tooling override success-looking output.
9. The normalized result is persisted even for failure states so operators can
   diagnose the run from recipe JSON.

## Canonical Result Schema

Successful and failing finalization results use the same normalized result
object. In full recipe JSON this object is stored as `workflow_result`. Shell
helpers and individual recipe steps may also expose the same fields as flattened
key/value outputs, but the field names and meanings are identical.

| Output | Meaning |
| --- | --- |
| `terminal_success` | Boolean string or JSON boolean indicating whether finalization proved success. |
| `terminal_state` | Stable state from the terminal-state vocabulary. |
| `terminal_reason` | Validated finalizer reason or deterministic validation failure reason. |
| `required_next_action` | Actionable next step. |
| `hollow_success_detected` | Whether hollow success was detected. |
| `evidence_used` | Evidence keys used for classification. |
| `finalizer_schema_version` | Accepted finalizer schema version. |
| `finalizer_confidence` | Finalizer confidence after validation. |
| `finalizer_output_valid` | `true` only when the finalizer JSON passed schema validation. |
| `implementation_completed` | Normalized implementation evidence. |
| `verification_completed` | Normalized verification evidence. |
| `publish_state_reached` | Normalized publish/PR/follow-up evidence. |
| `terminal_no_op` | Whether the final state is an explicit no-op success. |
| `terminal_failure` | `true` for all non-success terminal states. |
| `pr_url` | PR URL when a validated PR or follow-up exists. |
| `pr_number` | PR number when a validated GitHub PR exists. |

## Configuration

No feature flag is required. Agentic finalization is part of the
`default-workflow` terminal path.

| Setting or tool | Required when | Notes |
| --- | --- | --- |
| `NODE_OPTIONS=--max-old-space-size=32768` | Large nested workflow runs or Node-heavy checks | Recommended saved preference for this repository. It does not relax finalization rules. |
| `AMPLIHACK_AGENT_BINARY` | Nested agentic finalizer sessions | Preserved by the launcher so finalization uses the active supported agent runtime. |
| `git` | Always | Required for repository, branch, worktree, and diff evidence. |
| `jq` | Always | Required for structured JSON normalization and validation in shell steps. |
| `gh` | GitHub PR metadata path | Required only when a GitHub PR URL or GitHub remote makes PR metadata necessary. |
| `GH_TOKEN` or GitHub auth | GitHub PR metadata path | Missing or invalid auth fails closed when GitHub metadata is required. |
| `allow_no_op=true` | Explicit no-op tasks | Allows only valid no-op success states with evidence and reason text. It cannot bypass dirty work, failed CI, or malformed finalizer output. |

There is no configuration that converts finalization failures into advisory
warnings.

## Examples

### Open PR with failing CI

```json
{
  "schema_version": 1,
  "terminal_state": "BLOCKED_CI",
  "terminal_success": false,
  "confidence": "high",
  "reason": "PR #123 exists and matches this branch, but required CI checks are failing.",
  "required_next_action": "Fix failing CI checks before merge.",
  "hollow_success_detected": false,
  "evidence_used": [
    "pr.state=OPEN",
    "pr.head_branch_matches=true",
    "ci.state=FAILURE"
  ]
}
```

The recipe exits nonzero and persists `terminal_state=BLOCKED_CI`.

### No-diff rerun after equivalent work is upstream

```json
{
  "schema_version": 1,
  "terminal_state": "NO_DIFF_SUCCESS",
  "terminal_success": true,
  "confidence": "high",
  "reason": "The worktree is clean and has no meaningful diff or commits against origin/main.",
  "required_next_action": "No further action is required.",
  "hollow_success_detected": false,
  "evidence_used": [
    "git.worktree_clean=true",
    "git.branch_diff_status=no-diff",
    "git.commits_ahead=0"
  ]
}
```

The deterministic gate accepts this only if local Git evidence proves the clean
no-diff state.

### Closed unmerged PR with remaining diff

```json
{
  "schema_version": 1,
  "terminal_state": "FAILED_CLOSED_UNMERGED",
  "terminal_success": false,
  "confidence": "high",
  "reason": "PR #123 is closed without merge evidence and the branch still has a meaningful diff.",
  "required_next_action": "Reopen the PR, create an intentional follow-up branch, or remove the remaining diff.",
  "hollow_success_detected": false,
  "evidence_used": [
    "pr.state=CLOSED",
    "pr.merged=false",
    "git.branch_diff_status=has-diff"
  ]
}
```

Closed-unmerged PRs are never treated as success while meaningful work remains.

### Hollow success after planning-only output

```json
{
  "schema_version": 1,
  "terminal_state": "HOLLOW_SUCCESS",
  "terminal_success": false,
  "confidence": "high",
  "reason": "The run produced planning output but no implementation, verification, publish, or valid no-op evidence.",
  "required_next_action": "Resume default-workflow from implementation or emit a valid no-op state with evidence.",
  "hollow_success_detected": true,
  "evidence_used": [
    "observed_phases=workflow-prep,workflow-worktree,workflow-design",
    "implementation.completed=false",
    "verification.completed=false",
    "publish.state_reached=false"
  ]
}
```

The recipe exits nonzero even if earlier planning steps exited `0`.

### Malformed finalizer output

```text
The PR looks good to me. CI is probably fine.
```

This is not valid finalizer output. The deterministic gate reports
`FAILED_FINALIZER_OUTPUT`, sets `terminal_success=false`, and exits nonzero.

## Operator Troubleshooting

Use these commands to inspect finalization results:

```bash
jq '.. | objects | select(has("terminal_state")) | {
  terminal_success,
  terminal_state,
  terminal_reason,
  required_next_action,
  hollow_success_detected,
  evidence_used,
  finalizer_output_valid
}' recipe-result.json
```

For GitHub-backed PR decisions, confirm the PR identity:

```bash
gh pr view 123 --json number,state,mergedAt,headRefName,baseRefName,headRefOid,statusCheckRollup
git status --short
git rev-parse HEAD
git diff --stat origin/main...HEAD
```

For provider-neutral no-diff decisions, inspect local evidence:

```bash
git status --short
git rev-parse --verify origin/main
git diff --stat origin/main...HEAD
git rev-list --count origin/main..HEAD
```

Do not override `BLOCKED_CI`, `FAILED_MEANINGFUL_DIFF`, `HOLLOW_SUCCESS`,
`FAILED_FINALIZER_OUTPUT`, or `FAILED_INVALID_EVIDENCE` in CI scripts. These
states mean finalization did not prove successful closure.

## See Also

- [Workflow Terminal-State Reference](workflow-terminal-state.md)
- [Workflow Terminal-State Provider Safety](workflow-terminal-state-provider-safety.md)
- [How to Diagnose Workflow Terminal-State Failures](../howto/diagnose-workflow-terminal-state.md)
- [Workflow Publish and Finalize Resilience](../operations/workflow-resilience.md)
