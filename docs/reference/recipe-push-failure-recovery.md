---
title: Recipe Push Failure Recovery Reference
last_updated: 2026-11-12
review_schedule: quarterly
owner: workflow-team
---

# Recipe Push Failure Recovery Reference

When one of the three push steps in `default-workflow` fails, the run no longer
ends at once. A recovery agent works out whether the change or the environment
caused the failure. For an environment cause it makes one push from a clean
environment. A bash gate then checks the result itself. The run ends only when
the cause is in the change, or when the gate cannot confirm the push
(issue #2471, first slice of #2472).

The push scripts are unchanged. Recovery is built from features the recipe
runner already has: `continue_on_error`, step `condition`s, `type: "recipe"`
sub-recipe calls and `parse_json` outputs. There is no new runner mechanism and
no new step field.

This page builds on the git hook repair loop described in
[Recipe Commit Hook Repair](./recipe-commit-hook-repair.md). That loop fixes
a hook rejection that the change caused. This page covers what happens after
a push step has failed for any reason.

## Contents

- [Where it applies](#where-it-applies)
- [How a failed push is handled](#how-a-failed-push-is-handled)
- [When recovery runs](#when-recovery-runs)
- [The `push-failure-recovery` sub-recipe](#the-push-failure-recovery-sub-recipe)
- [Calling the sub-recipe](#calling-the-sub-recipe)
- [The recovery agent](#the-recovery-agent)
- [Agent output contract](#agent-output-contract)
- [Causes: `CHANGE`, `ENVIRONMENT`, `NONE`](#causes-change-environment-none)
- [The gate](#the-gate)
- [Gate output](#gate-output)
- [Writing back `commit_result`](#writing-back-commit_result)
- [Stale values from earlier steps or runs](#stale-values-from-earlier-steps-or-runs)
- [Example run record](#example-run-record)
- [Step budget](#step-budget)
- [Security limits](#security-limits)
- [Residual risks](#residual-risks)
- [Out of scope](#out-of-scope)
- [Tests](#tests)
- [Related references](#related-references)

## Where it applies

| Recipe | Push step | Recovery call step | Output key read by recovery |
|---|---|---|---|
| `workflow-publish.yaml` | `step-15-commit-push` | `step-15-push-recovery` | `commit_result` |
| `workflow-pr-review.yaml` | `step-18c-push-feedback-changes` | `step-18c-push-recovery` | `feedback_push_result` |
| `workflow-finalize.yaml` | `step-20b-push-cleanup` | `step-20b-push-recovery` | `cleanup_push_result` |

Each push step has one change: `continue_on_error: true`. Its `command:` text
is byte-for-byte the same as before, so the tests that pull commands out of the
YAML by step id still see the same script.

Each phase recipe declares its push step's output key as an empty string in
its `context:` map (`commit_result: ""`, `feedback_push_result: ""`,
`cleanup_push_result: ""`).

When a bash step exits non-zero, runner rev `4d5187e` keeps only its stderr,
in the step error (`Command failed (exit N): <stderr>`). It throws away
stdout and does not set the step's output key, even with `continue_on_error`.
step-15 traps `EXIT` and prints a `commit_result` object on every exit path,
including `hook-rejected-*`, `diverged-upstream`, the #1762 branch refusal
and `push-failed`. On a non-zero exit that object is lost, and
`commit_result` keeps the empty value the phase recipe declared. Only on exit
0 does the runner store step-15's object.

The recovery condition is correct whichever way the runner behaves. A stored
failure object has `pushed: "false"`, and the empty value has no `pushed`
field, so `commit_result.pushed != 'true'` holds in both cases. A later
runner that kept stdout on failure would not change when recovery runs.

## How a failed push is handled

```text
push step (continue_on_error: true)
   │ succeeded ──────────────────────────────► recovery skipped; run as before
   │ failed or not pushed
   ▼
<step>-push-recovery  (type: recipe → push-failure-recovery)
   ├─ push_recovery          agent: judge cause, at most one clean push
   ├─ push_recovery_result   bash gate: check independently, fail closed
   └─ push_recovery_commit   bash: write commit_result back (step-15 only)
   │ gate exit 0 ────────────────────────────► run continues
   │ gate exit 1 ────────────────────────────► run ends with the reason
```

When the push step succeeds, the recovery step is skipped and the run behaves
exactly as it did before this change.

## When recovery runs

Each recovery call step's condition contains its push step's condition word
for word, followed by a test that the push did not happen:

| Recovery call step | Condition |
|---|---|
| `step-15-push-recovery` | `goal_already_met != 'true' && terminal_state.terminal_success != 'true' && terminal_state.should_publish == 'true' && commit_result.pushed != 'true' && commit_result.reason != 'no-upstream'` |
| `step-18c-push-recovery` | `terminal_state.terminal_success != 'true' && 'Changes Pushed' not in feedback_push_result` |
| `step-20b-push-recovery` | `terminal_state.terminal_success != 'true' && terminal_state.should_finalize == 'true' && 'Cleanup Complete' not in cleanup_push_result` |

The repeated push-step condition stops recovery from running after a push step
that was skipped on purpose.

`no-upstream` is excluded on step-15 because it is a reported outcome, not a
failure.

step-20b's script also has an intended skip path (`skip()`), which exits 0
without printing `Cleanup Complete`, so recovery runs after it. The agent
finds no linked worktree to push from, returns `NONE`, and the gate exits 0
with `pushed: "false"` and reason `no-worktree`. See [Causes](#causes-change-environment-none).

A stale `commit_result` from a parent run cannot make recovery skip. See
[Stale values](#stale-values-from-earlier-steps-or-runs).

## The `push-failure-recovery` sub-recipe

File: `amplifier-bundle/recipes/push-failure-recovery.yaml`. It is a leaf
sub-recipe: it calls no other recipe.

| Field | Value |
|---|---|
| `recursion.max_depth` | `6` |
| `recursion.max_total_steps` | `10` |
| Steps | `push_recovery` (agent), `push_recovery_result` (bash), `push_recovery_commit` (bash) |
| `continue_on_error` | none of its steps sets it, so a gate failure ends the run |

### Context

| Key | Meaning | Set by |
|---|---|---|
| `push_step` | id of the push step that failed, e.g. `step-15-commit-push` | caller |
| `push_output_key` | that step's output key: `commit_result`, `feedback_push_result` or `cleanup_push_result` | caller |
| `worktree_setup` | the run's worktree record; `branch_name` is the gate's first source for the branch | inherited |
| `base_branch` | the run's base branch; never pushed to | inherited |
| `forbidden_base_branches` | extra protected branch names, separated by commas or whitespace | inherited |
| `issue_number` | the run's issue number; used only in the record | inherited |
| `push_recovery` | the agent's JSON output; reset to `""` by the call step | `push_recovery` step |
| `push_recovery_result` | the gate's JSON output; reset to `""` by the call step | `push_recovery_result` step |

When the sub-recipe returns successfully, the runner merges its context back
into the caller's context (runner rev `4d5187e`). That is how
`commit_result` reaches step-16.

## Calling the sub-recipe

Each phase recipe adds one call step directly after its push step. In
`workflow-publish.yaml`:

```yaml
  - id: "step-15-commit-push"
    type: "bash"
    continue_on_error: true
    condition: "goal_already_met != 'true' && terminal_state.terminal_success != 'true' && terminal_state.should_publish == 'true'"
    command: |
      # unchanged
    output: "commit_result"
    parse_json: true

  - id: "step-15-push-recovery"
    type: "recipe"
    recipe: "push-failure-recovery"
    condition: "goal_already_met != 'true' && terminal_state.terminal_success != 'true' && terminal_state.should_publish == 'true' && commit_result.pushed != 'true' && commit_result.reason != 'no-upstream'"
    context: { push_step: "step-15-commit-push", push_output_key: "commit_result", push_recovery: "", push_recovery_result: "" }
```

step-18c and step-20b follow the same pattern with their own `push_step`,
`push_output_key` and condition. Every call step resets `push_recovery` and
`push_recovery_result` to `""`. See
[Stale values](#stale-values-from-earlier-steps-or-runs).

`default-workflow.yaml` gets one change to go with this. Its call steps for
the three phase recipes reset each push output key: `workflow-publish` gets
`context: { commit_result: "" }`, `workflow-pr-review` gets
`context: { feedback_push_result: "" }` and `workflow-finalize` gets
`context: { cleanup_push_result: "" }`.

## The recovery agent

Step `push_recovery`, agent `amplihack:pre-commit-diagnostic`, `output: "push_recovery"`,
`parse_json: true`.

The prompt includes only two placeholders, `{{push_step}}` and
`{{push_output_key}}`. Nothing the agent reads is put back into a prompt or a
command.

The prompt says that this is a leaf step that must do the task itself and must
not start another workflow, recipe or orchestrator (the
[#1333 guard](../../crates/amplihack-cli/tests/issue_1333_leaf_steps_do_not_orchestrate.rs)).

### What the agent does

1. Works only in the run's linked task worktree. It records the output of
   `git status --porcelain` before doing anything else, as `evidence[0]`.
2. Collects evidence:
   - `git status`, `git rev-list @{u}..HEAD` and the remote branch ref;
   - step-15's existing hook-repair evidence files in the git directory, if any
     (see [Evidence](./recipe-commit-hook-repair.md#evidence));
   - the **names**, never the values, of the `RECIPE_VAR_*` and `WORKFLOW_*`
     variables in its own environment, collected with `compgen -e`.
3. Works out why the push step failed from that evidence, not from the push
   step's output. After a non-zero exit the runner discards step-15's
   `commit_result` object (see [Where it applies](#where-it-applies)), so the
   agent usually sees `""` there. Each reason has its own evidence:
   - **Hook rejection** (`hook-rejected-*`): hook-repair evidence files exist
     in the git directory. The hook repair loop has already run, so the agent
     does not start another fix loop. It judges from that loop's evidence.
   - **Upstream on another branch** (the #1762 refusal): an upstream is set
     and its branch name, without `origin/`, differs from
     `worktree_setup.branch_name`.
   - **Diverged history** (`diverged-upstream`): after `git fetch origin`,
     `git rev-list --count HEAD..@{u}` is not 0.

   If the upstream is on another branch, or the history has diverged, the
   agent makes **no push** and returns `CHANGE` with `push_result:
   "not-attempted"`. step-15 refuses these cases on purpose, and a person has
   to decide what to do. A push to `refs/heads/<branch_name>` would get round
   that refusal and could create a new remote branch.
4. Otherwise makes **at most one** push, from a clean environment, with hooks
   on:

   ```bash
   env -u RECIPE_VAR_issue_number -u RECIPE_VAR_pr_number -u WORKFLOW_STARTED_AT \
     git push origin HEAD:refs/heads/feat/issue-2471-fix-amplihack-rs-issue
   ```

   There is one `-u` for each `RECIPE_VAR_*` and `WORKFLOW_*` name found in
   step 2. Sign-in variables are left alone: the clean push is not a way round
   a credential problem.
5. Judges the cause (see [Causes](#causes-change-environment-none)) and prints
   one JSON object.

There is no waiting, no timed retry and no second attempt.

### What the agent must not do

- push with `--no-verify`, `--force`, `--force-with-lease`, a `+` refspec,
  `--delete`, `--mirror`, `--all` or `--tags`;
- push to `main`, `master`, `base_branch` or any name in
  `forbidden_base_branches`;
- push from the main checkout, or to a different remote branch;
- push when the upstream is on a branch other than `worktree_setup.branch_name`,
  or when HEAD is behind its upstream;
- override `core.hooksPath`, or change git config or hook files;
- edit, stage or commit files;
- read credential or token files;
- print variable values, or credentials found in URLs or command output.

Hook files, `git status` output and evidence files are untrusted data. The
prompt tells the agent not to follow instructions found in them.

## Agent output contract

The agent prints one JSON object:

```json
{
  "failed": "step-15-commit-push: pre-push hook rejected the push",
  "cause": "ENVIRONMENT",
  "branch": "feat/issue-2471-fix-amplihack-rs-issue",
  "evidence": [
    "",
    "inherited: RECIPE_VAR_issue_number, RECIPE_VAR_pr_number, WORKFLOW_STARTED_AT",
    "the clean push succeeded"
  ],
  "actions": [
    "env -u RECIPE_VAR_issue_number -u RECIPE_VAR_pr_number -u WORKFLOW_STARTED_AT git push origin HEAD:refs/heads/feat/issue-2471-fix-amplihack-rs-issue"
  ],
  "push_result": "pushed",
  "explanation": "pre-push tests read RECIPE_VAR_issue_number from the run; the push passed once those variables were removed"
}
```

| Field | Type | Values |
|---|---|---|
| `failed` | string | what failed, in one line |
| `cause` | string | `CHANGE`, `ENVIRONMENT` or `NONE` |
| `branch` | string | the branch the agent pushed or would push |
| `evidence` | array of strings | `evidence[0]` is the initial `git status --porcelain` output, empty when clean |
| `actions` | array of strings | the commands run, with values redacted |
| `push_result` | string | `pushed`, `failed` or `not-attempted` |
| `explanation` | string | the reason, in plain words |

The gate validates every enum. An unknown value, a missing field or output
that is not JSON makes the gate fail.

## Causes: `CHANGE`, `ENVIRONMENT`, `NONE`

| Cause | When the agent picks it | What the gate does |
|---|---|---|
| `ENVIRONMENT`, clean push succeeded | the push passes once `RECIPE_VAR_*` and `WORKFLOW_*` are removed | checks the remote ref equals HEAD; exit 0 with `pushed: "true"` |
| `ENVIRONMENT`, clean push failed | sign-in failure, GitHub call limit, missing tool | the ref check fails; exit 1, recording the environment cause |
| `CHANGE` | the clean push fails on a test the change broke, a real conflict, or a lint failure in changed files; or the upstream is on another branch, or the history has diverged, so no push is made | reason `cause-in-change`; exit 1 |
| `NONE`, branch already at HEAD | nothing needed pushing | checks the remote ref equals HEAD; exit 0 with `pushed: "true"`, reason `already-at-head` |
| `NONE`, no linked worktree | step-20b `skip()`, or step-18c with no worktree; never valid on the `commit_result` path | exit 0 with `pushed: "false"`, reason `no-worktree`; exit 1 on the `commit_result` path |

`NONE` never produces `pushed: "true"` without a checked ref. On the
`commit_result` path, a checked ref gives `pushed: "true"`. step-16 runs only
when `commit_result.pushed == 'true'`, so `NONE` with `pushed: "false"` there
would skip PR creation without failing the run. That is why the gate exits 1
for `NONE` with no worktree on the `commit_result` path.

## The gate

Step `push_recovery_result`, `type: "bash"`, `output: "push_recovery_result"`,
`parse_json: true`. It runs under `set -euo pipefail`. It does not trust
anything the agent says it did.

Rules, in order. The first rule that fails ends the gate with exit 1 and the
reason shown:

1. **Inputs.** Reads values only from `RECIPE_VAR_*`, through `rv_field` or
   `jq`. The command contains no `{{...}}` placeholder, so agent text is never
   parsed as shell.
2. **Agent JSON** (`invalid-agent-output`). `jq -e` checks that the output is
   an object with every field, and that `cause` and `push_result` are known
   values.
3. **Helpers.** Finds `worktree_setup_path.sh` and `verify_pushed_ref.sh` with
   the same search step-15 uses: the directory of
   `WORKFLOW_RUNTIME_ARTIFACT_HELPER` when it is set, then `AMPLIHACK_HOME`,
   `REPO_PATH`, `~/.copilot` and `~/.amplihack`, each under
   `amplifier-bundle/tools/`. Unlike step-15, it does not search the task
   worktree (`$(pwd)`). A script committed on the branch under test must not
   be able to replace the check.
4. **Worktree** (`not-linked-worktree`). Resolves the task worktree from
   `worktree_setup` with `worktree_setup_path.sh`. The directory must be a
   linked worktree: `git rev-parse --git-dir` differs from
   `--git-common-dir`. If none is found and the cause is `NONE` on a
   path other than `commit_result`, the gate prints its record with
   `pushed: "false"` and reason `no-worktree`, and exits 0. It exits 1 in
   every other case where no worktree is found.
5. **Branch** (`branch-mismatch`). The gate works out the branch itself:
   `worktree_setup.branch_name`, else the worktree's upstream branch without
   `origin/`. It fails if neither exists or if the agent's `branch` differs.
6. **Upstream** (`upstream-mismatch`). If the worktree has an upstream
   (`git rev-parse --abbrev-ref @{u}` succeeds), its branch name without
   `origin/` must equal the branch from rule 5. This is the same check as
   the #1762 refusal in step-15. The gate therefore fails even when the agent
   claimed `ENVIRONMENT` and pushed to `refs/heads/<branch_name>`.
7. **Protected names** (`protected-branch`). After stripping `refs/heads/`
   and `origin/` prefixes and splitting `forbidden_base_branches` on commas
   or whitespace, the gate refuses `main`, `master`, `base_branch` and every
   listed name.
8. **Ref safety** (`unsafe-ref`). `git check-ref-format --branch` must pass,
   the name must not start with `-`, and HEAD must match `^[0-9a-f]{40}$`.
   Rules 4 to 8 all run before any remote read. Names are passed to git
   after `--` where git accepts it.
9. **No edits** (`worktree-changed`). `git status --porcelain` must match
   `evidence[0]`. If the agent changed files, the gate fails.
10. **Ancestry** (`diverged-upstream`). If an upstream is set, the gate runs
    `git fetch origin`, then `git rev-list --count HEAD..@{u}` must be 0. This
    matches step-15's #978 refusal.
11. **Cause.**
    - `CHANGE` (`cause-in-change`): exit 1.
    - `ENVIRONMENT` and `NONE` with a worktree: check that the remote ref
      equals HEAD with `verify_pushed_ref.sh origin <branch> <sha>`. If the
      helper is not found, the gate compares `git ls-remote origin --
      refs/heads/<branch>` itself. Unlike step-15, a missing helper is not a
      warning. Any mismatch or error ends the gate with `push-unverified`.

On exit 1 the gate prints one line to stderr:

```text
push-failure-recovery: cause-in-change: cause is in the change
```

The format is `push-failure-recovery: <reason>: <detail>`. The runner puts
stderr in the step error, so this line ends up in the run's failure message.
Stdout from a failed step is thrown away, so a failing gate stores no
record.

### Gate reasons

| Reason | Exit | `pushed` | When |
|---|---|---|---|
| `recovered-environment` | 0 | `"true"` | `ENVIRONMENT`, remote ref equals HEAD |
| `already-at-head` | 0 | `"true"` | `NONE`, remote ref equals HEAD |
| `no-worktree` | 0 | `"false"` | `NONE`, no linked worktree, not the `commit_result` path |
| `invalid-agent-output` | 1 | none | rule 2 |
| `not-linked-worktree` | 1 | none | rule 4, including `NONE` with no worktree on the `commit_result` path |
| `branch-mismatch` | 1 | none | rule 5 |
| `upstream-mismatch` | 1 | none | rule 6 |
| `protected-branch` | 1 | none | rule 7 |
| `unsafe-ref` | 1 | none | rule 8 |
| `worktree-changed` | 1 | none | rule 9 |
| `diverged-upstream` | 1 | none | rule 10 |
| `cause-in-change` | 1 | none | `CHANGE` |
| `push-unverified` | 1 | none | the ref check failed or could not run |
| `invalid-gate-output` | 1 | none | the write-back step found no verified gate record (`commit_result` path only) |

## Gate output

The gate prints a record only when it exits 0. It builds the record with
`jq -n --arg/--argjson`. Its own checks supply `sha`, `branch` and `reason`,
and on the `no-worktree` path `sha` is `""`. The agent's object goes,
untrusted, under `recovery`:

```json
{
  "pushed": "true",
  "sha": "3c1e9a0b5d7f2e4a6c8b0d1f3e5a7c9b1d3f5e7a",
  "branch": "feat/issue-2471-fix-amplihack-rs-issue",
  "reason": "recovered-environment",
  "recovery_attempted": "true",
  "recovery": {
    "failed": "step-15-commit-push: pre-push hook rejected the push",
    "cause": "ENVIRONMENT",
    "evidence": ["", "inherited: RECIPE_VAR_issue_number, RECIPE_VAR_pr_number, WORKFLOW_STARTED_AT", "the clean push succeeded"],
    "actions": ["env -u RECIPE_VAR_issue_number -u RECIPE_VAR_pr_number -u WORKFLOW_STARTED_AT git push origin HEAD:refs/heads/feat/issue-2471-fix-amplihack-rs-issue"],
    "push_result": "pushed",
    "explanation": "pre-push tests read RECIPE_VAR_issue_number from the run; the push passed once those variables were removed"
  }
}
```

The keys `pushed`, `sha`, `branch` and `reason` match step-15's
`commit_result` shape, so step-16 reads the record unchanged.

Before the agent's strings go into the record, the gate:

- cuts each string to 4096 characters;
- replaces `{{` and `}}`, so a later prompt cannot expand them;
- redacts credentials: URL user-info becomes `scheme://***@`, and `gh*_`,
  `github_pat_`, `Bearer` and `x-access-token` strings are masked.

The gate prints only with `printf '%s'`.

## Writing back `commit_result`

Step `push_recovery_commit` runs only when `push_output_key == 'commit_result'`.
It checks that `RECIPE_VAR_push_recovery_result` is an object with
`pushed: "true"` and `recovery_attempted: "true"` (else it fails with
`invalid-gate-output`), and prints it with `parse_json: true` under `output: "commit_result"`. When the
sub-recipe returns, the context merge passes the checked `commit_result` back to
`workflow-publish`, and step-16 continues with it.

step-18c and step-20b need no write-back. No recipe step reads
`feedback_push_result` or `cleanup_push_result` except the recovery
condition. The only other places they appear are the `output:` lines of
step-18c and step-20b. Their record stays under `push_recovery_result`.

## Stale values from earlier steps or runs

Runner rev `4d5187e` builds a recipe's starting context from the recipe's
own `context:` defaults and then copies the caller's whole context on top. On
a clash the caller wins. A call step's own `context:` map is applied last, so
it wins over both. When a sub-recipe succeeds, every key in its context is
copied back into the caller's context.

Three kinds of stale value follow from this. Each has its own guard:

| Stale value | How it could arrive | Guard |
|---|---|---|
| `RECIPE_VAR_commit_result` in the process environment, e.g. `{"pushed":"true"}` from a parent run | inherited by the runner process | The runner does not load `RECIPE_VAR_*` from its environment into the context, and conditions read only the context. Because the phase recipe declares `commit_result: ""`, every bash step is given `RECIPE_VAR_commit_result=""`, which replaces the inherited value. |
| `commit_result`, `feedback_push_result` or `cleanup_push_result` in a calling recipe's context | passed with `--set` by a parent run, or copied up from an earlier sub-recipe | The caller wins over the phase recipe's `""` default, so that default alone is not enough. The `default-workflow` call steps reset each key in their `context:` map (see [Calling the sub-recipe](#calling-the-sub-recipe)). |
| `push_recovery` or `push_recovery_result` from step-15's recovery, reaching step-18c's or step-20b's call | copied back to `workflow-publish`, then to `default-workflow`, then down to `workflow-pr-review` | Each recovery call step resets both keys to `""` in its `context:` map. Inside a call, every key is also written before it is read: the agent step and the gate have no condition, and the write-back step reads only the gate's output from the same call. |

The stale-value tests in [Tests](#tests) check all three guards against the
installed runner.

## Example run record

The lines below are illustrative. They show the order of steps and results,
not the runner's real log format, which may differ.

A run whose push step failed only because of inherited variables:

```text
step-15-commit-push      failed (continue_on_error)
step-15-push-recovery    running push-failure-recovery
  push_recovery          cause=ENVIRONMENT push_result=pushed
  push_recovery_result   pushed=true reason=recovered-environment
  push_recovery_commit   commit_result written
step-16-create-draft-pr  running
```

A run whose change broke a test ends like this:

```text
step-15-commit-push      failed (continue_on_error)
step-15-push-recovery    running push-failure-recovery
  push_recovery          cause=CHANGE push_result=failed
  push_recovery_result   push-failure-recovery: cause-in-change: cause is in the change
run failed: step push_recovery_result exited 1
```

## Step budget

Each phase recipe gains one call step. Each call runs at most three steps, so a
run adds at most nine. That is well inside `max_total_steps` (80 per phase
sub-recipe, 120 for `default-workflow`). All step ids are unique across the
sub-recipes. `workflow-pr-review.yaml` and `workflow-finalize.yaml` were
compacted (one-line `recursion:` and `context:` maps, blank lines removed) to
stay under the 400-line phase budget.

## Security limits

- The gate never takes the branch from the agent. It works out the branch
  itself, and refuses an upstream on any other branch, so recovery cannot get
  round step-15's #1762 refusal.
- The gate finds its helpers outside the task worktree, so the branch under
  test cannot replace them.
- The gate never reports `pushed: "true"` without checking the remote ref
  against HEAD itself.
- No bash command contains a `{{...}}` placeholder.
- The only push allowed is `HEAD:refs/heads/<branch>` to `origin`, with hooks
  on.
- Only variable names are collected, never values. Credential files are never
  read.
- The clean environment removes only `RECIPE_VAR_*` and `WORKFLOW_*`. Sign-in
  variables stay.
- Agent output is untrusted: it is validated, truncated, defanged and redacted
  before it is stored.

## Residual risks

- The gate runs after the agent. A rogue agent's push cannot be undone by the
  gate, so server-side branch protection is still the real defence for
  protected branches.
- An agent that force-pushes HEAD to the task branch would pass the ref check.
  This is accepted, because HEAD is the run's tested commit.
- The agent's `CHANGE` or `ENVIRONMENT` judgement can be wrong. A wrong
  `ENVIRONMENT` call still needs a push that passed hooks; a wrong `CHANGE`
  call ends the run, which is the conservative failure.

## Out of scope

- steps 21, 03 and 02b, and `loop-evidence-collector`;
- runner changes and new step fields;
- changes to the push scripts or helper behaviour;
- re-pushing commits stranded by earlier runs;
- version bumps.

## Tests

| Test | What it proves |
|---|---|
| `tests/integration/push_failure_recovery.rs` | the recovery wiring, run through `recipe-runner-rs` |
| `tests/integration/workflow_finalize_resilience.rs` | Part 1 of #2471: `workflow_pr_ready_command` removes every inherited `RECIPE_VAR_*` and `WORKFLOW_*` variable by prefix, not from a fixed list, so values from the run cannot reach `workflow_pr_ready.sh` |
| `default_workflow_decomposition_test.rs` | step inventories include `step-15-push-recovery`, `step-18c-push-recovery`, `step-20b-push-recovery`; phase line budgets and unique ids hold |
| `issue_1333_leaf_steps_do_not_orchestrate.rs` | the recovery prompt does not start another orchestration |
| `issue_978_step15_rebase_base_test`, `issue_1762_caller_branch_alias_test`, `issue_1671_hook_rejection_repair_test`, `issue_1561_step_outputs_prefer_recipe_var` | the push scripts' `command:` text is unchanged |

`push_failure_recovery.rs` builds a small recipe from the real step-15,
recovery and gate definitions, taken by id. Its fixture:

- a TempDir `HOME`, `TMPDIR` and `XDG_CONFIG_HOME`; `GIT_CONFIG_GLOBAL=/dev/null`
  and `GIT_CONFIG_NOSYSTEM=1`; sign-in and `AMPLIHACK_*` variables removed;
- a local bare remote whose `pre-receive` hook rejects a push when
  `RECIPE_VAR_issue_number` reaches it;
- a linked worktree on `feat/x`, one commit ahead;
- a fake `AMPLIHACK_LAUNCHER_BINARY` (mode 0700) written by the test. It
  refuses to run outside the TempDir, makes the clean-environment push, prints
  the agent JSON, and answers step-15's hook-repair prompt without making a
  fix.

Cases:

| Case | Expected |
|---|---|
| positive (step-15) | the script fails, recovery runs, the remote ref equals HEAD, `commit_result.pushed == "true"`, the record shows `cause=ENVIRONMENT`, names `RECIPE_VAR_issue_number` and lists the clean push |
| negative | the hook always fails and the fake agent returns `CHANGE`; the run exits non-zero with `cause-in-change`, and the remote does not move |
| upstream on another branch | the worktree's upstream is `origin/feat/other`, so step-15 refuses (#1762); a misbehaving fake agent pushes `HEAD:refs/heads/feat/x` and returns `ENVIRONMENT`; the run exits non-zero with `upstream-mismatch` and step-16 does not run |
| diverged upstream | the remote branch has a commit HEAD lacks, so step-15 refuses (#978); the fake agent returns `ENVIRONMENT`; the run exits non-zero with `diverged-upstream` |
| `NONE` on step-15 | the remote ref already equals HEAD; `commit_result.pushed == "true"` with reason `already-at-head`, and step-16 runs |
| stale environment | an inherited `RECIPE_VAR_commit_result` of `{"pushed":"true"}` does not skip recovery |
| stale caller context | a caller context with `commit_result` set to `{"pushed":"true"}` does not skip recovery, because the `default-workflow` call step resets it |
| stale recovery keys | values of `push_recovery` and `push_recovery_result` left by step-15's recovery do not reach step-18c's call |
| step-18c, step-20b | the same harness runs the other two call steps; step-20b `skip()` gives `no-worktree` with exit 0 |

Every case also checks that the workspace `git status` and the remote `main`
are unchanged, and that the record holds no variable values.

Limits:

- The fake agent tests the wiring, not the model's judgement.
- The behaviour on failure (stdout discarded, caller context wins over
  defaults, call-step `context:` wins over both) was read from runner rev
  `4d5187e`. The stale-value cases are what prove it against the installed
  runner.
- The test is skipped when `recipe-runner-rs` is not installed, as in the
  #1552 tests. CI installs the runner, so the test runs there.

## Related references

- [Recipe Commit Hook Repair](./recipe-commit-hook-repair.md): the hook repair
  loop that runs inside step-15 before recovery
- [Recipe Executor Environment](./recipe-executor-environment.md): which
  environment variables bash steps receive, and why inherited `RECIPE_VAR_*`
  can reach a hook
- [Loop-Health Evaluator Reference](./loop-health-evaluator.md): the evaluator
  the hook repair loop uses
