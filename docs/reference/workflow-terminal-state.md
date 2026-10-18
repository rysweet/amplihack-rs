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
| 3 | Valid publish/PR/merge/no-diff state with required details, including an open PR for the current branch found by the [branch PR lookup](#branch-pr-lookup) | Succeed, because publish/finalize proves the workflow reached terminal semantics. |
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
| `REMOTE_HOST_TYPE` | `github`, `azdo`, or `other` | Remote provider. Defaults to `other`. The [branch PR lookup](#branch-pr-lookup) runs only for `github`. |
| `PR_PUBLISH_RESULT_STATE` | enum string | State reported by the publish step, such as `FOLLOWUP_CREATED`, `EXISTING_OPEN_PR`, or `unknown`. |
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

## Branch PR Lookup

Issue #1596. A development run can open its PR from an agent step instead of
from the publish step. The publish step then reports an empty
`pr_publish_result.pr_url`, so the run would close with
`FAILED_MISSING_TERMINAL_EVIDENCE` even though the PR exists. Before the helper
decides that publish evidence is missing, it asks GitHub whether the current
branch already has an open PR.

### When the lookup runs

`workflow_final_status.sh` calls `resolve_open_pr_for_current_branch` once,
after it has normalized the evidence markers and before it evaluates the
publish state. The lookup runs only when every condition holds:

| Condition | Why |
| --- | --- |
| `REMOTE_HOST_TYPE=github` | Azure DevOps and other remotes are unchanged. |
| `PR_URL` is empty after all fallbacks (`PR_URL`, `PR_PUBLISH_RESULT_PR_URL`, `RECIPE_VAR_pr_publish_result__pr_url`) | A known PR URL is already publish evidence. |
| `publish_state_reached` is not `true` | An explicit `false` or an empty value both allow the lookup. |
| The publish state is not `FOLLOWUP_CREATED`, `EXISTING_OPEN_PR`, `MERGED`, `CLOSED_OBSOLETE`, or `NO_DIFF_SUCCESS` | Success states are never re-examined. |
| No invalid boolean marker, and `terminal_failure` and `terminal_no_op` are not `true` | Failure and no-op evidence keep precedence. |
| `implementation_completed` and `verification_completed` are not both `true` | That evidence already closes the run as `IMPLEMENTED_VERIFIED`; the lookup only rescues runs that would otherwise fail for missing evidence. |
| `gh` and `jq` are on `PATH` | Without them there is nothing to ask. |
| The current directory is inside a Git work tree, and `git branch --show-current` names a branch | A detached `HEAD` has no branch to look up. |
| The branch is a valid ref name, does not start with `-`, and is not `main`, `master`, or the `origin/HEAD` target | A base branch never stands in for the run's own work. |
| `remote.origin.url` parses to an `owner/repo` matching `^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$` | The query needs a validated repository. |

The helper reads Git state from the current directory. `workflow-finalize`
step `step-22b-final-status` already runs `cd "$REPO_PATH"` first, so no recipe
change is needed.

### The query

The helper makes one read-only call through the shared retry wrapper, with
rate-limit waits turned off so a throttled API cannot stall finalization:

```bash
GH_RETRY_MAX_RL_WINDOWS=0 _gh_retry_core "final status branch PR lookup" \
  pr list --repo "$repo" --head "$branch" --state open \
  --json url,number,isCrossRepository
```

From the result it takes the first matching entry `gh` returns: one that is
not cross-repository and has a non-empty `url`. `gh pr list` does not promise
any particular order, so when a branch has more than one open PR in the origin
repository, which one is used is undefined. PRs from forks never count.

### Validation before use

A candidate is accepted only when both checks pass:

- `url` matches `^https://github\.com/<owner>/<repo>/pull/<n>$`, where
  `<owner>/<repo>` is the validated origin repository (compared
  case-insensitively).
- `number` is a positive integer equal to `<n>` from the URL.

Only then does the helper set `PR_URL`, `PR_NUMBER`, and
`PUBLISH_STATE=EXISTING_OPEN_PR`, all together. The previous publish state is
replaced whatever it was (empty, `unknown`, `create-new-pr`, and so on),
because the lookup never runs over a success state. The usual logic then sets
`publish_state_reached=true`, reports `terminal_state=EXISTING_OPEN_PR`, and
runs the PR status block, including `validate_final_pr_scope` and the
[#1268 adjudication](#scoped-pr-signal-and-agentic-adjudication), unchanged.
A PR found this way is evidence, not proof. The scope check still applies, and
it can still fail the run. See [Effect on reported states](#effect-on-reported-states).

### Output

The lookup adds nothing to `stdout`. When it finds a PR it writes one line to
`stderr`:

```text
INFO: resolved open PR https://github.com/rysweet/amplihack-rs/pull/<n> for branch feat/issue-1596-fix-issue-1596-read-it (previous publish state: 'unknown'); treating as EXISTING_OPEN_PR (#1596)
```

`<n>` is the PR number. The URL and branch in that line have already passed
validation, and the previous state is reduced to `[A-Za-z0-9_-]` characters.

When nothing is found, every input stays as it was and the helper behaves
exactly as before:

| Lookup result | `stderr` | Outcome |
| --- | --- | --- |
| `[]`, or only fork PRs | nothing extra | Existing path, e.g. `FAILED_MISSING_TERMINAL_EVIDENCE`, exit `1`. |
| `gh` fails (auth error, rate limit, network) | `WARNING: branch PR lookup failed (#1596): gh failed` | Existing path. |
| Malformed JSON or a non-array result | `WARNING: branch PR lookup failed (#1596): malformed JSON` | Existing path. |
| A URL that fails validation | `WARNING: branch PR lookup failed (#1596): URL failed validation` | Existing path. |
| A `number` that is not a positive integer or differs from the URL | `WARNING: branch PR lookup failed (#1596): PR number mismatch` | Existing path. |

The reason after the colon is always one of those four fixed phrases. It never
contains raw `gh` error output or any part of the candidate URL or number, so
a hostile PR title, URL, or error message cannot inject extra lines into
`stderr` any more than into `stdout`.

### Effect on reported states

Rows marked † report `terminal_success=true` and `terminal_state=EXISTING_OPEN_PR`,
but the final exit status still depends on the PR status block that runs
because `PR_URL` is now set:

- `validate_final_pr_scope` passes: exit `0` and the success line
  `All 23 workflow steps completed successfully.`
- The scope check misses and the [#1268 adjudication](#scoped-pr-signal-and-agentic-adjudication)
  returns `SUCCESS`: exit `0`, with an INFO line on `stderr`.
- The adjudication returns a failing verdict: exit `1`, with
  `Workflow final status failed; terminal success was not proven.` on `stderr`.
- The adjudication returns `UNCERTAIN`: exit `0`, `terminal_verdict=UNCERTAIN`
  on `stdout`, and no success line.

In the last two cases `terminal_success=true` has already been printed. Callers
must use the exit status and `terminal_verdict`, not `terminal_success` alone.

| Before the lookup | Open PR on branch | Result |
| --- | --- | --- |
| Empty `PR_URL`, no other evidence | yes | † `terminal_status=EXISTING_OPEN_PR`; exit depends on the scope check |
| Empty `PR_URL`, no other evidence | no | `terminal_state=FAILED_MISSING_TERMINAL_EVIDENCE`, exit `1` (unchanged) |
| `PR_PUBLISH_RESULT_STATE=unknown` | yes | † exit depends on the scope check |
| Implementation and verification evidence | n/a | No lookup. `IMPLEMENTED_VERIFIED`, exit `0` (unchanged). That evidence already closes the run, so the lookup is confined to the missing-evidence path. |
| `TERMINAL_STATE` (or `TERMINAL_STATE_TERMINAL_STATE`, `RECIPE_VAR_terminal_state__terminal_state`) set | yes | The override is still the reported state. The lookup changes only `PR_URL`, `PR_NUMBER`, and the publish state. |
| `TERMINAL_FAILURE=true` | yes | No lookup. Explicit failure, exit `1`. |
| `PR_URL` already set, or `PR_PUBLISH_RESULT_STATE=FOLLOWUP_CREATED` | n/a | No lookup call. |

### Example

An agent step pushed `feat/issue-1596-fix-issue-1596-read-it` and opened a PR,
and the publish step reported nothing. Run the helper from the checkout:

```bash
REMOTE_HOST_TYPE=github \
ISSUE_NUMBER=1596 \
TASK_DESCRIPTION="Resolve agent-opened branch PR in final status" \
amplifier-bundle/tools/workflow_final_status.sh
```

This uses your real `gh` auth and the real scope helper, so it queries GitHub
and checks the found PR against issue #1596. `stdout` includes these lines,
along with a `PR Status:` block from `gh pr view`:

```text
publish_state_reached=true
terminal_success=true
terminal_state=EXISTING_OPEN_PR
terminal_status=EXISTING_OPEN_PR
PR Status:
=== Task: Resolve agent-opened branch PR in final status ===
=== Issue: #1596 ===
=== PR: https://github.com/rysweet/amplihack-rs/pull/<n> ===
All 23 workflow steps completed successfully.
```

`<n>` is the number of the open PR on the branch. If the PR does not match
the issue's scope, the result follows the scope-check rules in
[Effect on reported states](#effect-on-reported-states) instead.

Run the same command on a branch with no open PR and the result is unchanged:
`terminal_state=FAILED_MISSING_TERMINAL_EVIDENCE` and exit `1`.

The regression test `tests/issue_1596_final_status_branch_pr_lookup.sh` covers
these cases with a fake `gh` and a temporary repository. It needs no
credentials, and CI runs it in the step "Final status — resolves agent-opened
branch PR (issue #1596)".

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

`workflow_final_status.sh` reads that verdict from
`WORKFLOW_TERMINAL_AGENT_VERDICT`, `TERMINAL_ADJUDICATION_AGENT_VERDICT`, or
`RECIPE_VAR_terminal_adjudication__agent_verdict`, and adjudicates from the
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
