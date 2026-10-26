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
- [Branch PR Lookup](#branch-pr-lookup)
- [Configuration](#configuration)
- [Failure Semantics](#failure-semantics)
- [Examples](#examples)
- [Scoped-PR Signal and Agentic Adjudication](#scoped-pr-signal-and-agentic-adjudication)
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
classifier, not as a brittle text parser. Deterministic shell/JSON steps collect
Git, PR, CI, implementation, verification, publish, and observed-phase evidence
into typed recipe state. The classifier derives one terminal state from that
typed evidence, applying guards before any success state, and chooses the recipe
exit status.

An agentic finalizer step contributes a human-readable narrative only. That
narrative is captured as an artifact and is **never** parsed with `jq`, regex,
fence stripping, or JSON extraction to drive control flow. A successful run
cannot be reported as failed merely because the finalizer emitted prose instead
of parser-compatible JSON. Malformed or contradictory typed evidence fails
closed as `FAILED_INVALID_EVIDENCE`; a failed reporting step after a successful
implementation classifies as `FAILED_REPORTING` and preserves durable evidence.

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
| 3 | Valid publish/PR/merge/no-diff state with required details, including an open PR for the run's worktree branch found by the [branch PR lookup](#branch-pr-lookup) | Succeed, because publish/finalize proves the workflow reached terminal semantics. |
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
| `evidence_used` | list/string | Finalization output | Stable typed-evidence keys used by the deterministic classifier. |
| `reporting_failure` | boolean | Finalization output | `true` when a reporting/finalization step itself failed (from that step's exit status). The classifier combines it with implementation/verification evidence to produce `FAILED_REPORTING` while durable evidence is preserved. |

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
| `FAILED_INVALID_EVIDENCE` | No | Typed evidence is malformed, unknown, empty, or contradictory. |
| `FAILED_IMPLEMENTATION` | No | Durable implementation or verification evidence is absent or failed while meaningful work remains. |
| `FAILED_REPORTING` | No | Implementation succeeded but a reporting/finalization step failed. Durable evidence (`pr_url`, `pr_number`, implementation/verification markers) is preserved and reported. |
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

> **Retired:** `FAILED_FINALIZER_OUTPUT` no longer exists. Terminal
> classification is derived from typed evidence, so an agentic finalizer can no
> longer produce a machine-control parse failure. Reporting-step failures now
> classify as `FAILED_REPORTING` (durable evidence preserved), and malformed
> deterministic evidence classifies as `FAILED_INVALID_EVIDENCE`.

> **Vocabulary scope:** `MANUAL_REQUIRED` and `BLOCKED_MANUAL_PROVIDER` are part
> of this broader terminal-state vocabulary but are not emitted by the agentic
> finalizer's own classifier. They reach a finalization result only through
> `prior_terminal_state` propagation from an upstream deterministic probe.

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
  "reporting_failure": "false",
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
  "reporting_failure": "false",
  "implementation_completed": "false",
  "verification_completed": "false",
  "publish_state_reached": "false",
  "terminal_no_op": "false",
  "terminal_failure": "true",
  "observed_phases": "workflow-prep,workflow-worktree",
  "missing_evidence": "implementation_completed,verification_completed,publish_state_reached,terminal_no_op"
}
```

A reporting-failure `workflow_result` preserves durable evidence:

```json
{
  "terminal_success": "false",
  "terminal_state": "FAILED_REPORTING",
  "terminal_reason": "implementation succeeded but a reporting step failed",
  "required_next_action": "Re-run the reporting step; the implementation is intact.",
  "hollow_success_detected": "false",
  "evidence_used": "implementation.completed=true,verification.completed=true,finalizer_step_status.reporting_failure=true",
  "reporting_failure": "true",
  "implementation_completed": "true",
  "verification_completed": "true",
  "publish_state_reached": "true",
  "terminal_no_op": "false",
  "terminal_failure": "true",
  "pr_url": "https://github.com/rysweet/amplihack-rs/pull/123",
  "pr_number": "123"
}
```

The recipe runner treats a nonzero terminal-state step as recipe failure.
`smart-execute-routing.yaml` and `smart-orchestrator.yaml` must propagate that
failure instead of converting it into orchestration success.

`workflow-finalize` emits the same normalized fields after classifying the
terminal state from typed evidence. The agentic finalizer narrative is never
parsed; if a reporting/finalization step fails after successful implementation,
the normalized result uses `terminal_state=FAILED_REPORTING`,
`terminal_success=false`, and `reporting_failure=true`, and preserves durable
`pr_url`/`pr_number` and implementation/verification markers. Malformed typed
evidence uses `terminal_state=FAILED_INVALID_EVIDENCE`.

---

## Shell Helper API

`amplifier-bundle/tools/workflow_final_status.sh` is the target canonical
evaluator for this feature. Recipes call it from the repository checkout or
workflow worktree.

```bash
amplifier-bundle/tools/workflow_final_status.sh
```

### Inputs

The helper reads recipe values from `RECIPE_VAR_<key>`, as the runner exports
them. A dotted input `key.field` is the `field` of the JSON object in
`RECIPE_VAR_<key>`, read with `rv_field` from
[`recipe_var.sh`](../../amplifier-bundle/tools/recipe_var.sh); a nested
`RECIPE_VAR_<key>__<field>` name or a bare upper-case name such as `PR_NUMBER`
is never read. When you run the helper directly, set the `RECIPE_VAR_` names;
in a recipe run, pass values with `-c <key>=<value>` (see
[Step outputs ignore inherited bare variables](auto-drive-to-merge.md#step-outputs-ignore-inherited-bare-variables)).
`workflow_pr_ready.sh` and `workflow_terminal_evidence.sh` read the PR inputs
the same way.

The `probe-terminal-state` step reads its own inputs (`repo_path`,
`branch_name`, `base_ref`, `pr_number`, `pr_url`, `goal_already_met`) from
`RECIPE_VAR_<key>`, the worktree from
[`worktree_setup_path.sh`](../../amplifier-bundle/tools/worktree_setup_path.sh)
(no path uses `repo_path`; a rejected path is `FAILED_INVALID_INPUT`), and
`no_merge` through `no_merge_flag`, so a bare `NO_MERGE` can only switch
merging off (see the [`NO_MERGE` override](auto-drive-to-merge.md#no_merge-override)).

| Input | Type | Description |
| --- | --- | --- |
| `WORKFLOW_CLASSIFICATION` | enum string | Workflow class such as `Development`, `Default`, `Investigation`, or `Ops`. |
| `RECIPE_NAME` | string | Current recipe name. Used for diagnostics and protected-workflow detection. |
| `REPO_PATH` | path | Repository root used for Git evidence. |
| `worktree_setup.worktree_path` | path | Preferred workflow checkout when step 04 created or reused a worktree. |
| `worktree_setup.branch_name` | git ref name | Expected branch for development completion. |
| `BASE_REF` | git ref name | Intended comparison base. |
| `RECIPE_VAR_pr_url`, then `pr_publish_result.pr_url` | URL string | Pull request URL, when publication happened or recovery is in progress. |
| `RECIPE_VAR_pr_number`, then `pr_publish_result.pr_number` | positive integer string | Pull request number, when available. |
| `RECIPE_VAR_issue_number` | string | Tracking issue, used to scope the PR. |
| `REMOTE_HOST_TYPE` | `github`, `azdo`, or `other` | Remote provider. Defaults to `other`. The [branch PR lookup](#branch-pr-lookup) runs only for `github`. |
| `pr_publish_result.state` | enum string | State reported by the publish step, such as `FOLLOWUP_CREATED`, `EXISTING_OPEN_PR`, or `unknown`. |
| `RECIPE_VAR_allow_no_op` | boolean string | Explicit opt-out for eligible no-op paths. Defaults to `false`. |
| `implementation_terminal_evidence.implementation_completed` | boolean string | Direct implementation evidence. |
| `verification_terminal_evidence.verification_completed` | boolean string | Direct verification evidence. |
| `publish_terminal_evidence.publish_state_reached` | boolean string | Direct publish/finalize evidence. |
| `implementation_terminal_evidence.terminal_no_op`, then `verification_terminal_evidence.terminal_no_op` | boolean string | Direct no-op evidence. |
| `terminal_state.terminal_failure` | boolean string | Direct failure evidence. |
| `terminal_state.terminal_state` | enum string | Existing state from a publish/finalize step. |
| `terminal_state.terminal_reason` | string | Existing reason from a publish/finalize step. |
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

## Branch PR Lookup

Issue #1596. A run can open its PR from an agent step instead of the publish
step. When that happens `pr_publish_result.pr_url` is empty. Before
`workflow_final_status.sh` declares `FAILED_MISSING_TERMINAL_EVIDENCE`, it asks
GitHub once, read-only, whether the **run's branch** already has an open PR in
the origin repository:

```bash
gh pr list --repo "$repo" --head "$branch" --state open --json url,number,isCrossRepository,headRefOid
```

A PR counts only when its head commit is the worktree's `HEAD` (issue #1679), so
a PR that does not hold exactly this run's work is ignored.

**Which branch.** The branch is read from the run's worktree
(`worktree_setup.worktree_path`).
`REPO_PATH` is the main checkout, and its branch is `main`, a detached `HEAD`,
or something unrelated to the run, so the lookup never uses it.
`step-22b-final-status` enters the worktree before calling the helper, as
`step-21-pr-ready` does, so the scoped PR check also sees the run's branch and
HEAD.

When no worktree path is given, the helper falls back to the current
directory's branch, with one exception: if `REPO_PATH` is set and the current
directory is inside the same checkout (same `git rev-parse --show-toplevel`),
the lookup does not run. That covers step-22b's own fallback, which `cd`s into
`REPO_PATH` when the worktree path is unset or gone, and callers that do not
`cd` at all, such as `strict-terminal-state`. The cwd fallback is kept on
purpose for single-checkout callers that set neither `REPO_PATH` nor a worktree
path; there the current directory is the run's checkout.

**When it runs.** Only on the missing-evidence path. That means:

- GitHub host. When `REMOTE_HOST_TYPE` is unset because the run recorded no
  workflow-prep output, the host is taken from the worktree's `origin`: only a
  `github.com` origin is looked up (issue #1679). An explicit non-GitHub
  `REMOTE_HOST_TYPE` still means no lookup.
- no PR URL
- no success publish state
- no failure, no-op, or invalid evidence
- implementation and verification evidence not both present

The guard list lives in the comment above `resolve_open_pr_for_current_branch`
in `amplifier-bundle/tools/workflow_final_status.sh`. The code is the
authority; this page does not repeat the list.

**Fail closed.** The lookup changes nothing, and the run fails exactly as it
did before, when any of the following is true:

- the worktree path is set but missing
- the worktree path is unset and the current directory is the `REPO_PATH`
  checkout
- the branch is detached, is the base branch, or differs from
  `worktree_setup.branch_name`
- `gh` errors
- the JSON is malformed
- only fork PRs exist
- no same-repository PR has the worktree's `HEAD` as its head commit
- the URL and number do not agree with each other or with the origin repository

Failures print one fixed-phrase line on `stderr`,
`WARNING: branch PR lookup failed (#1596): <reason>`. When the lookup is skipped
because the worktree or branch cannot be established, it prints
`INFO: branch PR lookup skipped (#1679): <reason>` instead, so a
`FAILED_MISSING_TERMINAL_EVIDENCE` result says which check stopped it. The
lookup never writes to `stdout`.

**Effect.** A validated PR sets `PR_URL`, `PR_NUMBER` and
`PUBLISH_STATE=EXISTING_OPEN_PR`. It is evidence, not proof:
`validate_final_pr_scope` and the
[#1268 adjudication](#scoped-pr-signal-and-agentic-adjudication) still decide
the exit status. Read the exit status and `terminal_verdict`, not
`terminal_success` alone.

`tests/issue_1596_final_status_branch_pr_lookup.sh` is the executable
specification. Section 15 runs step-22b's own command from
`workflow-finalize.yaml` with three layouts for `REPO_PATH`:

- on `main`
- detached
- on an unrelated branch that has its own open PR

In each, the run's branch lives in a separate `git worktree`. Section 15 also
runs step-22b with the worktree path set-but-missing and unset; in both, an
open PR on `REPO_PATH`'s unrelated branch is never looked up. Section 17
(issue #1679) runs step-22b with no recorded outputs at all, not even
`REMOTE_HOST_TYPE`: an open PR at the worktree's `HEAD` resolves, a PR at
another commit does not, and with no PR the run still fails.

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

The finalizer produces a human-readable narrative artifact only. Its output is
never parsed for control flow. If the finalizer step fails, the deterministic
classifier records a reporting failure: when implementation and verification
evidence is present, the terminal state is `FAILED_REPORTING` and durable
evidence is preserved; otherwise the classifier falls through to the applicable
evidence-based failure state rather than to a success-shaped shell guess.

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

## Scoped-PR Signal and Agentic Adjudication

Issue #1268. `workflow_pr_scope.sh` answers a genuinely semantic question — *is
this the PR this workflow was supposed to produce?* — by string-matching
`headRefName`, `baseRefName`, `headRefOid`, `isCrossRepository`, an expected
title prefix, and `created_after`. On 2026-08-21 that match missed and
`step-22b-final-status` exited non-zero **after** the run's PR had been created,
reviewed, quality-audited and merged. The run was reported failed, and the two
follow-up PRs it had opened were abandoned; one went `DIRTY` as `main` moved
underneath it.

The scope match is now a **signal**, not the arbiter.

### A PR behind local HEAD is unpushed work

Issue #1467. An agent step can open the PR early, and a later step can commit
without pushing (the pre-commit phase's fixes, the step-20c quality audit). The
PR's `headRefOid` is then an ancestor of local `HEAD`: the branch is ahead, not
stale. While the PR is `OPEN`:

- the terminal-state probe accepts it, prints
  `INFO: ... commit(s) ahead of open PR head ...; the next push step pushes them (#1467)`,
  and skips the failing-checks gate, because those checks ran on the superseded
  head (often the very commit the unpushed fix repairs). `step-15-commit-push`,
  or `step-20b-push-cleanup` in finalize, then pushes the commits;
- the probe passes `--allow-head-behind` to `workflow_pr_scope.sh`, which then
  accepts a named PR that is `OPEN` and whose head is an ancestor of local
  `HEAD`; every other discriminator stays a hard filter. End-of-run callers
  (`workflow_final_status.sh`, `workflow_pr_ready.sh`) do not pass it: after
  the last push, a PR behind `HEAD` lacks the run's work and must not count.

A merged or closed PR, a head that is not an ancestor of local `HEAD`, or a
remote branch rewound behind any value this checkout's `origin/<branch>` held
(its current value or its last 50 reflog entries, so a rewind that was already
fetched is caught too), still fails as
`stale PR metadata` or `no_scoped_pr`. Merged-with-unpushed-commits must not be
reported as `MERGED`, because the local commits would be lost.

In workflow-finalize, `step-20c-quality-audit` runs before `step-20a-artifact-guard`
and `step-20b-push-cleanup`, so the audit's commits are checked and pushed
before `step-21-pr-ready` validates the PR.

### The three-verdict contract

`amplifier-bundle/tools/workflow_terminal_evidence.sh` has two modes:

| Mode | Purpose |
| --- | --- |
| `collect` (and its recipe wrapper `collect-for-step`) | Gathers the run's actual artifacts: every PR ever opened from the head branch in **any** state (`--state all`), the explicitly published PR, every currently open PR, and whether the head commit's work is already contained in the base branch (ancestry **or** an empty diff, so a squash-merge counts). Always emits parseable JSON. |
| `adjudicate` | Renders `SUCCESS`, `UNCERTAIN`, or `FAILED` from that evidence plus an optional agent verdict. |

| Verdict | Meaning | Final status |
| --- | --- | --- |
| `SUCCESS` | A merged PR exists, the work is already in base, or a live open PR carries it. | exit 0 |
| `UNCERTAIN` | The evidence could not be read. Nothing is claimed; every outstanding PR is enumerated and handed back to the goal-seeking loop. | exit 0, and success is **not** reported |
| `FAILED` | The evidence **was** readable and shows the work did not land and no PR carries it. | exit non-zero |

A merged PR is not in the open-PR list. Treating that absence as failure was the
precise bug; `--state all` is why the collector cannot repeat it. Several PRs
from one run — a main PR plus follow-ups — is a normal outcome, not an anomaly.

### Why this cannot become a rubber stamp

Judgement is admitted through a one-way lattice:

- **A hard negative is never lifted.** When readable evidence shows no merged
  PR, no open PR, and no work in base, the verdict is `FAILED` and the agent
  verdict is not consulted at all (`verdict_source: deterministic_hard_negative`).
- **An asserted `SUCCESS` needs an artifact.** It is honoured only when the
  collected evidence carries at least one positive fact — a merged PR, an open
  PR, or work in base. Otherwise the verdict is refused down to `UNCERTAIN`
  (`verdict_source: agent_success_refused_no_artifact`).
- **Downgrades are always honoured.** An agent verdict of `FAILED` fails the run
  even against mechanically positive evidence. A false alarm costs a re-check;
  a silently orphaned run costs live PRs.
- **Verdict tokens match exactly, never as substrings.** `NOT_MERGED` contains
  `MERGED` and `UNSUCCESSFUL` contains `SUCCESS`; a `contains` test would read
  both as approval. Unrecognised tokens, including the empty string, normalise
  to `UNCERTAIN`, which lifts nothing on its own.

The agent's prose narrative never crosses into control flow. A deterministic
`terminal-adjudication` step extracts a single `TERMINAL_VERDICT:` token from
it, and the shell adjudicator then re-checks that token against the evidence.

### Where it runs

`workflow-finalize.yaml` calls the `workflow-terminal-adjudication` sub-recipe
as `terminal-adjudication-gate`, immediately before `step-22b-final-status` —
the incident's own path. The sub-recipe is three steps:

1. `collect-terminal-adjudication-evidence` — deterministic, fail-soft
   (`workflow_terminal_evidence.sh collect-for-step`). A missing collector
   degrades to `UNCERTAIN` rather than aborting, because a bookkeeping helper
   vetoing a successful run *is* issue #1268.
2. `terminal-state-adjudicator` — the agentic step, conditioned on
   `mechanical_verdict != 'SUCCESS'`, so the healthy path costs nothing extra.
3. `terminal-adjudication` — deterministic token extraction
   (`workflow_terminal_evidence.sh verdict-token`) into
   `terminal_adjudication.agent_verdict`.

`workflow_final_status.sh` reads that verdict with
`rv_field terminal_adjudication agent_verdict` (the `agent_verdict` field of
the `terminal_adjudication` step output), and adjudicates from the
collected evidence whenever the scope match misses — including on the
`strict-terminal-state` path, which reaches the same helper without an agent
verdict and so gets the deterministic half of the contract. Outstanding open
PRs are printed as `outstanding_pr=#N …` lines **whatever the verdict** — the
incident's `no_scoped_pr` message named neither the PR that had merged nor the
two left dangling.

### Audit: other gates that can veto a successful run

Every bash gate reachable after publish was reviewed against the same rule — a
bookkeeping step must not turn a successful run into a failed one:

- `step-22b-final-status` / `strict-terminal-state` — **fixed here.** The scope
  miss no longer exits non-zero on its own.
- `step-20b-push-cleanup` (missing git repo, detached HEAD) — fails closed, and
  correctly so: it runs *before* publish, so there is no landed work to discard.
- `step-21-pr-ready` closed-unmerged exit — a readable negative about the PR
  itself, not a string comparison. Retained.
- `step-20a-artifact-guard`, `step-17a-testing-evidence-gate`,
  `implementation-terminal-evidence` — already degraded to visible warnings
  under issue #962's no-discard policy; see
  [Fatal allowlist and degrade policy](#no-silent-relaxation).
- Helper-resolution ladders — the post-publish `exit 2` sites are genuine
  "amplihack is not installed" faults, kept fail-visible by #962; the new
  collector deliberately opts out and degrades instead.

The remaining `exit 1` sites in `workflow-publish.yaml` and
`workflow-pr-review.yaml` all run before a PR exists, so none of them can
orphan a live artifact.


## Security Invariants

- Structured typed markers and deterministic evidence, not free-form agent
  prose, determine terminal success.
- The agentic finalizer narrative is treated as untrusted, opaque text. It is
  never parsed, evaluated, or interpolated to drive control flow, so adversarial
  tokens such as `terminal_state: MERGED` cannot flip a decision.
- Protected workflow classifications are validated against a small known set.
- Missing, empty, unknown, malformed, and contradictory evidence fails closed.
- `terminal_failure=true` overrides all success-looking markers.
- Terminal-state adjudication (issue #1268) admits judgement through a one-way
  lattice: a readable hard negative is never lifted, an asserted `SUCCESS` is
  honoured only when the collected evidence carries a positive artifact, and
  verdict tokens are matched as exact tokens so `NOT_MERGED` and `UNSUCCESSFUL`
  can never read as approval.
- No-op states require explicit state and reason text.
- Git and PR evidence is read only from the workflow checkout or configured
  repository path.
- Branch names, PR numbers, and PR URLs are validated before use.
- The branch PR lookup (issue #1596) uses ambient `gh` auth and the read-only
  `pr list` call only. It ignores fork PRs, accepts only a URL on the validated
  origin repository, never logs the raw `remote.origin.url`, and fails closed
  on any `gh`, JSON, or validation error.
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
