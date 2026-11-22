# How to Diagnose Workflow Terminal-State Failures

Use this guide when `default-workflow` or `smart-orchestrator` exits nonzero
with `FAILED_MISSING_TERMINAL_EVIDENCE`, `FAILED_INVALID_EVIDENCE` (recorded by earlier runs), or another
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
| `evidence_used` | Typed evidence keys the reported decision was made on; since #2343 `assess-finalization` makes it, not a deterministic classifier. |
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

### The PR exists but the run still failed

Since issue #1596, the final-status helper asks GitHub for an open PR on the
run's branch when the publish step reported no PR URL. It reads that branch from
the run's worktree (`worktree_setup.worktree_path`), not from `repo_path`. If
such a run still fails with `FAILED_MISSING_TERMINAL_EVIDENCE`, check the
worktree:

```bash
wt="<worktree_setup.worktree_path from the run log>"
git -C "$wt" branch --show-current
gh pr list --repo "$(gh repo view --json nameWithOwner -q .nameWithOwner)" \
  --head "$(git -C "$wt" branch --show-current)" --state open \
  --json url,number,isCrossRepository,headRefOid
git -C "$wt" rev-parse HEAD
```

| Finding | Cause | Fix |
| --- | --- | --- |
| The worktree directory no longer exists | The lookup refuses to fall back to `repo_path`'s branch. | Re-run finalize while the worktree exists, or pass `pr_url`. |
| Branch is empty, a base branch, or not `worktree_setup.branch_name` | Nothing trustworthy to look up. | Check out the run's branch in the worktree. |
| `gh pr list` returns `[]`, or only fork PRs | No same-repository open PR on the branch. | Publish the PR from the origin repository. |
| The PR's `headRefOid` is not the worktree's `HEAD` | The PR does not hold this run's latest work (issue #1679). | Push the worktree's commits to the PR branch. |
| `stale PR metadata: headRefOid '…' does not match local HEAD '…'` | The PR head is not an ancestor of local `HEAD` (force-push, other branch), or the PR is merged or closed with local commits on top. An open PR that is merely behind is accepted (issue #1467). | Check `git merge-base --is-ancestor <headRefOid> HEAD`; reconcile the branch, or open a follow-up PR for the extra commits. |
| `stderr` shows `INFO: branch PR lookup skipped (#1679): <reason>` | The run's worktree or branch could not be established. | Act on the reason: for example, record `worktree_setup` or keep the worktree until finalize. |
| `stderr` shows `WARNING: branch PR lookup failed (#1596): <reason>` | `gh` failed, or returned data the helper will not trust. | Run `gh auth status`; check `origin` points at the PR's repository. |
| `terminal_state=EXISTING_OPEN_PR` but exit `1` or `terminal_verdict=UNCERTAIN` | The PR was found and then failed the scope check. | See [Scoped-PR Signal and Agentic Adjudication](../reference/workflow-terminal-state.md#scoped-pr-signal-and-agentic-adjudication). |
| `REMOTE_HOST_TYPE` is set and is not `github` | The lookup is GitHub-only. An unset value falls back to the worktree's `origin` (issue #1679). | Pass `pr_url` explicitly for Azure DevOps runs. |

See [Branch PR Lookup](../reference/workflow-terminal-state.md#branch-pr-lookup)
for the full rules.

---

## Fix `FAILED_INVALID_EVIDENCE`

Finalization no longer emits this state (#2343); it is still recognised so older
run records and other producers read correctly. Finalization now classifies on
the pushed head and the open PR, see
[The Run's Real Outcome](../reference/default-workflow-agentic-finalization.md#the-runs-real-outcome-2343).
The checks below apply to a record that carries the state from an earlier run
or another producer.

The workflow produced terminal markers, but they were not trustworthy.

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
terminal_reason=deploy-preview: the workflow reads secrets.PREVIEW_TOKEN, which the repository does not define
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

`HOLLOW_SUCCESS` is unchanged by #2343: empty or inaccessible agent output means
the run may have produced nothing at all, so it stays a blocker ahead of any
success classification. See
[The Run's Real Outcome](../reference/default-workflow-agentic-finalization.md#the-runs-real-outcome-2343).

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

`BLOCKED_CI` is a terminal failure state, not an incomplete finalizer run. Since
#2966 it means one thing: a check on the PR head fails, or cannot run, for a
cause the run could not fix itself. A red CI that the run can fix is repaired
before finalization decides (diagnose, fix, push, wait, reassess, with no round
or time limit), so a run that ended `BLOCKED_CI` has already tried and has named
what it needs from a person.

Read the cause and the step from the output. The failure line is:

```text
ERROR: workflow finalization failed closed: BLOCKED_CI: <check>: <cause>; next action: <owner>: <exact step>
```

The same two sentences are `terminal_reason` and `required_next_action` in the
result, and the step-recovery hand-off record names the owner. For example:

```text
BLOCKED_CI: deploy-preview: the workflow reads secrets.PREVIEW_TOKEN, which the repository does not define
next action: repository administrator: add PREVIEW_TOKEN under Settings, Secrets and variables, Actions, then re-run deploy-preview
```

Take the step the owner is named for. Typical causes the run cannot fix are a
missing secret, a workflow run awaiting maintainer approval, a setting only a
repository or organisation owner can change, an account or billing block on
CI, and a fix that needs a change under `.github/**`, which the repair never
edits, whether or not the pull request changes it. In that last case the owner
is the code owner of `.github/` or the repository administrator, who makes the
change. To confirm the state of the checks first:

```bash
gh pr view "$PR_NUMBER" \
  --json number,state,headRefName,baseRefName,headRefOid,statusCheckRollup
gh pr checks "$PR_NUMBER" --watch
```

Then rerun finalization. Do not convert `BLOCKED_CI` into success unless the CI
evidence changes.

If a run ended on a failure that looks fixable (a test, lint, build or smoke
failure, a flaky check, a slow run, a branch behind its base), it should not have
ended `BLOCKED_CI`. Read the head SHA and the check counts in the assessment's
`reason`, and the `assessment_action` in `evidence_used`. `assessment_action=repaired_ci`
beside `outcome.checks=failing` is normal: the first is what the run found, the
second what it did. Report a fixable cause that was named as unfixable as a bug
in the assessment's judgement.

---

## Check That a Red CI Is Repaired

The QA scenario for #2966 runs the shipped `workflow-finalize` recipe against a
local bare remote, a stub `gh` whose check results depend on the pushed head,
and a stub agent launcher. Run it with the `amplihack` binary you want to test:

```bash
AMPLIHACK_2966_QA_BIN="$(command -v amplihack)" \
  bash tests/gadugi/run-issue-2966-finalize-ci-repair-scenario.sh
```

The driver prints the binary path it uses and fails if the variable does not
name an existing executable; without the variable it builds `amplihack` with
cargo first. It runs with `env -i` and an allowlist, a fresh
`HOME` per case and temporary folders under `$TMPDIR`, so it never reads a real
Claude profile or a GitHub token. Three cases run in turn:

| Case | What the stubs do | Expected result |
| --- | --- | --- |
| `red_ci_is_repaired` | Checks fail until the fix is pushed. The `step-22` stub applies the fix, commits and pushes. | At least one repair push, `EXISTING_OPEN_PR`, `terminal_success=true`, no hand-off in the step-recovery report, exit `0` (not `81`). |
| `assessment_finishes_the_repair` | Checks fail on the first head, and the `step-22` stub ends without pushing a fix, so the second point of the loop has work to do. The assessment stub applies the fix and pushes it, and the stub `gh` reports green only for the new head. | `assessment_action=repaired_ci`, `EXISTING_OPEN_PR`, `terminal_success=true`, exit `0`. |
| `unfixable_cause_is_handed_to_its_owner` | A check fails because a secret is not defined. | `BLOCKED_CI` with the check and cause as the reason, the owner and step as the next action, hand-off recorded, exit `81`. |

Every case also asserts that the stub logs are not empty, that the stub `gh`
saw no call outside its table, and that no agent prompt went unmatched by a
stub key. The table holds `auth status`, `pr view` and `pr list`, and one
write that a shipped step makes, the `pr comment` of `step-21-pr-ready`; the
stub exits `96` on anything else, such as a merge, an `api` call, or a write to
secrets, variables or workflows.

To see the report reproduced, run the first fixture against the bundle from
before the change:

```bash
AMPLIHACK_2966_QA_BIN="$(command -v amplihack)" \
  AMPLIHACK_2966_BASELINE_REF=origin/main \
  bash tests/gadugi/run-issue-2966-finalize-ci-repair-scenario.sh
```

The driver extracts that ref's `amplifier-bundle` with `git archive` into a
separate temporary folder, and prints `BASELINE_CONFIRMED` when the run ends
`BLOCKED_CI` with exit `81` and never starts the `step-22` agent. That is the
behaviour the scenario exists to catch.

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
It reads recipe values only from their `RECIPE_VAR_` names, and an evidence
field from the JSON of its step output, as the runner exports them (see the
[helper inputs](../reference/workflow-terminal-state.md#inputs)).

```bash
WORKFLOW_CLASSIFICATION=Development \
RECIPE_NAME=default-workflow \
REPO_PATH=. \
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
RECIPE_VAR_implementation_terminal_evidence='{"implementation_completed":true}' \
RECIPE_VAR_verification_terminal_evidence='{"verification_completed":true}' \
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
