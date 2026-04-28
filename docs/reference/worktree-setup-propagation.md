# worktree_setup Context Propagation — Reference

Complete reference for how the `worktree_setup` context variable propagates through the composable default-workflow recipe chain, from `smart-orchestrator` down to individual phase sub-recipes.

## Contents

- [Background](#background)
- [The propagation chain](#the-propagation-chain)
- [Which recipes declare worktree_setup](#which-recipes-declare-worktree_setup)
- [Which recipes do NOT declare worktree_setup](#which-recipes-do-not-declare-worktree_setup)
- [Related context variable: allow_no_op](#related-context-variable-allow_no_op)
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

Downstream steps — particularly step-08c (the no-op/hollow-success guard in `workflow-tdd`) — read `WORKTREE_SETUP_WORKTREE_PATH` to verify that implementation actually produced file changes in the worktree directory. Without this variable, step-08c cannot locate the worktree and fails with:

```
WORKTREE_SETUP_WORKTREE_PATH: step-08c requires worktree_setup.worktree_path
from step-04 (workflow-worktree); ensure parent recipe ran worktree-setup and
propagated outputs
```

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
            ├─ workflow-tdd         ← declares worktree_setup: ""  ← CONSUMER (step-08c)
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

`allow_no_op` controls the step-08c hollow-success guard in `workflow-tdd`. When `true`, the guard permits a step to complete without file changes — used for orchestration, docs-only, or audit tasks that legitimately produce no working-tree edits.

| Value | Behavior |
|-------|----------|
| `false` (default) | step-08c requires at least one file change in the worktree |
| `true` | step-08c skips the file-change check |

`allow_no_op` is declared alongside `worktree_setup` in every post-worktree sub-recipe. The smart-classify-route step in `smart-orchestrator` sets it to `true` when the task classification permits no working-tree edits.

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

Inside shell steps, `worktree_setup` is a JSON string. The recipe runner flattens it using `parse_json` into individual environment variables:

| JSON path | Environment variable | Example value |
|-----------|---------------------|---------------|
| `worktree_setup.worktree_path` | `WORKTREE_SETUP_WORKTREE_PATH` | `/tmp/worktrees/feat-auth-1234` |
| `worktree_setup.branch_name` | `WORKTREE_SETUP_BRANCH_NAME` | `feat/add-auth-1234` |
| `worktree_setup.is_new_worktree` | `WORKTREE_SETUP_IS_NEW_WORKTREE` | `true` |

The flattening convention is: uppercase the parent key, replace dots with underscores, and uppercase each nested key. `worktree_setup.worktree_path` becomes `WORKTREE_SETUP_WORKTREE_PATH`.

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

**Error: `WORKTREE_SETUP_WORKTREE_PATH: step-08c requires worktree_setup.worktree_path...`**

This means a sub-recipe in the chain is missing the `worktree_setup` context declaration. Check:

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
- [Git Worktree Support](../concepts/worktree-support.md) — Runtime directory resolution for worktrees
- [Recipe Execution Flow](../concepts/recipe-execution-flow.md) — How recipes are loaded and executed
- [Smart-Orchestrator Recovery](../concepts/smart-orchestrator-recovery.md) — Hollow-success detection and the no-op guard
