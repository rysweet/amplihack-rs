# worktree_setup Context Propagation — Reference

Complete reference for how the `worktree_setup` context variable propagates through the composable default-workflow recipe chain, from `smart-orchestrator` down to individual phase sub-recipes.

## Contents

- [Background](#background)
- [The propagation chain](#the-propagation-chain)
- [Which recipes declare worktree_setup](#which-recipes-declare-worktree_setup)
- [Which recipes do NOT declare worktree_setup](#which-recipes-do-not-declare-worktree_setup)
- [Related context variable: allow_no_op](#related-context-variable-allow_no_op)
- [Base inputs: base_branch and base_sha](#base-inputs-base_branch-and-base_sha)
- [How context flows through sub-recipes](#how-context-flows-through-sub-recipes)
- [Shell variable expansion](#shell-variable-expansion)
- [Regression test coverage](#regression-test-coverage)
- [Troubleshooting](#troubleshooting)

---

## Background

`workflow-worktree` (step 04 of default-workflow) creates a git worktree and branch for the current task. It outputs a JSON object called `worktree_setup` containing:

| Field | Example value | Purpose |
|-------|---------------|---------|
| `worktree_path` | `/tmp/worktrees/feat-auth-1234` | Absolute path to the created worktree |
| `branch_name` | `feat/add-auth-1234` | Branch name created or reused |
| `is_new_worktree` | `true` | Whether a new worktree was created vs. reattached |
| `base_ref` | `origin/dev` | Ref the worktree was resolved from; a full SHA when only `base_sha` was given |
| `base_branch` | `dev` | Branch the PR targets; empty when only `base_sha` was given |
| `base_sha` | `5d1e0c9b8a7f6e5d4c3b2a1908f7e6d5c4b3a291` | Commit the worktree started at; empty on the legacy and existing-branch paths |
| `source_verified` | `verified` | `verified`, `legacy` or `not-applicable` |
| `source_rule` | `fetched` | Rule that decided the source: `fetched`, `legacy-unverified` or `existing-branch` |

`consensus-issue-worktree.yaml` (`step3-setup-worktree`) emits the same five
base fields. `source_rule` is in the JSON because the runner drops the stderr of
a step that passes, so the `RULE:` line is not kept. See
[Worktree Source Resolution](../../amplifier-bundle/tools/workflow_worktree_source.md)
for every value (issue #1957).

Downstream steps — particularly the `step-08c-work-verifier` / `step-08c-enforce-verdict` pair in `workflow-tdd` (issue #615 — replaced the legacy six-escape-hatch bash hollow-success guard with an agentic verifier) — read `WORKTREE_SETUP_WORKTREE_PATH` so the verifier agent runs inside the worktree where implementation actually happened. Without this variable, the verifier cannot inspect the right working tree and may fall back to `INSUFFICIENT_EVIDENCE`.

---

## The propagation chain

Context must flow through every recipe boundary in the chain. The recipe runner's `_execute_sub_recipe()` merges parent context into child context, but only for keys the child **declares** in its `context:` block.

```
smart-orchestrator
  └─ smart-execute-routing
       └─ default-workflow          ← declares worktree_setup: ""
            ├─ workflow-prep        ← does NOT declare (runs before worktree creation)
            ├─ workflow-worktree    ← PRODUCES worktree_setup (does not consume it)
            ├─ workflow-design      ← does NOT declare (runs before worktree output is needed)
            ├─ workflow-tdd         ← declares worktree_setup: ""  ← CONSUMER (step-08c work-verifier)
            ├─ workflow-refactor-review  ← declares worktree_setup: ""
            ├─ workflow-precommit-test   ← declares worktree_setup: ""
            ├─ workflow-publish          ← declares worktree_setup: ""
            ├─ workflow-pr-review        ← declares worktree_setup: ""
            └─ workflow-finalize         ← declares worktree_setup: ""
```

The `smart-execute-routing` call sites that invoke `default-workflow` pass `worktree_setup` in their explicit `context:` blocks, ensuring the value threads from the orchestrator level into the workflow.

---

## Which recipes declare worktree_setup

Every post-worktree phase sub-recipe declares both `worktree_setup` and `allow_no_op` in its `context:` block:

| Recipe file | Context declaration |
|-------------|-------------------|
| `default-workflow.yaml` | `worktree_setup: ""` |
| `workflow-tdd.yaml` | `worktree_setup: ""` |
| `workflow-refactor-review.yaml` | `worktree_setup: ""` |
| `workflow-precommit-test.yaml` | `worktree_setup: ""` |
| `workflow-publish.yaml` | `worktree_setup: ""` |
| `workflow-pr-review.yaml` | `worktree_setup: ""` |
| `workflow-finalize.yaml` | `worktree_setup: ""` |

The empty-string default is intentional. Bash `${VAR:?msg}` and `${VAR:+alt}` guards in shell steps correctly detect an empty string as "not yet set," providing safety when a sub-recipe runs before `workflow-worktree` has executed (which should not happen in normal flow, but is defensive).

---

## Which recipes do NOT declare worktree_setup

| Recipe file | Reason |
|-------------|--------|
| `workflow-prep.yaml` | Runs before worktree creation (steps 00–03b). `worktree_setup` does not exist yet. |
| `workflow-design.yaml` | Runs before worktree creation (steps 05–06d). Same reason. |
| `workflow-worktree.yaml` | Produces `worktree_setup` as output. Does not consume it. |

---

## Related context variable: allow_no_op

`allow_no_op` controls the `step-08c-enforce-verdict` fast-path opt-out in `workflow-tdd` (issue #615). When `true`, the enforcer treats the step as `WORK_VERIFIED` without parsing the verifier's verdict — used for orchestration, docs-only, or audit tasks that legitimately produce no working-tree edits.

| Value | Behavior |
|-------|----------|
| `false` (default) | step-08c runs the agentic work-verifier and enforces its verdict |
| `true` | step-08c-enforce-verdict short-circuits to exit 0 (orchestration / docs-only / audit / meta) |

`allow_no_op` is declared alongside `worktree_setup` in every post-worktree sub-recipe. The smart-classify-route step in `smart-orchestrator` sets it to `true` when the task classification permits no working-tree edits.

---

## Base inputs: base_branch and base_sha

`default-workflow.yaml` and `consensus-workflow.yaml` declare two inputs next
to `existing_branch`:

```yaml
  base_branch: ""   # remote branch the task worktree starts from
  base_sha: ""      # exact full-length commit; wins over base_branch, which must contain it
```

They reach `workflow-worktree` and `consensus-issue-worktree` by the normal
context merge. `smart-orchestrator.yaml`, `autodrive-build.yaml` and
`auto-drive-to-merge.yaml` forward `{{base_branch}}` and `{{base_sha}}` at
every call site that already forwards `existing_branch`.

Both keys are in `is_essential_context_key` in
`crates/amplihack-cli/src/commands/recipe/run/execute.rs`, so the #1023
environment budget never drops them.

Both step bodies copy the inputs on their first two lines, because the step
later sets its own local `BASE_BRANCH`:

```bash
REQ_BASE_BRANCH="${BASE_BRANCH:-}"
REQ_BASE_SHA="${BASE_SHA:-}"
```

### How later steps read the verified base

`workflow_publish_pr.sh` (`resolve_pr_base_ref`), `workflow_pr_ready.sh` and
`workflow_final_status.sh` (`resolve_expected_base_branch`) call
`workflow_worktree_source.sh verified-base`. Both values come from the recipe
context, or, only when both context keys are absent, both come from
`WORKTREE_SETUP_SOURCE_VERIFIED` and `WORKTREE_SETUP_BASE_BRANCH`.

| Exit | Meaning | What the PR tool does |
| --- | --- | --- |
| 0 | `source_verified` is exactly `verified`, `base_branch` passes the input check again, and `refs/remotes/origin/<b>` resolves. Prints `<b>`. | Uses `<b>` (`origin/<b>` in `workflow_publish_pr.sh`). |
| 1 | `verified` was recorded but the name or ref is unusable, or the context cannot be read. One `ERROR:` line. | Refuses. |
| 3 | Nothing was verified: `legacy`, `not-applicable`, any other value, absent, no context, or a SHA-only run. | Runs `requested-inputs`. |

`workflow_worktree_source.sh requested-inputs` reads the top-level
`base_branch` and `base_sha` and prints nothing. It exits 0 if either is set,
3 if neither is, and 1 if the context cannot be read. After a `verified-base`
exit 3, only a `requested-inputs` exit 3 lets the tool use its existing
`origin/HEAD` chain; any other result refuses. A missing
`recipe_context_value.sh` counts as "cannot read" (exit 1), never as "not
given". When `workflow_publish_pr.sh` refuses, it still writes its publish
result, with state `FAILED_INVALID_INPUT` (`invalid-input`).

On the legacy path `base_branch` is filled in but `source_verified` is
`legacy`, so `verified-base` exits 3, `requested-inputs` exits 3, and the PR
base does not change. A run that set `base_branch` or `base_sha` but has no
verified `worktree_setup` (lost context, or SHA-only) refuses instead of
targeting the default branch, and a stale value from a parent run cannot change
the PR base.

---

## How context flows through sub-recipes

The recipe runner's `_execute_sub_recipe()` method merges context as follows:

1. The parent recipe's accumulated context (all declared keys + outputs from completed steps) forms the base.
2. The child recipe's `context:` block declares which keys it accepts and their defaults.
3. For each key the child declares, if the parent has a value for that key, the parent's value overwrites the child's default.
4. Keys the child does NOT declare are not passed through — they are invisible to the child.

This is why every consuming sub-recipe must explicitly declare `worktree_setup` and `allow_no_op`. Without the declaration, the recipe runner silently drops the value at the boundary.

---

## Shell variable expansion

Inside shell steps, `worktree_setup` is a JSON object in
`RECIPE_VAR_worktree_setup`. The runner sets no upper-case alias for an
object, and it writes nested `RECIPE_VAR_worktree_setup__<field>` names only
for the fields the current object has, so neither a bare
`WORKTREE_SETUP_WORKTREE_PATH` nor a nested name is read; either can hold a
parent run's value.

| JSON path | Read with | Example value |
|-----------|-----------|---------------|
| `worktree_setup.worktree_path` | [`worktree_setup_path.sh <step-label>`](../../amplifier-bundle/tools/worktree_setup_path.sh) | `/tmp/worktrees/feat-auth-1234` |
| `worktree_setup.branch_name` | `rv_field worktree_setup branch_name` ([`recipe_var.sh`](../../amplifier-bundle/tools/recipe_var.sh)) | `feat/add-auth-1234` |

`worktree_setup_path.sh` prints the path only when it is a resolved worktree
root, and otherwise exits non-zero; each step assigns the result to a local
`WORKTREE_SETUP_WORKTREE_PATH` and decides what a failure means (see the
tool's header and
[Step outputs ignore inherited bare variables](auto-drive-to-merge.md#step-outputs-ignore-inherited-bare-variables)).

### Directory access tiers

Not all steps that consume the worktree path treat a missing
directory the same way. Steps are classified into three tiers:

| Tier | Steps | Guard pattern | Missing directory behavior |
|------|-------|---------------|---------------------------|
| **Hard-fail** | 15, 16, 18c | `cd "${WORKTREE_SETUP_WORKTREE_PATH:?…}"` | Recipe aborts (`:?` expansion errors on unset/null variable) |
| **Resilient** | 19c, 21 | `if [ -d "${WORKTREE_SETUP_WORKTREE_PATH:-}" ]; then cd …; elif [ -d "${REPO_PATH:-}" ]; then cd …; fi` | Falls back to `REPO_PATH` then `$(pwd)`, emits WARNING to stderr |
| **Skip** | 20a, 20b | `if [ -z "$wt" ] \|\| [ ! -d "$wt" ]; then echo "WARNING: …" >&2; exit 0; fi; cd "$wt"` | Also skips when the path is not the top of a linked worktree. Emits WARNING to stderr and exits `0`; never runs in `REPO_PATH` |

Hard-fail steps need worktree files to perform their work (review, verdict
enforcement). Resilient steps perform read-only verification that functions
from any directory inside the repository. Skip steps stage, commit, or clean
up, and `REPO_PATH` is the launch checkout, so they run in the task worktree or
not at all (issue #1528).

See [Issue #647 — Resilient Worktree Cleanup](../recipes/issue-647-resilient-worktree-cleanup.md)
for the full rationale and fallback chain specification.

---

## Regression test coverage

`TestWorktreeSetupPropagation479` in `amplifier-bundle/tools/test_default_workflow_fixes.py` validates the propagation chain with four test methods:

| Test | Assertion |
|------|-----------|
| `test_post_worktree_sub_recipes_declare_worktree_setup` | All 6 post-worktree sub-recipes declare `worktree_setup` in their context |
| `test_post_worktree_sub_recipes_declare_allow_no_op` | All 6 post-worktree sub-recipes declare `allow_no_op` in their context |
| `test_default_workflow_declares_worktree_setup` | `default-workflow.yaml` itself declares `worktree_setup` in context (regression guard) |
| `test_smart_execute_routing_forwards_worktree_setup` | All `default-workflow` call sites in `smart-execute-routing.yaml` include `worktree_setup` in their context blocks |

These tests load the YAML files directly and parse the `context:` sections, catching any future regression where a context declaration is accidentally removed.

---

## Troubleshooting

**Error: `WORKTREE_SETUP_WORKTREE_PATH: ... requires worktree_setup.worktree_path...`** (legacy step-08c bash guard, removed by issue #615)

If you still see this error from a stale build, upgrade. In the agentic
verifier, missing worktree path manifests as the verifier reporting
`INSUFFICIENT_EVIDENCE` instead. The underlying cause is the same: a
sub-recipe in the chain is missing the `worktree_setup` context declaration. Check:

1. The sub-recipe's `context:` block includes `worktree_setup: ""`.
2. The parent recipe's call site passes `worktree_setup` in its context.
3. The `default-workflow.yaml` context block includes `worktree_setup: ""`.

Run the regression tests to identify the gap:

```sh
python -m pytest amplifier-bundle/tools/test_default_workflow_fixes.py::TestWorktreeSetupPropagation479 -v
```

## See also

- [Recipe Executor Environment](./recipe-executor-environment.md) — Context augmentation for shell and agent steps
- [step-04-setup-worktree](./recipe-step-04-worktree-reattach-prune.md) — Worktree creation and reattach semantics
- [Worktree Source Resolution](../../amplifier-bundle/tools/workflow_worktree_source.md) — `base_branch` / `base_sha` contract and `verified-base`
- [Git Worktree Support](../concepts/worktree-support.md) — Runtime directory resolution for worktrees
- [Recipe Execution Flow](../concepts/recipe-execution-flow.md) — How recipes are loaded and executed
- [Smart-Orchestrator Recovery](../concepts/smart-orchestrator-recovery.md) — Hollow-success detection and the no-op guard
- [Issue #647 — Resilient Worktree Cleanup](../recipes/issue-647-resilient-worktree-cleanup.md) — Late-stage fallback chain for missing worktree directories
