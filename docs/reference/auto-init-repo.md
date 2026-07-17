# auto_init_repo — Reference

Recipe context variable that controls whether `workflow-prep` step-01
(`step-01-prepare-workspace`) initializes a new git repository when `repo_path`
is **not** inside a git work tree. This enables repo-creation tasks — where the
goal is to produce a brand-new repository — to proceed through the workflow
instead of hard-failing at the first step (issue #900).

## Contents

- [Motivation](#motivation)
- [Declaration sites](#declaration-sites)
- [Type and default](#type-and-default)
- [Propagation chain](#propagation-chain)
- [Implementation checklist](#implementation-checklist)
- [Bash consumption](#bash-consumption)
- [Auto-init behavior](#auto-init-behavior)
- [Interaction with other context variables](#interaction-with-other-context-variables)
- [Override at invocation](#override-at-invocation)
- [Source](#source)

---

## Motivation

Before this feature, `step-01-prepare-workspace` ran an unconditional guard:

```bash
if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "ERROR: step-01-prepare-workspace requires a git repo ..." >&2
  exit 1
fi
```

This hard-fail broke any task whose goal was to **create** a repository: the
workspace does not yet contain a `.git` directory, so step-01 exited `1` before
the agent could do any work. The later steps in `workflow-prep.yaml` (the guards
near the metadata and local-tracking sections) already degrade gracefully for
non-git directories, so only step-01 needed to change.

`auto_init_repo` replaces the hard-fail with a gated auto-initialization: when
the workspace is not a git work tree and the flag is enabled (the default),
step-01 runs `git init -b main`, prints an informational line, and continues.

---

## Declaration sites

The variable is declared in the recipe context blocks that flow into
`workflow-prep`:

| Recipe | File | Purpose |
|--------|------|---------|
| `smart-orchestrator` | `amplifier-bundle/recipes/smart-orchestrator.yaml` | Top-level entry point for `/dev` tasks |
| `default-workflow` | `amplifier-bundle/recipes/default-workflow.yaml` | 23-step development workflow |

Both declare it identically:

```yaml
context:
  auto_init_repo: "true"
```

---

## Type and default

**Type:** string
**Default:** `"true"` (auto-init enabled)
**Valid values:** `"true"` (auto-init a new repo on non-git dirs), `"false"` (restore the pre-#900 hard-fail)

The value is a quoted YAML string, not a native boolean. This matches the
pattern used by `skip_pre_agent_validation` and `force_single_workstream:
"false"` and avoids YAML 1.1 boolean parsing quirks (`yes`/`no`/`on`/`off`).

The recipe runner exposes context variables as environment variables, which are
always strings. Using string type is explicit and prevents type-mismatch
surprises.

**The default MUST allow repo-creation tasks to proceed.** `"true"` is the
default so that the common case — an agent asked to scaffold a new project —
works without any extra flags. Setting the flag to `"false"` is a fail-closed
opt-out intended for locked-down or CI contexts that must never create a
repository implicitly.

---

## Propagation chain

The variable flows through the recipe hierarchy the same way
`skip_pre_agent_validation` does:

```
smart-orchestrator.yaml          (declares auto_init_repo: "true")
  └─ smart-execute-routing.yaml  (explicitly forwards in context blocks)
       └─ default-workflow.yaml  (declares auto_init_repo: "true")
            └─ workflow-prep.yaml  (consumes as $AUTO_INIT_REPO)
```

`smart-execute-routing.yaml` explicitly forwards the variable in every
`context:` block that also forwards `worktree_setup`, `allow_no_op`, and
`skip_pre_agent_validation`. This defensive pattern ensures the value reaches
`default-workflow` even if automatic context-merging semantics change.

**This forwarding is load-bearing, not cosmetic.** Auto-init's happy path works
even without it (an unset `$AUTO_INIT_REPO` is empty → default-allow → auto-init
fires). But the documented fail-closed opt-out `-c auto_init_repo="false"` only
takes effect if the value is forwarded end-to-end; without the declaration and
forwarding, the `"false"` never reaches step-01 and the security control is
silently inert. See [Implementation checklist](#implementation-checklist).

Intermediate routing recipes (`smart-classify-route.yaml`) pass context through
without consuming it. If such a recipe introduces a `context:` block that would
shadow this variable, add explicit forwarding there too.

---

## Implementation checklist

`auto_init_repo` is **not** a single-file change. Because the fail-closed
opt-out (`-c auto_init_repo="false"`) must reach `workflow-prep` from the CLI
invocation, the variable has to be declared **and** forwarded through the whole
recipe chain — exactly like `skip_pre_agent_validation`. Editing only
`workflow-prep.yaml` leaves the happy path working (empty value → default-allow)
but makes the documented `"false"` opt-out silently non-functional.

The complete change set is:

| File | Edit | Anchor |
|------|------|--------|
| `smart-orchestrator.yaml` | Declare `auto_init_repo: "true"` in the top-level `context:` block | Next to `skip_pre_agent_validation: "true"` (~line 54) |
| `default-workflow.yaml` | Declare `auto_init_repo: "true"` in the `context:` block | Next to `skip_pre_agent_validation: "true"` (~line 96) |
| `smart-execute-routing.yaml` | Forward `auto_init_repo: "{{auto_init_repo}}"` in each context block that forwards `skip_pre_agent_validation` | 3 blocks (~lines 123, 235, 371) |
| `workflow-prep.yaml` | Consume `$AUTO_INIT_REPO` in the step-01 work-tree guard | `step-01-prepare-workspace` |

Regression coverage lives in
`tests/integration/workflow_prep_git_recovery_test.rs` (auto-init path,
disabled-flag hard-fail, unchanged existing-checkout path).

---

## Bash consumption

`workflow-prep.yaml` step-01 (`step-01-prepare-workspace`) reads the variable as
`$AUTO_INIT_REPO` inside the work-tree guard, after `cd "$REPO_PATH"`:

```bash
if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  if [ "$AUTO_INIT_REPO" = "false" ]; then
    echo "ERROR: step-01-prepare-workspace requires a git repo at $(pwd); either \`git init\` or rerun from a checkout (set auto_init_repo=true to auto-initialize)" >&2
    exit 1
  fi
  echo "[init] no git repo found — initialized a new one for repo-creation task"
  git init -b main >/dev/null 2>&1 \
    || { git init >/dev/null 2>&1 && git checkout -b main >/dev/null 2>&1; } \
    || git symbolic-ref HEAD refs/heads/main
fi
# ... existing status / fetch / branch diagnostics run unchanged ...
```

**Default-allow semantics:** Auto-init runs unless it is explicitly disabled via
`"false"`. Empty, unset, or any other value — including the default `"true"` —
enables auto-init. This is intentional: repo-creation tasks are a first-class
use case and must proceed without extra flags.

**Idempotent and non-destructive:** The auto-init block only runs when
`git rev-parse --is-inside-work-tree` fails — i.e., there is no existing repo.
It never re-initializes or clobbers a `.git` directory in a real checkout. For
real checkouts, step-01 behavior is byte-for-byte unchanged: status, fetch (with
the ADO-remote remediation hints), and current-branch diagnostics all run as
before.

**Security:** `$REPO_PATH` and `$AUTO_INIT_REPO` are always double-quoted in
bash to prevent word splitting on empty or malformed values. Only strict string
equality `= "false"` is used — never `eval`, pattern matching, or unquoted
comparison. The initial branch name is a hard-coded literal `main` and is never
derived from an environment variable.

---

## Auto-init behavior

When auto-init fires, step-01:

1. Prints the informational line
   `[init] no git repo found — initialized a new one for repo-creation task`
   to stdout (not stderr — this is normal, expected behavior, not a warning).
2. Creates a new repository on the `main` branch in `$REPO_PATH`.
3. Continues into the existing status / fetch / branch diagnostics, which now
   operate against the freshly-initialized repo (empty status, no remote to
   fetch — both handled gracefully by the existing `|| WARNING` guards).

### Initial-branch fallback chain

`git init -b <branch>` requires git **2.28+**. To remain portable on older git,
step-01 uses a three-tier fallback:

| Tier | Command | Applies to |
|------|---------|-----------|
| 1 | `git init -b main` | git ≥ 2.28 |
| 2 | `git init` then `git checkout -b main` | older git, no commits yet |
| 3 | `git symbolic-ref HEAD refs/heads/main` | last-resort HEAD retarget |

All three converge on an unborn `main` branch, so downstream steps see a
consistent default branch regardless of the host git version.

---

## Interaction with other context variables

| Variable | Relationship |
|----------|-------------|
| `skip_pre_agent_validation` | Independent. Uses the same string-as-boolean pattern. `auto_init_repo` gates the work-tree guard at the top of step-01; `skip_pre_agent_validation` gates the validation block at the bottom of step-01. |
| `force_single_workstream` | Independent. Same string-as-boolean pattern. |
| `allow_no_op` | Independent. `allow_no_op` opts out of the step-08c work-verifier; `auto_init_repo` controls step-01 repo initialization. |
| `worktree_setup` | Independent. Both are explicitly forwarded in `smart-execute-routing.yaml`. When `worktree_setup` provisions a worktree, that worktree is already a git work tree, so auto-init does not fire. |

---

## Override at invocation

Auto-init is enabled by default, so repo-creation tasks need no extra flags:

```sh
amplihack recipe run amplifier-bundle/recipes/smart-orchestrator.yaml \
  -c task_description="scaffold a new Rust CLI project" \
  -c repo_path="./my-new-project"
```

To disable auto-init and restore the pre-#900 hard-fail (fail-closed for
locked-down or CI contexts that must never create a repository):

```sh
amplihack recipe run amplifier-bundle/recipes/smart-orchestrator.yaml \
  -c task_description="refactor auth module" \
  -c repo_path="." \
  -c auto_init_repo="false"
```

To restore the default (auto-init enabled), omit the flag or set it explicitly:

```sh
-c auto_init_repo="true"
```

---

## Source

- `amplifier-bundle/recipes/smart-orchestrator.yaml` — context declaration
- `amplifier-bundle/recipes/default-workflow.yaml` — context declaration
- `amplifier-bundle/recipes/smart-execute-routing.yaml` — explicit forwarding
- `amplifier-bundle/recipes/workflow-prep.yaml` — bash consumption in step-01
- `tests/integration/workflow_prep_git_recovery_test.rs` — regression tests for
  the auto-init path, the disabled-flag hard-fail, and the unchanged
  existing-checkout path

## Related

- [Create a New Repository from a Workflow Task](../howto/create-a-new-repository.md) — Step-by-step guide for repo-creation tasks
- [skip_pre_agent_validation Reference](./skip-pre-agent-validation.md) — Sibling step-01 context variable using the same pattern
- [Recipe Executor Environment](./recipe-executor-environment.md) — How context variables become environment variables in shell steps
- [Environment Variables](./environment-variables.md) — All variables read or injected by `amplihack`
- [Troubleshoot Recipe Execution](../howto/troubleshoot-recipe-execution.md) — Common recipe failure modes
