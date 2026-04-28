# skip_pre_agent_validation — Reference

Recipe context variable that controls whether the pre-agent project validation guard runs before the agent step in the default workflow. The guard block is a hook point where project-specific validation commands can be added per-repository; the initial implementation contains echo stubs.

## Contents

- [Declaration sites](#declaration-sites)
- [Type and default](#type-and-default)
- [Propagation chain](#propagation-chain)
- [Bash consumption](#bash-consumption)
- [Interaction with other context variables](#interaction-with-other-context-variables)

---

## Declaration sites

The variable is declared in two recipe context blocks:

| Recipe | File | Purpose |
|--------|------|---------|
| `smart-orchestrator` | `amplifier-bundle/recipes/smart-orchestrator.yaml` | Top-level entry point for `/dev` tasks |
| `default-workflow` | `amplifier-bundle/recipes/default-workflow.yaml` | 23-step development workflow |

Both declare it identically:

```yaml
context:
  skip_pre_agent_validation: "true"
```

---

## Type and default

**Type:** string
**Default:** `"true"` (validation skipped)
**Valid values:** `"true"` (skip), `"false"` (run validation)

The value is a quoted YAML string, not a native boolean. This matches the pattern used by `force_single_workstream: "false"` in `smart-orchestrator.yaml` and avoids YAML 1.1 boolean parsing quirks (`yes`/`no`/`on`/`off`).

The recipe runner exposes context variables as environment variables, which are always strings. Using string type is explicit and prevents type-mismatch surprises.

---

## Propagation chain

The variable flows through the recipe hierarchy:

```
smart-orchestrator.yaml          (declares skip_pre_agent_validation: "true")
  └─ smart-execute-routing.yaml  (explicitly forwards in context blocks)
       └─ default-workflow.yaml  (declares skip_pre_agent_validation: "true")
            └─ workflow-prep.yaml  (consumes as $SKIP_PRE_AGENT_VALIDATION)
```

`smart-execute-routing.yaml` explicitly forwards the variable in every `context:` block that also forwards `worktree_setup` and `allow_no_op`. This defensive pattern ensures the value reaches `default-workflow` even if automatic context-merging semantics change.

Intermediate routing recipes (`smart-classify-route.yaml`) are expected to pass context through without consuming it. Validate during implementation that `smart-classify-route.yaml` does not contain a `context:` block that would shadow this variable; if it does, add explicit forwarding there too.

---

## Bash consumption

`workflow-prep.yaml` step-01 (`step-01-prepare-workspace`) reads the variable as `$SKIP_PRE_AGENT_VALIDATION` after the git workspace preparation commands:

```bash
if [ "$SKIP_PRE_AGENT_VALIDATION" = "false" ]; then
  echo "Running pre-agent project validation..."
  # Project-specific validation commands are added here per-repository.
  # The initial implementation contains echo stubs only.
fi
```

**Default-deny semantics:** Validation runs only when explicitly opted in via `"false"`. Empty, unset, or any other value — including the default `"true"` — skips validation. This is intentional: the agent's own validation at the end of each round is sufficient for most workflows.

**Security:** The variable value is always double-quoted in bash (`"$SKIP_PRE_AGENT_VALIDATION"`) to prevent word splitting on empty or malformed values. Only strict string equality `= "false"` is used — never `eval`, pattern matching, or unquoted comparison.

---

## Interaction with other context variables

| Variable | Relationship |
|----------|-------------|
| `force_single_workstream` | Independent. Uses the same string-as-boolean pattern. |
| `allow_no_op` | Independent. `allow_no_op` controls the step-08c hollow-success guard; `skip_pre_agent_validation` controls step-01 pre-flight validation. |
| `worktree_setup` | Independent. Both are explicitly forwarded in `smart-execute-routing.yaml`. |

---

## Override at invocation

Pass the variable via `-c` to opt in to pre-agent validation:

```sh
amplihack recipe run amplifier-bundle/recipes/smart-orchestrator.yaml \
  -c task_description="refactor auth module" \
  -c repo_path="." \
  -c skip_pre_agent_validation="false"
```

To restore the default (skip validation), omit the flag or set it explicitly:

```sh
-c skip_pre_agent_validation="true"
```

---

## Source

- `amplifier-bundle/recipes/smart-orchestrator.yaml` — context declaration
- `amplifier-bundle/recipes/default-workflow.yaml` — context declaration
- `amplifier-bundle/recipes/smart-execute-routing.yaml` — explicit forwarding
- `amplifier-bundle/recipes/workflow-prep.yaml` — bash consumption in step-01

## Related

- [Recipe Executor Environment](./recipe-executor-environment.md) — How context variables become environment variables in shell steps
- [Environment Variables](./environment-variables.md) — All variables read or injected by `amplihack`
- [Troubleshoot Recipe Execution](../howto/troubleshoot-recipe-execution.md) — Common recipe failure modes
- [Skip Pre-Agent Validation on Large Codebases](../howto/skip-pre-agent-validation.md) — Step-by-step guide for the most common use case
