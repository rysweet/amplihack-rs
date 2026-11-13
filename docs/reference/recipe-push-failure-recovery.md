---
title: Recipe Push Failure Recovery Reference
last_updated: 2026-11-13
review_schedule: quarterly
owner: workflow-team
---

# Recipe Push Failure Recovery Reference

When step-15 or step-18c of `default-workflow` fails to push, the run no
longer ends at once. A recovery agent works out whether the change or the
environment caused the failure. For an environment cause it makes one push
from a clean environment. A bash gate then checks the result itself. The run
ends only when the cause is in the change, or when the gate cannot confirm a
fast-forward push of HEAD (issue #2471, first slice of #2472).

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
- [The snapshot](#the-snapshot)
- [The recovery agent](#the-recovery-agent)
- [Agent output contract](#agent-output-contract)
- [Causes: `CHANGE`, `ENVIRONMENT`, `NONE`](#causes-change-environment-none)
- [The gate](#the-gate)
- [The stderr recovery record](#the-stderr-recovery-record)
- [Gate output](#gate-output)
- [Writing back `commit_result`](#writing-back-commit_result)
- [Stale values from earlier steps or runs](#stale-values-from-earlier-steps-or-runs)
- [Example run record](#example-run-record)
- [Restoring a rewritten remote branch](#restoring-a-rewritten-remote-branch)
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

Step-20b (`step-20b-push-cleanup` in `workflow-finalize.yaml`) is not covered
here. Its failures belong to the general recovery step from #2570
(`step-recovery run --essential --stop-on-handoff`, part of #2539).
`workflow-finalize.yaml` is the same as on `main`.

Each covered push step has one change: `continue_on_error: true`. Its
`command:` text is byte-for-byte the same as before, so the tests that pull
commands out of the YAML by step id still see the same script.

Each phase recipe declares its push step's output key as an empty string in
its `context:` map (`commit_result: ""`, `feedback_push_result: ""`).

step-15 traps `EXIT` and prints a `commit_result` object on every exit path,
including `hook-rejected-*`, `diverged-upstream`, the #1762 branch refusal
and `push-failed`. The runner keeps no output from a step that exits
non-zero, and step-15 exits non-zero on every path that leads to recovery
(`no-upstream` exits 0 and is excluded by the condition). So the failed
step's `reason` does not reach the recovery: `commit_result` keeps the empty
value the phase recipe declared, and the agent works from its own evidence.
step-18c prints plain text, not an object, and has no reason field either.

The recovery condition holds either way: the empty value has no `pushed`
field, so `commit_result.pushed != 'true'` is true.

## How a failed push is handled

```text
push step (continue_on_error: true)
   │ succeeded ──────────────────────────────► recovery skipped; run as before
   │ failed or not pushed
   ▼
<step>-push-recovery  (type: recipe → push-failure-recovery)
   ├─ push_recovery_snapshot bash: record HEAD, status, untracked paths,
   │                         remote branch commit, origin URL hashes
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

The repeated push-step condition stops recovery from running after a push step
that was skipped on purpose.

`no-upstream` is excluded on step-15 because it is a reported outcome, not a
failure.

Neither step has an intended skip path, so a missing linked worktree always
fails the gate with `not-linked-worktree`.

A stale `commit_result` from a parent run cannot make recovery skip. See
[Stale values](#stale-values-from-earlier-steps-or-runs).

## The `push-failure-recovery` sub-recipe

File: `amplifier-bundle/recipes/push-failure-recovery.yaml`. It is a leaf
sub-recipe: it calls no other recipe.

| Field | Value |
|---|---|
| `recursion.max_depth` | `6` |
| `recursion.max_total_steps` | `10` |
| Steps | `push_recovery_snapshot` (bash), `push_recovery` (agent), `push_recovery_result` (bash), `push_recovery_commit` (bash) |
| `continue_on_error` | none of its steps sets it, so a gate failure ends the run |

### Context

| Key | Meaning | Set by |
|---|---|---|
| `push_step` | id of the push step that failed, e.g. `step-15-commit-push` | caller |
| `push_output_key` | that step's output key: `commit_result` or `feedback_push_result` | caller |
| `worktree_setup` | the run's worktree record; `branch_name` is the gate's first source for the branch | inherited |
| `base_branch` | the run's base branch; never pushed to | inherited |
| `forbidden_base_branches` | extra protected branch names, separated by commas or whitespace | inherited |
| `issue_number` | the run's issue number; used only in the record | inherited |
| `push_recovery_snapshot` | the record of the worktree and remote before the agent runs; reset to `""` by the call step | `push_recovery_snapshot` step |
| `push_recovery` | the agent's JSON output; reset to `""` by the call step | `push_recovery` step |
| `push_recovery_result` | the gate's JSON output; reset to `""` by the call step | `push_recovery_result` step |

The sub-recipe's `context:` map does not declare `commit_result` or
`feedback_push_result`. A declaration there would replace the value the
failed push step left, and the snapshot reads that value.

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
    context: { push_step: "step-15-commit-push", push_output_key: "commit_result", push_recovery_snapshot: "", push_recovery: "", push_recovery_result: "" }
```

step-18c follows the same pattern with its own `push_step`,
`push_output_key` and condition. Every call step resets
`push_recovery_snapshot`, `push_recovery` and `push_recovery_result` to `""`.
See [Stale values](#stale-values-from-earlier-steps-or-runs).

`default-workflow.yaml` gets one change to go with this. Its call steps for
the two phase recipes reset each push output key: `workflow-publish` gets
`context: { commit_result: "" }` and `workflow-pr-review` gets
`context: { feedback_push_result: "" }`. The `workflow-finalize` call step
is unchanged.

## The snapshot

Step `push_recovery_snapshot`, `type: "bash"`, `output: "push_recovery_snapshot"`,
`parse_json: true`. It runs before the agent, so the agent cannot write it.
It records state and decides nothing. It finds the task worktree as the gate
does (rules 3 and 4) and prints one compact JSON object:

```json
{"linked": "true",
 "path": "/abs/worktree",
 "head": "3c1e9a0b5d7f2e4a6c8b0d1f3e5a7c9b1d3f5e7a",
 "status": "",
 "tracked_dirty": "false",
 "local_branch": "feat/issue-2471-fix-amplihack-rs-issue",
 "untracked": ["notes/scratch.txt"],
 "untracked_truncated": "false",
 "remote_before": "9f2b6d4e8a0c2e4f6a8b0c2d4e6f8a0b2c4d6e8f",
 "remote_before_source": "fetched",
 "origin_url_sha256": "<64 hex characters>",
 "origin_pushurl_sha256": "<64 hex characters>"}
```

| Field | How it is computed |
|---|---|
| `linked` | `"true"` when a linked worktree was found. With none, the snapshot prints `{"linked": "false"}` and exits 0; the gate then fails with `not-linked-worktree` |
| `path` | the worktree's absolute path |
| `head` | `git rev-parse --verify HEAD`, 40 hexadecimal characters |
| `status` | `git -c core.fsmonitor=false status --porcelain --untracked-files=no`. Untracked files are kept out of this field and listed in `untracked` instead |
| `tracked_dirty` | `"true"` when `status` is not empty |
| `local_branch` | `git symbolic-ref -q --short HEAD`, or empty |
| `untracked` | `git ls-files --others --exclude-standard`, sorted under `LC_ALL=C`, cut to the first 200 paths and to 16 KiB, dropping a partial last line. `ls-files` runs without `-z`, so git quotes paths that contain control characters |
| `untracked_truncated` | `"true"` when either cap cut the list |
| `remote_before` | the commit the remote branch pointed at before the agent ran, or `absent`. See [Recording the remote branch](#recording-the-remote-branch) |
| `remote_before_source` | `fetched`, `tracking-ref` or `absent` |
| `origin_url_sha256` | SHA-256 of `git remote get-url origin`. Only the hash is stored, never the URL |
| `origin_pushurl_sha256` | SHA-256 of `git remote get-url --push origin`, which equals the fetch URL when no push URL is set |

The caps keep the snapshot well under the 128 KiB limit on one environment
variable (`MAX_ARG_STRLEN`) when the runner passes it to later steps.
`status` has no cap.

The snapshot never sets `continue_on_error`, so a git error here ends the
run.

### Recording the remote branch

The snapshot makes one fetch, with no retry:

```bash
GIT_TERMINAL_PROMPT=0 GCM_INTERACTIVE=never \
  git -c core.hooksPath=/dev/null fetch --quiet origin -- \
  "+refs/heads/$BRANCH:refs/remotes/origin/$BRANCH" >/dev/null 2>&1
```

`$BRANCH` is the branch the gate will use (rule 5). It must be non-empty,
must not start with `-` and must pass `git check-ref-format --branch`;
otherwise the fetch is skipped. The leading `+` lets the tracking ref follow
a remote that was rewritten before the run.

| Result | `remote_before` | `remote_before_source` |
|---|---|---|
| the fetch succeeded and `refs/remotes/origin/$BRANCH` exists | that commit | `fetched` |
| the fetch failed and `git ls-remote --exit-code origin -- refs/heads/$BRANCH` exits 2 (no such branch) | `absent` | `absent` |
| the fetch failed and the tracking ref exists | the tracking ref's commit | `tracking-ref` |
| none of the above: the fetch failed, `ls-remote` exited with a code other than 2 (for example a credential or network fault) and there is no tracking ref | `absent` | `absent` |

The last row cannot tell an absent branch from an unreachable remote, and
records both as `absent`. Rule 10b then has nothing to compare against, so a
force-push by the agent in that case is not detected. See
[Residual risks](#residual-risks).

`ls-remote` runs with the same prompt settings and its output goes to
`/dev/null`; only its exit code is used. With prompts off, a fetch that
would ask for a password fails at once instead of waiting.

A failed fetch does not end the run. A credential or network fault is one of
the things recovery exists to repair, and such a fault often breaks the fetch
too. The gate runs the fast-forward check against whatever
`remote_before` holds and copies `remote_before_source` into its record.

## The recovery agent

Step `push_recovery`, agent `amplihack:pre-commit-diagnostic`, `output: "push_recovery"`,
`parse_json: true`.

The runner starts the agent with the inherited environment only, not with the
recipe's context as `RECIPE_VAR_*` variables, so the prompt carries the values
the agent needs as placeholders: `{{push_step}}`, `{{push_output_key}}`,
`{{worktree_setup.worktree_path}}`, `{{worktree_setup.branch_name}}`,
`{{base_branch}}` and `{{forbidden_base_branches}}`. All of them come from
the run's own context, not from command output. Nothing the agent itself prints is put back into a prompt or a
command.

The prompt says that this is a leaf step that must do the task itself and must
not start another workflow, recipe or orchestrator (the
[#1333 guard](../../crates/amplihack-cli/tests/issue_1333_leaf_steps_do_not_orchestrate.rs)).

### What the agent does

1. Works only in the run's linked task worktree.
2. Collects evidence:
   - `git status`, `git rev-list @{u}..HEAD` and the remote branch ref;
   - step-15's existing hook-repair evidence files in the git directory, if any
     (see [Evidence](./recipe-commit-hook-repair.md#evidence));
   - the **names**, never the values, of the `RECIPE_VAR_*` and `WORKFLOW_*`
     variables in its own environment, collected with `compgen -e`.
3. Works out why the push step failed from that evidence. Each reason has its
   own evidence:
   - **Hook rejection** (`hook-rejected-*`): hook-repair evidence files exist
     in the git directory. The hook repair loop has already run, so the agent
     does not start another fix loop. It judges from that loop's evidence.
   - **Upstream on another branch** (the #1762 refusal): an upstream is set
     and its branch name, without `origin/`, differs from
     `worktree_setup.branch_name`.
   - **Diverged history** (`diverged-upstream`): after `git fetch origin`,
     `git rev-list --count HEAD..@{u}` is not 0.

   If the upstream is on another branch, the history has diverged, HEAD is
   detached, the checked-out branch is protected, or tracked changes are not
   committed (the step's commit failed), the agent makes **no push** and
   returns `CHANGE` with `push_result: "not-attempted"`. step-15 refuses these
   cases on purpose, and a person has to decide what to do. A push to
   `refs/heads/<branch_name>` would get round that refusal and could create a
   new remote branch.
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
- push to `main`, `master`, `base_branch` or any
  name in `forbidden_base_branches` (both quoted in the prompt);
- push from the main checkout, or to a different remote branch;
- push when the upstream is on a branch other than `worktree_setup.branch_name`,
  or when HEAD is behind its upstream;
- change the `origin` URL or push URL;
- override `core.hooksPath`, or change git config or hook files;
- edit, stage or commit files;
- read credential or token files;
- print variable values, or credentials found in URLs or command output.

A force-push that breaks the first rule is caught by the gate (rule 10b), but
it has already reached the remote by then. See
[Restoring a rewritten remote branch](#restoring-a-rewritten-remote-branch).

### Untrusted data in the prompt

Push output, such as `remote:` lines printed by a server hook, can contain
third-party text. Hook files, `git status` output (including untracked file
names) and evidence files are untrusted in the same way. The prompt tells the agent that none of these are instructions and
that it must not follow instructions found in them.

## Agent output contract

The agent prints one JSON object:

```json
{
  "failed": "step-15-commit-push: pre-push hook rejected the push",
  "cause": "ENVIRONMENT",
  "branch": "feat/issue-2471-fix-amplihack-rs-issue",
  "evidence": [
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
| `evidence` | array of strings | the agent's findings; information only, the gate checks none of it |
| `actions` | array of strings | the commands run, with values redacted |
| `push_result` | string | `pushed`, `failed` or `not-attempted` |
| `explanation` | string | the reason, in plain words |

The gate validates every enum. An unknown value, a missing field or output
that is not JSON makes the gate fail.

## Causes: `CHANGE`, `ENVIRONMENT`, `NONE`

| Cause | When the agent picks it | What the gate does |
|---|---|---|
| `ENVIRONMENT`, clean push succeeded | the push passes once `RECIPE_VAR_*` and `WORKFLOW_*` are removed | checks fast-forward and that the remote ref equals HEAD; exit 0 with `pushed: "true"` |
| `ENVIRONMENT`, clean push failed | sign-in failure, GitHub call limit, missing tool | the ref check fails; exit 1 with `push-unverified` |
| `CHANGE` | the clean push fails on a test the change broke, a real conflict, or a lint failure in changed files; or the upstream is on another branch, or the history has diverged, so no push is made | reason `cause-in-change`; exit 1 |
| `NONE`, branch already at HEAD | nothing needed pushing | checks fast-forward and that the remote ref equals HEAD; exit 0 with `pushed: "true"`, reason `already-at-head` |
| any cause, no linked worktree | the worktree was removed or never created | exit 1 with `not-linked-worktree` |

`NONE` never produces `pushed: "true"` without a checked ref. On the
`commit_result` path, `pushed: "false"` would skip PR creation without
failing the run, and step-18c has no skip path, so its review would go on
without the feedback changes. A missing worktree therefore always fails.

## The gate

Step `push_recovery_result`, `type: "bash"`, `output: "push_recovery_result"`,
`parse_json: true`. It runs under `set -euo pipefail`. It does not trust
anything the agent says it did.

Rules, in order. The first rule that fails ends the gate with exit 1 and the
reason shown:

1. **Inputs.** Reads values only from `RECIPE_VAR_*`, through
   `jq`. The command contains no `{{...}}` placeholder, so agent text is never
   parsed as shell. `SAFE` is set to `""` at the top (see
   [The stderr recovery record](#the-stderr-recovery-record)).
2. **Agent JSON and snapshot** (`invalid-agent-output`, `push-unverified`).
   `jq -e` checks that the agent's output is an object with every field, and
   that `cause` and `push_result` are known values. Once the agent's object
   is valid, the gate defangs, redacts and cuts it, and stores the result as
   compact JSON in `SAFE`. The snapshot must be a well-formed record from the
   snapshot step. Every snapshot must have `linked` set to `"true"` or
   `"false"`. When `linked` is `"false"`, that is the only field required, so a missing worktree reaches rule 4 and
   fails there with `not-linked-worktree`. When `linked` is `"true"`, the
   snapshot must also meet these checks:
   - `remote_before` is `absent` or matches `^[0-9a-f]{40}$`;
   - `remote_before_source` is `fetched`, `tracking-ref` or `absent`;
   - `untracked` is an array of strings;
   - `origin_url_sha256` and `origin_pushurl_sha256` match `^[0-9a-f]{64}$`.

   Every snapshot value that later appears in a gate message is checked here
   first.
3. **Helpers.** Finds `worktree_setup_path.sh` and `verify_pushed_ref.sh` with
   the same search step-15 uses: the directory of
   `WORKFLOW_RUNTIME_ARTIFACT_HELPER` when it is set, then `AMPLIHACK_HOME`,
   `REPO_PATH`, `~/.copilot` and `~/.amplihack`, each under
   `amplifier-bundle/tools/`. Unlike step-15, it does not search the task
   worktree (`$(pwd)`). A script committed on the branch under test must not
   be able to replace the check.
4. **Worktree** (`not-linked-worktree`, `worktree-changed`). Resolves the
   task worktree from `worktree_setup` with `worktree_setup_path.sh`. The
   directory must be a linked worktree: `git rev-parse --git-dir` differs from
   `--git-common-dir`. Helper errors are shown, not hidden. If none is found,
   the gate fails with `not-linked-worktree`, whatever the cause. A worktree
   found by only one of the snapshot and the gate, or at a different path,
   fails with `worktree-changed`.
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
   listed name. The same list is used in rule 8b.
8. **Ref safety** (`unsafe-ref`). `git check-ref-format --branch` must pass,
   the name must not start with `-`, and HEAD (from `git rev-parse --verify`)
   must match `^[0-9a-f]{40}$`. Rules 4 to 8 all run before any remote read.
   Names are passed to git after `--` where git accepts it.
8b. **Local branch** (`unsafe-ref`, `worktree-changed`, `branch-mismatch`).
   `git symbolic-ref -q --short HEAD` must not be empty (step-18c refuses a
   detached HEAD), must equal the snapshot's `local_branch`, must not be a
   protected name (step-15 refuses those), and must equal the branch from
   rule 5 or have `origin/<branch>` as its upstream (the #1762 alias).
9. **Snapshot** (`worktree-changed`, `uncommitted-changes`). HEAD and
   `git status --porcelain --untracked-files=no` must equal the snapshot's
   `head` and `status`, so an agent that edits a tracked file, commits or
   switches branch is caught. Then the snapshot's `tracked_dirty` must be
   `"false"`. Every push step stages all tracked changes before it commits,
   so a tracked change left over means its commit did not happen
   (`uncommitted-changes`). This runs before rule 11, so `NONE` and
   `ENVIRONMENT` cannot skip it.

   The gate also lists the untracked paths again and records how many are
   new as `new_untracked_count`:

   ```bash
   LC_ALL=C comm -13 <(snapshot paths) <(current paths) | wc -l
   ```

   Only the count is recorded. The agent chooses the file names, and the
   record reaches later prompts as `commit_result`, so the names are left
   out. The count is information only: a new untracked file, such as a log
   left by a pre-push hook, never fails the gate. The record copies the
   snapshot's `untracked_truncated`; when it is `"true"`, paths that existed
   before but fell outside the snapshot's cap are counted as new, so the
   count is an upper bound.
9b. **Remote URL** (`push-unverified`). The SHA-256 of
   `git remote get-url origin` and of `git remote get-url --push origin` must
   equal the snapshot's `origin_url_sha256` and `origin_pushurl_sha256`. An
   agent that points `origin` at another remote cannot then have the gate
   verify against that remote.
10. **Ancestry** (`diverged-upstream`). If an upstream is set, the gate runs
    `git -c core.hooksPath=/dev/null fetch origin`, then
    `git rev-list --count HEAD..@{u}` must be 0. This matches step-15's #978
    refusal. If either command fails, the gate fails with `push-unverified`
    and includes git's own error message, flattened, redacted and cut like
    every other `fail` detail.
10b. **Fast-forward** (`remote-history-rewritten`, `push-unverified`). When
    `remote_before` is not `absent`, the gate runs
    `git merge-base --is-ancestor <remote_before> HEAD`:
    - exit 0: the push, if any, was a fast-forward; continue;
    - exit 1: `remote_before` is not in HEAD's history, so the remote branch
      held a commit that HEAD does not contain. Fail with
      `remote-history-rewritten`. The message names both commits and gives
      the restore command (see
      [Restoring a rewritten remote branch](#restoring-a-rewritten-remote-branch));
    - any other exit code, for example because `remote_before` is not in the
      local object store: fail with `push-unverified`.

    The gate exports `GIT_NO_REPLACE_OBJECTS=1` and `GIT_GRAFT_FILE=/dev/null`,
    so a replace ref or graft written by the agent cannot make
    `remote_before` look like an ancestor of HEAD.

    When `remote_before` is `absent`, there was no remote branch to
    overwrite, and the rule passes. Rule 10b runs before rule 11, so a
    force-push followed by a `CHANGE` verdict is still reported as
    `remote-history-rewritten`.
11. **Cause.**
    - `CHANGE` (`cause-in-change`): exit 1.
    - `ENVIRONMENT` and `NONE`: check that the remote ref equals HEAD with
      `verify_pushed_ref.sh origin <branch> <sha>`. There is no inline
      fallback: unlike step-15, a missing helper is not a warning but a
      `push-unverified` failure. Any mismatch or error ends the gate with
      `push-unverified`.

On exit 1 the gate prints one line to stderr:

```text
push-failure-recovery: cause-in-change: cause is in the change
```

The format is `push-failure-recovery: <reason>: <detail>`. The runner puts
stderr in the step error, so this line ends up in the run's failure message.
When `SAFE` is set, a second line follows; see
[The stderr recovery record](#the-stderr-recovery-record).

A rule 10b failure looks like this:

```text
push-failure-recovery: remote-history-rewritten: step-15-commit-push (ENVIRONMENT): origin/feat/issue-2471-fix-amplihack-rs-issue was 9f2b6d4e8a0c2e4f6a8b0c2d4e6f8a0b2c4d6e8f before the agent ran (remote_before_source fetched), which is not an ancestor of HEAD 3c1e9a0b5d7f2e4a6c8b0d1f3e5a7c9b1d3f5e7a; the remote history was rewritten and the gate undid nothing. To restore: git push --force-with-lease=refs/heads/feat/issue-2471-fix-amplihack-rs-issue:3c1e9a0b5d7f2e4a6c8b0d1f3e5a7c9b1d3f5e7a origin 9f2b6d4e8a0c2e4f6a8b0c2d4e6f8a0b2c4d6e8f:refs/heads/feat/issue-2471-fix-amplihack-rs-issue
```

Each word of the restore command is quoted with `printf '%q'`, because git
allows `$`, `;`, `` ` `` and `|` in branch names. A branch named
`feat/a;b` appears as `refs/heads/feat/a\;b`. The command is printed only
when it fits whole inside the 1,000-byte message limit and redaction leaves
it unchanged. Otherwise the message ends with
`To restore, push <remote_before> to the branch above with --force-with-lease expecting <HEAD>; the command is too long to print whole.`

### Gate reasons

| Reason | Exit | `pushed` | When |
|---|---|---|---|
| `recovered-environment` | 0 | `"true"` | `ENVIRONMENT`, fast-forward, remote ref equals HEAD |
| `already-at-head` | 0 | `"true"` | `NONE`, fast-forward, remote ref equals HEAD |
| `invalid-agent-output` | 1 | none | rule 2 |
| `not-linked-worktree` | 1 | none | rule 4: no linked worktree, for any cause |
| `branch-mismatch` | 1 | none | rule 5 |
| `upstream-mismatch` | 1 | none | rule 6 |
| `protected-branch` | 1 | none | rule 7 |
| `unsafe-ref` | 1 | none | rules 8 and 8b: bad name, detached HEAD, protected local branch |
| `worktree-changed` | 1 | none | rules 4, 8b, 9: the worktree differs from the snapshot |
| `uncommitted-changes` | 1 | none | rule 9: tracked changes left, so the step's commit failed |
| `diverged-upstream` | 1 | none | rule 10 |
| `remote-history-rewritten` | 1 | none | rule 10b: the remote branch's earlier commit is not an ancestor of HEAD |
| `cause-in-change` | 1 | none | `CHANGE` |
| `push-unverified` | 1 | none | the snapshot is missing or malformed, the origin URLs changed (rule 9b), the ancestry check could not run (rule 10b), or the ref check failed or could not run |
| `invalid-gate-output` | 1 | none | the write-back step found no verified gate record (`commit_result` path only) |

## The stderr recovery record

The gate does not set `continue_on_error`, so when it exits 1 the runner
ends the run without storing the gate's stdout, and no record reaches the
context. (The push steps differ: they set `continue_on_error`, which is why
the runner may keep a failed push step's output.) So that the agent's account is not lost, `fail()` also prints
the redacted agent object to stderr once rule 2 has accepted it:

```text
push-failure-recovery: cause-in-change: cause is in the change
push-failure-recovery: recovery record: {"failed":"step-15-commit-push: pre-push test failed","cause":"CHANGE","branch":"feat/issue-2471-fix-amplihack-rs-issue","evidence":["the clean push failed in pre-push","tests/integration/foo.rs fails on HEAD"],"actions":["env -u RECIPE_VAR_issue_number git push origin HEAD:refs/heads/feat/issue-2471-fix-amplihack-rs-issue"],"push_result":"failed","explanation":"the change breaks a test; a clean push fails the same way"}
```

- The record is `SAFE`: compact `jq -c` output that has already been
  defanged, redacted and cut, as described in [Gate output](#gate-output).
  The raw agent output is never printed.
- Because `jq -c` escapes newlines, the agent cannot add a line that looks
  like a gate line.
- Failures before rule 2 accepts the agent's object (for example
  `invalid-agent-output`) print only the reason line.

## Gate output

The gate prints a record only when it exits 0. It builds the record with
`jq -n --arg/--argjson`. Its own checks supply `sha`, `branch`, `reason`,
`remote_before`, `remote_before_source`, `new_untracked_count` and
`untracked_truncated`. The agent's
object goes, untrusted, under `recovery`:

```json
{
  "pushed": "true",
  "sha": "3c1e9a0b5d7f2e4a6c8b0d1f3e5a7c9b1d3f5e7a",
  "branch": "feat/issue-2471-fix-amplihack-rs-issue",
  "reason": "recovered-environment",
  "recovery_attempted": "true",
  "remote_before": "9f2b6d4e8a0c2e4f6a8b0c2d4e6f8a0b2c4d6e8f",
  "remote_before_source": "fetched",
  "new_untracked_count": 1,
  "untracked_truncated": "false",
  "recovery": {
    "failed": "step-15-commit-push: pre-push hook rejected the push",
    "cause": "ENVIRONMENT",
    "evidence": ["inherited: RECIPE_VAR_issue_number, RECIPE_VAR_pr_number, WORKFLOW_STARTED_AT", "the clean push succeeded"],
    "actions": ["env -u RECIPE_VAR_issue_number -u RECIPE_VAR_pr_number -u WORKFLOW_STARTED_AT git push origin HEAD:refs/heads/feat/issue-2471-fix-amplihack-rs-issue"],
    "push_result": "pushed",
    "explanation": "pre-push tests read RECIPE_VAR_issue_number from the run; the push passed once those variables were removed"
  }
}
```

| Field | Meaning |
|---|---|
| `pushed`, `sha`, `branch`, `reason` | match step-15's `commit_result` shape, so step-16 reads the record unchanged |
| `recovery_attempted` | always `"true"`; the write-back step requires it |
| `remote_before` | the snapshot's value: the remote branch's commit before the agent ran, or `absent` |
| `remote_before_source` | `fetched`, `tracking-ref` or `absent`; shows how exact the fast-forward check was |
| `new_untracked_count` | a number: how many untracked paths are present after the agent ran but not before; information only. The paths are not recorded |
| `untracked_truncated` | the snapshot's flag; when `"true"`, `new_untracked_count` is an upper bound |
| `recovery` | the agent's object, defanged, redacted and cut |

Before the agent's strings go into the record or into `SAFE`, the gate:

- replaces `{{` and `}}`, so a later prompt cannot expand them;
- redacts credentials: URL user-info becomes `scheme://***@`, and `gh*_`,
  `github_pat_`, `Bearer` and `x-access-token` strings are masked, so
  `https://u:ghp_***@example.invalid/x` is stored as
  `https://***@example.invalid/x`;
- then cuts each string to 4096 characters. Redacting first means a cut
  cannot split a credential out of its pattern. `fail()` also redacts before
  it cuts its message to 600 bytes.

The snapshot step has its own copy of `redact()`, with the same four
expressions. A comment at each copy names the other, and the redaction test
in [Tests](#tests) checks both by behaviour.

The gate prints only with `printf '%s'`.

## Writing back `commit_result`

Step `push_recovery_commit` runs only when `push_output_key == 'commit_result'`.
It checks that `RECIPE_VAR_push_recovery_result` is an object with
`pushed: "true"` and `recovery_attempted: "true"` (else it fails with
`invalid-gate-output`), and prints it with `parse_json: true` under
`output: "commit_result"`. When the sub-recipe returns, the context merge
passes the checked `commit_result` back to `workflow-publish`, and step-16
continues with it.

step-18c needs no write-back. No recipe step reads `feedback_push_result`
except the recovery condition. Its record stays under `push_recovery_result`.

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
| `commit_result` or `feedback_push_result` in a calling recipe's context | passed with `--set` by a parent run, or copied up from an earlier sub-recipe | The caller wins over the phase recipe's `""` default, so that default alone is not enough. The `default-workflow` call steps reset each key in their `context:` map (see [Calling the sub-recipe](#calling-the-sub-recipe)). |
| `push_recovery_snapshot`, `push_recovery` or `push_recovery_result` from step-15's recovery, reaching step-18c's call | copied back to `workflow-publish`, then to `default-workflow`, then down to `workflow-pr-review` | Each recovery call step resets all three keys to `""` in its `context:` map. Inside a call, every key is also written before it is read: the snapshot, agent and gate steps have no condition, and the write-back step reads only the gate's output from the same call. |

The stale-value tests in [Tests](#tests) check all three guards against the
installed runner.

## Example run record

The lines below are illustrative. They show the order of steps and results,
not the runner's real log format, which may differ.

A run whose push step failed only because of inherited variables:

```text
step-15-commit-push      failed (continue_on_error)
step-15-push-recovery    running push-failure-recovery
  push_recovery_snapshot linked=true remote_before_source=fetched
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
                         push-failure-recovery: recovery record: {"failed":…,"cause":"CHANGE",…}
run failed: step push_recovery_result exited 1
```

A run whose agent force-pushed over another commit:

```text
step-15-commit-push      failed (continue_on_error)
step-15-push-recovery    running push-failure-recovery
  push_recovery_snapshot remote_before=9f2b6d4… remote_before_source=fetched
  push_recovery          cause=ENVIRONMENT push_result=pushed
  push_recovery_result   push-failure-recovery: remote-history-rewritten: … To restore: git push --force-with-lease=…
run failed: step push_recovery_result exited 1
```

## Restoring a rewritten remote branch

The gate detects a force-push; it does not prevent one, and it never runs the
restore itself. When it fails with `remote-history-rewritten`, its message
gives the command that puts the remote branch back:

```bash
git push --force-with-lease=refs/heads/feat/issue-2471-fix-amplihack-rs-issue:3c1e9a0b5d7f2e4a6c8b0d1f3e5a7c9b1d3f5e7a \
  origin 9f2b6d4e8a0c2e4f6a8b0c2d4e6f8a0b2c4d6e8f:refs/heads/feat/issue-2471-fix-amplihack-rs-issue
```

The first commit is the HEAD the agent pushed; the second is
`remote_before`. The lease makes the push fail, rather than overwrite, if
anyone has pushed to the branch since. Run it from a clone that has
`remote_before` in its object store, after checking that the commit is the
one you want back. When `remote_before_source` was `tracking-ref`, confirm
first that no one pushed to the branch between the last fetch and the run.

## Step budget

Two phase recipes gain one call step each. Each call runs at most four steps,
so a run adds at most eight. That is well inside `max_total_steps` (80 per
phase sub-recipe, 120 for `default-workflow`). All step ids are unique across
the sub-recipes. `workflow-publish.yaml` (398 lines) and
`workflow-pr-review.yaml` (398) have no room left under the 400-line phase
budget; the next step added to either of them needs a split.

## Security limits

- The gate never takes the branch from the agent. It works out the branch
  itself, and refuses an upstream on any other branch, so recovery cannot get
  round step-15's #1762 refusal.
- The gate finds its helpers outside the task worktree, so the files on the
  branch under test cannot replace them. This does not cover the agent; see
  [Residual risks](#residual-risks).
- The gate never reports `pushed: "true"` without checking the remote ref
  against HEAD itself, and without checking that the remote branch's earlier
  commit is an ancestor of HEAD.
- The gate compares the worktree and the `origin` URLs only with its own
  snapshot, taken before the agent ran. The agent's `evidence`, and the
  snapshot's `untracked` and the record's `new_untracked_count`, are
  information only; no rule branches on them.
- The snapshot's fetch and `ls-remote`, and the gate's fetch, run with
  `GIT_TERMINAL_PROMPT=0` and `GCM_INTERACTIVE=never`, so they cannot wait
  for a password. The snapshot's output goes to `/dev/null`, so no URL
  reaches the record. The gate's fetch output appears only in a failure
  message, after `redact` has removed credentials from it. Both fetches use
  `-c core.hooksPath=/dev/null`, so the branch under test cannot run
  `reference-transaction` hooks outside the agent's step.
- Both `git status` calls run with `-c core.fsmonitor=false`, so an
  fsmonitor command set by the agent does not run in the gate.
- The gate ignores replace refs and grafts (`GIT_NO_REPLACE_OBJECTS=1`,
  `GIT_GRAFT_FILE=/dev/null`) for its ancestry checks. It also sets
  `advice.graftFileDeprecated=false` through `GIT_CONFIG_COUNT`, so git does
  not print a deprecation hint on every command.
- The gate refuses a detached HEAD and a protected local branch, as the push
  steps do, so `continue_on_error` does not turn those refusals into a push.
- No bash command contains a `{{...}}` placeholder.
- The only push allowed is `HEAD:refs/heads/<branch>` to `origin`, with hooks
  on.
- Only variable names are collected, never values. Credential files are never
  read.
- The clean environment removes only `RECIPE_VAR_*` and `WORKFLOW_*`. Sign-in
  variables stay.
- Agent output is untrusted: it is validated, defanged, redacted and cut
  before it is stored or printed.

## Residual risks

- The agent runs as the same user as the gate. Before the gate runs, it can
  change the repository's git config (for example a filter driver that
  `git status` runs, or `core.sshCommand`) or overwrite the bundle helpers
  outside the worktree. This gives the agent no power it did not already
  have, but the gate's checks assume neither was changed.
- The gate runs after the agent. A rogue agent's push cannot be undone by the
  gate, so server-side branch protection is still the defence for protected
  branches.
- A force-push by the agent has reached the remote by the time rule 10b
  reports it. The recipe detects it and gives the restore command; preventing
  it needs server-side branch protection on the task branch.
- When `remote_before_source` is `tracking-ref`, the snapshot's fetch failed
  and the branch was not confirmed absent. Rule 10b then compares against the
  last fetched state, not the remote as it was when the agent started. A
  commit pushed by someone else after that fetch, and then overwritten by the
  agent, is not detected. With `fetched` the check is exact.
- When the fetch fails, `ls-remote` fails with an exit code other than 2 and
  there is no tracking ref, the snapshot records `remote_before` as `absent`
  although the branch may exist. Rule 10b skips the check, so a force-push in
  that case is not detected. A follow-up issue is to cover this case and
  the `tracking-ref` case.
- The agent's `CHANGE` or `ENVIRONMENT` judgement can be wrong. A wrong
  `ENVIRONMENT` call still needs a fast-forward push that passed hooks; a
  wrong `CHANGE` call ends the run, which is the conservative failure.
- The snapshot's `status` field has no size cap.

## Out of scope

- step-20b; its recovery belongs to #2570's general recovery step;
- steps 21, 03 and 02b, and `loop-evidence-collector`;
- runner changes and new step fields;
- changes to the push scripts or helper behaviour;
- re-pushing commits stranded by earlier runs;
- version bumps.

The plan is to fold this recovery into #2570's general recovery once both
have landed, with `CHANGE` or an unrepairable environment becoming a handoff
that names a reason, an owner and what is needed.

## Tests

| Test | What it proves |
|---|---|
| `tests/integration/push_failure_recovery.rs` | the recovery wiring, run through `recipe-runner-rs` |
| `tests/integration/workflow_finalize_resilience.rs` | Part 1 of #2471: every `workflow_pr_ready.sh` case runs through `private_scope_command`, which calls `isolate_scope_child`, which clears the environment and sets `HOME`, `TMPDIR`, `GH_CONFIG_DIR`, `GIT_CONFIG_GLOBAL=/dev/null` and `PATH=<tmp>/bin:/usr/bin:/bin`. No `RECIPE_VAR_*`, `WORKFLOW_*`, `GH_TOKEN`, `GITHUB_TOKEN` or `GIT_ASKPASS` value from the host reaches the script, and the fake `gh` lives in `<tmp>/bin` |
| `default_workflow_decomposition_test.rs` | step inventories include `step-15-push-recovery` and `step-18c-push-recovery`; phase line budgets and unique ids hold |
| `issue_1333_leaf_steps_do_not_orchestrate.rs` | the recovery prompt does not start another orchestration |
| `issue_978_step15_rebase_base_test`, `issue_1762_caller_branch_alias_test`, `issue_1671_hook_rejection_repair_test`, `issue_1561_step_outputs_prefer_recipe_var` | the push scripts' `command:` text is unchanged |

`push_failure_recovery.rs` builds a small recipe from the real step-15,
recovery and gate definitions, taken by id. Its fixture:

- a TempDir `HOME`, `TMPDIR` and `XDG_CONFIG_HOME`; `GIT_CONFIG_GLOBAL=/dev/null`
  and `GIT_CONFIG_NOSYSTEM=1`; sign-in and `AMPLIHACK_*` variables removed;
- a local bare `remote.git` whose `pre-receive` hook rejects a push when
  `RECIPE_VAR_issue_number` reaches it;
- a linked worktree on `feat/x`, one commit ahead. The `Upstream` setting
  chooses the remote's `feat/x`: `Ahead`, `AtHead`, `Diverged` (a commit HEAD
  lacks) or `Absent` (`refs/heads/feat/x` deleted from `remote.git` only);
- a fake `AMPLIHACK_LAUNCHER_BINARY` (mode 0700) written by the test. It
  refuses to run outside the TempDir, makes the clean-environment push, prints
  the agent JSON, and answers step-15's hook-repair prompt without making a
  fix. For each step in its literal step list it copies the snapshot to
  `$T/snapshot-<step>.json`, so tests can read it.

Cases:

| Case | Expected |
|---|---|
| positive (step-15) | the script fails, recovery runs, the remote ref equals HEAD, `commit_result.pushed == "true"`, the record shows `cause=ENVIRONMENT`, names `RECIPE_VAR_issue_number` and lists the clean push |
| negative (`CHANGE`) | the hook always fails and the fake agent returns `CHANGE`; the run exits non-zero with `cause-in-change`, stderr contains `recovery record`, `explanation` and `evidence`, and the remote does not move |
| force-push (`step15_force_push_over_another_commit_fails_closed`) | `Diverged` fixture, `force` agent mode: the agent force-pushes HEAD over the remote's commit and returns `ENVIRONMENT` with a token-shaped `explanation`; the run exits non-zero, the step is failed, the output contains `remote-history-rewritten` and both commits, `git ls-remote` on `remote.git` shows HEAD, and the token appears only in redacted form |
| absent remote (`step15_first_push_with_absent_remote_is_recovered`) | `Absent` fixture: the run is recovered, the snapshot records `remote_before` and `remote_before_source` as `absent`, and origin equals HEAD |
| untracked file (`step15_untracked_file_left_by_agent_is_recovered_and_recorded`) | `env-untracked` agent mode leaves a new untracked file; the run is recovered, `new_untracked_count` is 1 and the path is not named in the record |
| untracked past the cap (`step15_untracked_count_past_snapshot_cap_is_flagged`) | 250 untracked files exist before the snapshot and the agent adds one; the snapshot lists 200 with `untracked_truncated: "true"`, and the record has `new_untracked_count: 51` and the same flag |
| quoted restore command (`step15_restore_command_quotes_branch_with_shell_metacharacters`) | branch ``feat/x;$(touch>pwned)`touch>pwned`\|a&b>pwned`` with the `force` agent: bash reads the printed restore command back as the five intended words and creates no file |
| upstream on another branch | the worktree's upstream is `origin/feat/other`, so step-15 refuses (#1762); a misbehaving fake agent pushes `HEAD:refs/heads/feat/x` and returns `ENVIRONMENT`; the run exits non-zero with `upstream-mismatch` and step-16 does not run |
| diverged upstream | the remote branch has a commit HEAD lacks, so step-15 refuses (#978); the fake agent returns `ENVIRONMENT` without pushing; the run exits non-zero with `diverged-upstream` |
| `NONE` on step-15 | the remote ref already equals HEAD; `commit_result.pushed == "true"` with reason `already-at-head`, and step-16 runs |
| stale environment | an inherited `RECIPE_VAR_commit_result` of `{"pushed":"true"}` does not skip recovery |
| stale caller context | a caller context with `commit_result` set to `{"pushed":"true"}` does not skip recovery, because the `default-workflow` call step resets it |
| stale recovery keys | values of `push_recovery` and `push_recovery_result` left by step-15's recovery do not reach step-18c's call |
| step-18c | the same harness runs step-18c's call step |
| failed commit | a `pre-commit` hook rejects step-15's commit, origin already equals the old HEAD, the fake agent returns `NONE`; the run fails with `uncommitted-changes` and step-16 does not run |
| agent commits | the fake agent commits a new file, pushes it and returns `ENVIRONMENT`; the run fails with `worktree-changed` |
| protected local branch | the worktree is on `master`, so step-15 refuses; the fake agent pushes `HEAD:refs/heads/feat/x` anyway; the run fails with `unsafe-ref` |
| step-18c without a worktree | the fake agent returns `NONE`; the snapshot holds only `linked`, passes rule 2, and the run fails with `not-linked-worktree`, not `push-unverified` |
| step-18c detached HEAD | step-18c refuses; the fake agent pushes anyway; the run fails with `unsafe-ref` |

Every case also checks that the workspace `git status` and the remote `main`
are unchanged, and that the record holds no variable values.

Limits:

- The fake agent tests the wiring, not the model's judgement.
- The behaviour on failure (caller context wins over defaults, call-step
  `context:` wins over both) was read from runner rev `4d5187e`. The
  stale-value cases are what prove it against the installed runner.
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
