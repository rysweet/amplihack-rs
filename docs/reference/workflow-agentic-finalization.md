# Workflow Agentic Finalization Reference

> [Home](../index.md) > Reference > Workflow Agentic Finalization

This reference defines the issue #769 contract for bounded agentic
finalization. The implementation layers `workflow_agentic_finalization.sh` and
schema validation on top of the deterministic `workflow-terminal-state`,
`workflow_pr_ready.sh`, `workflow_final_status.sh`, and `workflow-complete`
gates.

## Contents

- [Purpose](#purpose)
- [Finalization Surfaces](#finalization-surfaces)
- [Finalization Pipeline](#finalization-pipeline)
- [Helper API](#helper-api)
- [Input Context](#input-context)
- [Output Schema](#output-schema)
- [Decision and Terminal-State Semantics](#decision-and-terminal-state-semantics)
- [Implementation Requirements](#implementation-requirements)
- [Artifact Scope](#artifact-scope)
- [Failure Semantics](#failure-semantics)
- [Security Invariants](#security-invariants)

---

## Purpose

`workflow-finalize` must not report code-change completion from brittle or
ambiguous evidence. Issue #769 makes finalization more resilient
by combining:

1. deterministic repository and PR evidence,
2. Artifact Guard scope checks,
3. bounded agent assessment,
4. strict JSON schema validation, and
5. the shared terminal-state vocabulary.

The agentic finalizer is advisory. It can explain why a branch is ready,
blocked, or already finalized, but it cannot override deterministic failures.

---

## Finalization Surfaces

| Surface | Contract |
| --- | --- |
| Initial terminal probe | `workflow-terminal-state.yaml` runs first and emits `terminal_state`, `terminal_success`, `publish_status`, and continuation flags. Proven terminal states skip redundant finalize work. |
| PR readiness | `workflow_pr_ready.sh` handles open, merged, closed-after-merge, and closed-unmerged PR readiness paths. |
| Final status | `workflow_final_status.sh` reports no-diff, merged, closed-after-merge, closed-unmerged, meaningful-diff, blocked-CI, and follow-up states. |
| Agentic helper | `amplifier-bundle/tools/workflow_agentic_finalization.sh` collects deterministic evidence, invokes the bounded agent assessment through `AMPLIHACK_AGENT_BINARY`, and schema-validates the result. |
| Output shape | `workflow-complete` emits existing terminal fields plus `agentic_decision`, `agentic_confidence`, `agentic_blocking_reasons`, and `agentic_evidence_summary`. |
| Tests | Integration tests cover helper existence, executable mode, schema validation, malformed-output rejection, hollow-success rejection, contradiction rejection, and artifact scope. |

---

## Finalization Pipeline

`workflow-finalize.yaml` runs in this order:

1. Run `workflow-terminal-state` as the first probe.
2. If `terminal_success=true`, skip redundant cleanup/publish work and carry the
   proven terminal state into `workflow-complete`.
3. If the state is active (`FOLLOWUP_CREATED`) and `should_finalize=true`, run
   cleanup, Artifact Guard, push cleanup, PR readiness, CI/mergeability, and final
   status as the current recipe does.
4. Collect deterministic evidence from Git status, branch/base diff, validation
   commands, Artifact Guard, PR metadata, `workflow_pr_ready.sh`, and
   `workflow_final_status.sh`.
5. Invoke `workflow_agentic_finalization.sh` with only the
   collected evidence and bounded task context.
6. Validate helper stdout with `jq` against the required schema.
7. Reject malformed output, unknown enum values, unsafe artifact scope, hollow
   success, and contradictions with deterministic evidence.
8. Map the accepted finalizer decision to the shared terminal-state fields used
   by `workflow-complete`.

The pipeline is intentionally additive. Existing deterministic terminal-state
behavior remains the source of truth when it proves a terminal success or
terminal failure.

---

## Helper API

The helper lives at:

```bash
amplifier-bundle/tools/workflow_agentic_finalization.sh
```

The helper reads recipe context and deterministic evidence from environment
variables and files, then writes one JSON object to stdout. It must be executable
after install and must fail closed when dependencies, input, repository state, or
agent output are invalid.

Direct invocation:

```bash
export REPO_PATH="$PWD"
export TASK_DESCRIPTION="Adaptive recovery for issue #769 workflow finalization"
export ISSUE_NUMBER=769
export BRANCH_NAME="$(git branch --show-current)"
export PR_URL="https://github.com/rysweet/amplihack-rs/pull/123"
export FINALIZATION_EVIDENCE_FILE="$(mktemp -t workflow-finalization-evidence-XXXXXX.json)"

bash amplifier-bundle/tools/workflow_agentic_finalization.sh
```

Exit codes:

| Code | Meaning |
| --- | --- |
| `0` | Valid finalizer JSON was emitted. |
| `1` | Finalization reached a known blocking state, such as unsafe artifacts, failed checks, or uncommitted meaningful work. |
| `2` | The helper could not run because input, dependencies, repository state, or JSON output was invalid. |

---

## Input Context

| Context key / environment variable | Required | Description |
| --- | --- | --- |
| `repo_path` / `REPO_PATH` | Yes | Repository root used for Git, Artifact Guard, and PR inspection. |
| `worktree_setup.worktree_path` / `WORKTREE_SETUP_WORKTREE_PATH` | No | Preferred workflow worktree. When present, evidence collection runs there instead of `repo_path`. |
| `task_description` / `TASK_DESCRIPTION` | Yes | Original task used to evaluate whether the implementation satisfies the requested outcome. |
| `final_requirements` / `FINAL_REQUIREMENTS` | No | Normalized requirements from earlier workflow phases. |
| `issue_number` / `ISSUE_NUMBER` | No | Issue the branch or PR should reference. |
| `branch_name` / `BRANCH_NAME` | No | Expected branch. Defaults to the current branch. |
| `base_ref` / `BASE_REF` | No | Base ref for diff comparison. Defaults to the same base resolution as `workflow-terminal-state`. |
| `pr_number` / `PR_NUMBER` | No | Pull request number to inspect. |
| `pr_url` / `PR_URL` | No | Pull request URL to inspect. |
| `finalization_evidence_file` / `FINALIZATION_EVIDENCE_FILE` | No | Optional path where the helper writes the deterministic evidence bundle. |
| `AMPLIHACK_AGENT_BINARY` | Yes | Agent binary used for the bounded finalization assessment. |
| `allow_no_op` / `ALLOW_NO_OP` | No | Allows an explicit no-op only when paired with a valid no-op state and reason. |

`jq` is required for schema validation. `gh` is required only for GitHub PR
metadata paths. GitHub credentials are read through the existing authenticated
`gh` CLI session and must not be written into finalization JSON.

---

## Output Schema

The helper emits one JSON object:

```json
{
  "schema_version": "1",
  "decision": "ready",
  "terminal_success": false,
  "terminal_state": "FOLLOWUP_CREATED",
  "terminal_reason": "implementation is committed, validation passed, and PR is open for issue #769",
  "publish_status": "FOLLOWUP_CREATED",
  "ready_for_review": true,
  "artifact_scope": {
    "safe": true,
    "blocked_paths": [],
    "runtime_artifacts_present": false
  },
  "evidence": {
    "branch": "feat/issue-769-the-recipe-exitfinalization-step-is-brittle-and-of",
    "issue_number": "769",
    "pr_url": "https://github.com/rysweet/amplihack-rs/pull/123",
    "modified_paths": [],
    "validation": [
      "pre-commit run --all-files",
      "cargo test --test workflow_finalize_terminal_state",
      "cargo test --test workflow_finalize_resilience"
    ]
  },
  "agent_assessment": {
    "summary": "Finalization evidence is complete and review scope excludes generated runtime artifacts.",
    "hollow_success": false,
    "required_actions": []
  }
}
```

Required top-level fields:

| Field | Type | Description |
| --- | --- | --- |
| `schema_version` | string | Schema version. Initial value is `"1"`. |
| `decision` | enum | `ready`, `blocked`, `needs_human`, or `finalized`. |
| `terminal_success` | boolean | `true` only for terminal success states that require no further workflow action. |
| `terminal_state` | enum | Shared terminal-state vocabulary. |
| `terminal_reason` | string | Non-empty actionable explanation. |
| `publish_status` | enum/string | Publish-facing state used by downstream recipe steps. |
| `ready_for_review` | boolean | `true` when the current work is represented by an open reviewable PR but is not merged yet. |
| `artifact_scope.safe` | boolean | Whether generated artifacts are absent from commit scope. |
| `artifact_scope.blocked_paths` | array | Unsafe repo-relative artifact paths, if any. |
| `evidence` | object | Deterministic evidence used for the decision. |
| `agent_assessment.hollow_success` | boolean | `true` when the finalizer could not prove meaningful completion. |
| `agent_assessment.required_actions` | array | Concrete follow-up actions for blocked decisions. |

The helper emits native JSON booleans. Recipe wiring may translate accepted
values into string context variables because existing YAML recipe context is
string-oriented.

Unknown additional fields are ignored. Unknown enum values in required fields
fail closed.

---

## Decision and Terminal-State Semantics

`decision` is the agentic finalizer vocabulary. `terminal_state` remains the
machine-checkable workflow vocabulary defined in
[Workflow Terminal-State Reference](./workflow-terminal-state.md).

| Decision | Typical terminal state | Meaning |
| --- | --- | --- |
| `ready` | `FOLLOWUP_CREATED` | The branch has meaningful completed work represented by an open PR or follow-up publication. It is ready for review, not terminally complete. |
| `finalized` | `NO_DIFF_SUCCESS` | No meaningful diff remains against the base ref. |
| `finalized` | `MERGED` | The workflow-owned PR is merged. |
| `finalized` | `CLOSED_OBSOLETE` | The PR or branch is obsolete because equivalent work is already upstream. |
| `blocked` | `FAILED_INVALID_EVIDENCE` | Output was malformed, contradictory, or incomplete. |
| `blocked` | `FAILED_DIRTY_WORKTREE` | Meaningful uncommitted work remains outside a valid no-op path. |
| `blocked` | `BLOCKED_CI` | Required validation or CI is failing. |

`FOLLOWUP_CREATED` is an active continuation state. It can mean "the current
work is published and ready for review," but it is not a terminal success for
the whole lifecycle unless project policy defines PR readiness as the completion
point for that workflow run. Merged, obsolete, and no-diff states are terminal
success states.

---

## Implementation Requirements

The issue #769 implementation includes:

1. `amplifier-bundle/tools/workflow_agentic_finalization.sh` with executable bit
   preserved by install/package flows.
2. Recipe wiring in `workflow-finalize.yaml` after deterministic evidence
   collection and before `workflow-complete`.
3. A deterministic evidence bundle written before the helper runs.
4. `jq` schema validation for helper output.
5. Fail-closed handling for malformed JSON, missing required fields, unknown enum
   values, unsafe artifacts, hollow success, and deterministic contradiction.
6. Tests for helper discovery, executable-bit validation, schema acceptance,
   malformed-output rejection, blocked artifact scope, no-diff finalization,
   ready-for-review mapping, and contradiction rejection.

---

## Artifact Scope

Generated runtime artifacts are never valid commit scope. Artifact Guard blocks
them before broad staging and before publication/finalization gates.

Default blocked examples include:

| Path | Reason |
| --- | --- |
| `.claude/runtime/` | Agent runtime state and generated logs. |
| `target/` when staged, tracked, or unignored | Rust build output. |
| `node_modules/` | Dependency tree. |
| `dist/`, `build/`, `coverage/` | Generated build or test output. |

`.gitignore` is noise reduction, not authorization. Ignored generated output can
still block finalization if it leaks into the parent worktree or commit scope.

---

## Failure Semantics

Finalization fails closed when any of these conditions are true:

- the helper is missing, not executable, or cannot find required tools;
- Artifact Guard reports blocked paths such as `.claude/runtime/`;
- helper output is non-JSON, empty JSON, missing required fields, or contains
  unknown enum values;
- the helper claims success while `agent_assessment.hollow_success` is `true`;
- deterministic evidence contradicts the agentic assessment;
- PR identity does not match the repository, branch, or expected issue; or
- required validation is absent or failing.

Failure output includes `decision="blocked"`, a failing `terminal_state`, a
non-empty `terminal_reason`, and actionable `required_actions` when available.

---

## Security Invariants

- Treat finalizer output as untrusted until `jq` schema validation passes.
- Never execute commands proposed by the finalizer.
- Quote repository paths, branch names, PR URLs, and issue values in shell steps.
- Redact credentials and authenticated remotes from captured evidence.
- Do not commit raw transcripts, `.claude/runtime`, local config, dependency
  trees, build output, or generated logs.
- Do not copy `.claude/runtime` wholesale into `/tmp`; extract only the minimum
  redacted evidence needed for debugging outside the repository, then delete the
  runtime directory.
- Do not weaken Artifact Guard to make finalization pass.

## See Also

- [How to Finalize an Existing Workflow Branch](../howto/finalize-existing-workflow-branch.md)
- [Tutorial: Workflow Agentic Finalization](../tutorials/workflow-agentic-finalization.md)
- [Workflow Terminal-State Reference](./workflow-terminal-state.md)
- [Artifact Guard](../artifact-guard.md)
